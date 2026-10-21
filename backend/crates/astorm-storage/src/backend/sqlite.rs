//! SQLite 结构化后端：[`SqliteStorage`]。
//!
//! 会话元数据表（`sessions`）将整条[`SessionRecord`]（富版）序列化为 JSON 存入 `data` 列，
//! 保留 `id` / `created_at` / `updated_at` 为可查询列（支撑按 id 去重与列表排序）；
//! 记录新增字段（如会话类型 `session_type`）无需迁移：JSON 列上带 `serde(default)` 即可兼容老行。
//! 消息历史表（`messages`）保持结构化列；`search` 为 `LIKE` 占位实现（真正全文检索由 Tantivy 承担）。

use std::path::Path;
use std::sync::Mutex;

use astorm_common::types::{
    AgentRecord, DeliverableRow, MessageRecord, PlanRecord, ScheduledTask, SearchHit, SessionRecord,
    Storage, StorageError, UsageEvent, UsageStatRow,
};
use rusqlite::{params, Connection, OptionalExtension, Row};

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS sessions (
    id          TEXT PRIMARY KEY,
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL,
    data        TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS messages (
    id          TEXT PRIMARY KEY,
    session_id  TEXT NOT NULL,
    role        TEXT NOT NULL,
    content     TEXT NOT NULL,
    tool_calls  TEXT NOT NULL,
    seq         INTEGER NOT NULL,
    created_at  INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_messages_session_seq ON messages(session_id, seq);
CREATE TABLE IF NOT EXISTS plans (
    task_id     TEXT PRIMARY KEY,
    session_id  TEXT NOT NULL,
    data        TEXT NOT NULL,
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_plans_session ON plans(session_id);
CREATE TABLE IF NOT EXISTS scheduled_tasks (
    task_id     TEXT PRIMARY KEY,
    data        TEXT NOT NULL,
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS agents (
    agent_id    TEXT PRIMARY KEY,
    data        TEXT NOT NULL,
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS usage_events (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id  TEXT NOT NULL,
    model       TEXT NOT NULL,
    provider    TEXT NOT NULL,
    ts          INTEGER NOT NULL,
    prompt      INTEGER NOT NULL,
    completion  INTEGER NOT NULL,
    cache_hit   INTEGER NOT NULL,
    cache_miss  INTEGER NOT NULL,
    total       INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_usage_events_ts ON usage_events(ts);
CREATE TABLE IF NOT EXISTS session_deliverables (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id    TEXT NOT NULL,
    round         INTEGER NOT NULL,
    session_title TEXT NOT NULL DEFAULT '',
    user_msg_seq  INTEGER NOT NULL DEFAULT 0,
    user_message  TEXT NOT NULL DEFAULT '',
    deliverables  TEXT NOT NULL DEFAULT '[]',
    changed_files TEXT NOT NULL DEFAULT '[]',
    memory_ids    TEXT NOT NULL DEFAULT '[]',
    created_at    INTEGER NOT NULL DEFAULT 0,
    updated_at    INTEGER NOT NULL DEFAULT 0,
    UNIQUE(session_id, round)
);
CREATE INDEX IF NOT EXISTS idx_deliverables_session ON session_deliverables(session_id, round);
CREATE INDEX IF NOT EXISTS idx_deliverables_updated ON session_deliverables(updated_at);
";

/// SQLite 结构化存储后端。
pub struct SqliteStorage {
    /// rusqlite `Connection` 为 `Send` 而非 `Sync`，用互斥锁串行化访问。
    conn: Mutex<Connection>,
}

impl SqliteStorage {
    /// 打开（或创建）`path` 指定的 SQLite 库并初始化表结构。
    ///
    /// 旧版 sessions 表（无 `data` 列）打开时检测到会直接 drop 重建：
    /// 当前存储尚未接入业务、无真实数据，无需迁移。
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StorageError> {
        let conn = Connection::open(path.as_ref())
            .map_err(|e| StorageError::Database(e.to_string()))?;
        // 桌面端与 astorm-server/CLI 可能同时指向同一数据目录：WAL 允许读写并行，
        // busy_timeout 避免并发写立刻报 "database is locked"。
        conn.busy_timeout(std::time::Duration::from_secs(5))
            .map_err(|e| StorageError::Database(e.to_string()))?;
        let _ = conn.pragma_update(None, "journal_mode", "WAL");
        conn.execute_batch(SCHEMA)
            .map_err(|e| StorageError::Database(e.to_string()))?;

        if !has_column(&conn, "sessions", "data")? {
            // 旧版 sessions 表（无 data 列）：当前无真实数据，drop 重建即可。
            conn.execute_batch("DROP TABLE sessions;")
                .map_err(|e| StorageError::Database(e.to_string()))?;
            conn.execute_batch(SCHEMA)
                .map_err(|e| StorageError::Database(e.to_string()))?;
        }

        Ok(Self {
            conn: Mutex::new(conn),
        })
    }
}

/// 检查表是否含指定列（PRAGMA table_info 无 params，直接字符串格式化成表名）。
fn has_column(conn: &Connection, table: &str, column: &str) -> Result<bool, StorageError> {
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info({table})"))
        .map_err(|e| StorageError::Database(e.to_string()))?;
    let rows = stmt
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|e| StorageError::Database(e.to_string()))?;
    for name in rows {
        if name.map_err(|e| StorageError::Database(e.to_string()))? == column {
            return Ok(true);
        }
    }
    Ok(false)
}

fn row_to_message(row: &Row) -> rusqlite::Result<MessageRecord> {
    let tool_calls: String = row.get(4)?;
    Ok(MessageRecord {
        id: row.get(0)?,
        session_id: row.get(1)?,
        role: row.get(2)?,
        content: row.get(3)?,
        tool_calls: serde_json::from_str(&tool_calls).unwrap_or(serde_json::Value::Null),
        seq: row.get(5)?,
        created_at: row.get(6)?,
        // sqlite 消息表为轻量列(富字段只走 jsonl),回退无值
        reasoning_content: None,
        tool_call_id: None,
        metadata: None,
    })
}

/// 转义 `LIKE` 通配符，避免查询串被当作模式。
fn escape_like(query: &str) -> String {
    query.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_")
}

impl Storage for SqliteStorage {
    fn upsert_session(&self, record: &SessionRecord) -> Result<(), StorageError> {
        let data = serde_json::to_string(record)?;
        let conn = self
            .conn
            .lock()
            .map_err(|_| StorageError::Other("连接锁中毒".into()))?;
        conn.execute(
            "INSERT INTO sessions (id, created_at, updated_at, data)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(id) DO UPDATE SET
                created_at = excluded.created_at,
                updated_at = excluded.updated_at,
                data = excluded.data",
            params![record.id.to_string(), record.created_at, record.updated_at, data],
        )
        .map_err(|e| StorageError::Database(e.to_string()))?;
        Ok(())
    }

    fn get_session(&self, session_id: &str) -> Result<Option<SessionRecord>, StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| StorageError::Other("连接锁中毒".into()))?;
        let data = conn
            .query_row(
                "SELECT data FROM sessions WHERE id = ?1",
                params![session_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|e| StorageError::Database(e.to_string()))?;
        data.map(|d| serde_json::from_str::<SessionRecord>(&d).map_err(StorageError::from))
            .transpose()
    }

    fn list_sessions(&self) -> Result<Vec<SessionRecord>, StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| StorageError::Other("连接锁中毒".into()))?;
        let mut stmt = conn
            .prepare("SELECT data FROM sessions ORDER BY created_at ASC")
            .map_err(|e| StorageError::Database(e.to_string()))?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|e| StorageError::Database(e.to_string()))?;
        let mut list = Vec::new();
        for row in rows {
            let data = row.map_err(|e| StorageError::Database(e.to_string()))?;
            list.push(serde_json::from_str::<SessionRecord>(&data)?);
        }
        Ok(list)
    }

    fn delete_session(&self, session_id: &str) -> Result<(), StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| StorageError::Other("连接锁中毒".into()))?;
        conn.execute("DELETE FROM messages WHERE session_id = ?1", params![session_id])
            .and_then(|_| conn.execute("DELETE FROM plans WHERE session_id = ?1", params![session_id]))
            .and_then(|_| conn.execute("DELETE FROM usage_events WHERE session_id = ?1", params![session_id]))
            .and_then(|_| conn.execute("DELETE FROM session_deliverables WHERE session_id = ?1", params![session_id]))
            .and_then(|_| conn.execute("DELETE FROM sessions WHERE id = ?1", params![session_id]))
            .map_err(|e| StorageError::Database(e.to_string()))?;
        Ok(())
    }

    fn append_messages(&self, _session_id: &str, messages: &[MessageRecord]) -> Result<(), StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| StorageError::Other("连接锁中毒".into()))?;
        for msg in messages {
            let tool_calls = serde_json::to_string(&msg.tool_calls)?;
            conn.execute(
                "INSERT OR REPLACE INTO messages (id, session_id, role, content, tool_calls, seq, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    msg.id,
                    msg.session_id,
                    msg.role,
                    msg.content,
                    tool_calls,
                    msg.seq,
                    msg.created_at
                ],
            )
            .map_err(|e| StorageError::Database(e.to_string()))?;
        }
        Ok(())
    }

    fn load_messages(&self, session_id: &str) -> Result<Vec<MessageRecord>, StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| StorageError::Other("连接锁中毒".into()))?;
        let mut stmt = conn
            .prepare(
                "SELECT id, session_id, role, content, tool_calls, seq, created_at
                 FROM messages WHERE session_id = ?1 ORDER BY seq ASC",
            )
            .map_err(|e| StorageError::Database(e.to_string()))?;
        let rows = stmt
            .query_map(params![session_id], row_to_message)
            .map_err(|e| StorageError::Database(e.to_string()))?;
        let mut list = Vec::new();
        for row in rows {
            list.push(row.map_err(|e| StorageError::Database(e.to_string()))?);
        }
        Ok(list)
    }

    fn rewrite_live_messages(
        &self,
        session_id: &str,
        _index: u32,
        messages: &[MessageRecord],
    ) -> Result<(), StorageError> {
        // SQLite 消息表可按会话整段替换（无活动/备份文件之分）：先清空该会话消息再写入压缩结果。
        {
            let conn = self
                .conn
                .lock()
                .map_err(|_| StorageError::Other("连接锁中毒".into()))?;
            conn.execute("DELETE FROM messages WHERE session_id = ?1", params![session_id])
                .map_err(|e| StorageError::Database(e.to_string()))?;
        }
        self.append_messages(session_id, messages)
    }

    /// 撤回截断：删除 `seq >= keep_seq_exclusive` 的消息，返回被删 id。
    fn truncate_messages(
        &self,
        session_id: &str,
        keep_seq_exclusive: i64,
    ) -> Result<Vec<String>, StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| StorageError::Other("连接锁中毒".into()))?;
        let ids: Vec<String> = {
            let mut stmt = conn
                .prepare("SELECT id FROM messages WHERE session_id = ?1 AND seq >= ?2")
                .map_err(|e| StorageError::Database(e.to_string()))?;
            let rows = stmt
                .query_map(params![session_id, keep_seq_exclusive], |r| r.get::<_, String>(0))
                .map_err(|e| StorageError::Database(e.to_string()))?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row.map_err(|e| StorageError::Database(e.to_string()))?);
            }
            out
        };
        if !ids.is_empty() {
            conn.execute(
                "DELETE FROM messages WHERE session_id = ?1 AND seq >= ?2",
                params![session_id, keep_seq_exclusive],
            )
            .map_err(|e| StorageError::Database(e.to_string()))?;
        }
        Ok(ids)
    }

    fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>, StorageError> {
        let pattern = format!("%{}%", escape_like(query));
        let conn = self
            .conn
            .lock()
            .map_err(|_| StorageError::Other("连接锁中毒".into()))?;
        let mut stmt = conn
            .prepare(
                "SELECT id, session_id, content FROM messages
                 WHERE content LIKE ?1 ESCAPE '\\'
                 ORDER BY created_at DESC LIMIT ?2",
            )
            .map_err(|e| StorageError::Database(e.to_string()))?;
        let rows = stmt
            .query_map(params![pattern, limit as i64], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .map_err(|e| StorageError::Database(e.to_string()))?;
        let mut hits = Vec::new();
        for row in rows {
            let (id, session_id, content) = row.map_err(|e| StorageError::Database(e.to_string()))?;
            hits.push(SearchHit {
                session_id,
                message_id: id,
                score: 1.0,
                snippet: Some(content),
            });
        }
        Ok(hits)
    }

    fn upsert_plan(&self, session_id: &str, plan: &PlanRecord) -> Result<(), StorageError> {
        let data = serde_json::to_string(plan)?;
        let conn = self
            .conn
            .lock()
            .map_err(|_| StorageError::Other("连接锁中毒".into()))?;
        conn.execute(
            "INSERT INTO plans (task_id, session_id, data, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(task_id) DO UPDATE SET
                session_id = excluded.session_id,
                data = excluded.data,
                created_at = excluded.created_at,
                updated_at = excluded.updated_at",
            params![plan.task_id, session_id, data, plan.created_at, plan.updated_at],
        )
        .map_err(|e| StorageError::Database(e.to_string()))?;
        Ok(())
    }

    fn get_plan(&self, session_id: &str, task_id: &str) -> Result<Option<PlanRecord>, StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| StorageError::Other("连接锁中毒".into()))?;
        let data = conn
            .query_row(
                "SELECT data FROM plans WHERE session_id = ?1 AND task_id = ?2",
                params![session_id, task_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|e| StorageError::Database(e.to_string()))?;
        data.map(|d| serde_json::from_str::<PlanRecord>(&d).map_err(StorageError::from))
            .transpose()
    }

    fn delete_plan(&self, session_id: &str, task_id: &str) -> Result<(), StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| StorageError::Other("连接锁中毒".into()))?;
        conn.execute(
            "DELETE FROM plans WHERE session_id = ?1 AND task_id = ?2",
            params![session_id, task_id],
        )
        .map_err(|e| StorageError::Database(e.to_string()))?;
        Ok(())
    }

    fn list_plans(&self, session_id: &str) -> Result<Vec<PlanRecord>, StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| StorageError::Other("连接锁中毒".into()))?;
        let mut stmt = conn
            .prepare("SELECT data FROM plans WHERE session_id = ?1 ORDER BY created_at DESC")
            .map_err(|e| StorageError::Database(e.to_string()))?;
        let rows = stmt
            .query_map(params![session_id], |row| row.get::<_, String>(0))
            .map_err(|e| StorageError::Database(e.to_string()))?;
        let mut plans = Vec::new();
        for row in rows {
            let data = row.map_err(|e| StorageError::Database(e.to_string()))?;
            plans.push(serde_json::from_str::<PlanRecord>(&data).map_err(StorageError::from)?);
        }
        Ok(plans)
    }

    fn list_scheduled_tasks(&self) -> Result<Vec<ScheduledTask>, StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| StorageError::Other("连接锁中毒".into()))?;
        let mut stmt = conn
            .prepare("SELECT data FROM scheduled_tasks ORDER BY created_at DESC")
            .map_err(|e| StorageError::Database(e.to_string()))?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|e| StorageError::Database(e.to_string()))?;
        let mut list = Vec::new();
        for row in rows {
            let data = row.map_err(|e| StorageError::Database(e.to_string()))?;
            list.push(serde_json::from_str::<ScheduledTask>(&data)?);
        }
        Ok(list)
    }

    fn upsert_scheduled_task(&self, task: &ScheduledTask) -> Result<(), StorageError> {
        let data = serde_json::to_string(task)?;
        let conn = self
            .conn
            .lock()
            .map_err(|_| StorageError::Other("连接锁中毒".into()))?;
        conn.execute(
            "INSERT INTO scheduled_tasks (task_id, data, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(task_id) DO UPDATE SET
                data = excluded.data,
                created_at = excluded.created_at,
                updated_at = excluded.updated_at",
            params![task.task_id, data, task.created_at, task.updated_at],
        )
        .map_err(|e| StorageError::Database(e.to_string()))?;
        Ok(())
    }

    fn get_scheduled_task(&self, task_id: &str) -> Result<Option<ScheduledTask>, StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| StorageError::Other("连接锁中毒".into()))?;
        let data = conn
            .query_row(
                "SELECT data FROM scheduled_tasks WHERE task_id = ?1",
                params![task_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|e| StorageError::Database(e.to_string()))?;
        data.map(|d| serde_json::from_str::<ScheduledTask>(&d).map_err(StorageError::from))
            .transpose()
    }

    fn delete_scheduled_task(&self, task_id: &str) -> Result<(), StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| StorageError::Other("连接锁中毒".into()))?;
        conn.execute(
            "DELETE FROM scheduled_tasks WHERE task_id = ?1",
            params![task_id],
        )
        .map_err(|e| StorageError::Database(e.to_string()))?;
        Ok(())
    }

    fn list_agents(&self) -> Result<Vec<AgentRecord>, StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| StorageError::Other("连接锁中毒".into()))?;
        let mut stmt = conn
            .prepare("SELECT data FROM agents ORDER BY created_at ASC")
            .map_err(|e| StorageError::Database(e.to_string()))?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|e| StorageError::Database(e.to_string()))?;
        let mut list = Vec::new();
        for row in rows {
            let data = row.map_err(|e| StorageError::Database(e.to_string()))?;
            list.push(serde_json::from_str::<AgentRecord>(&data)?);
        }
        // 内置卡片置前（内置主智能体是「默认」语义，列表首位可读性最好），其余保持创建顺序
        list.sort_by_key(|a| !a.builtin);
        Ok(list)
    }

    fn upsert_agent(&self, agent: &AgentRecord) -> Result<(), StorageError> {
        let data = serde_json::to_string(agent)?;
        let conn = self
            .conn
            .lock()
            .map_err(|_| StorageError::Other("连接锁中毒".into()))?;
        conn.execute(
            "INSERT INTO agents (agent_id, data, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(agent_id) DO UPDATE SET
                data = excluded.data,
                created_at = excluded.created_at,
                updated_at = excluded.updated_at",
            params![agent.agent_id, data, agent.created_at, agent.updated_at],
        )
        .map_err(|e| StorageError::Database(e.to_string()))?;
        Ok(())
    }

    fn get_agent(&self, agent_id: &str) -> Result<Option<AgentRecord>, StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| StorageError::Other("连接锁中毒".into()))?;
        let data = conn
            .query_row(
                "SELECT data FROM agents WHERE agent_id = ?1",
                params![agent_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|e| StorageError::Database(e.to_string()))?;
        data.map(|d| serde_json::from_str::<AgentRecord>(&d).map_err(StorageError::from))
            .transpose()
    }

    fn delete_agent(&self, agent_id: &str) -> Result<(), StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| StorageError::Other("连接锁中毒".into()))?;
        conn.execute("DELETE FROM agents WHERE agent_id = ?1", params![agent_id])
            .map_err(|e| StorageError::Database(e.to_string()))?;
        Ok(())
    }

    fn append_usage_event(&self, event: &UsageEvent) -> Result<(), StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| StorageError::Other("连接锁中毒".into()))?;
        conn.execute(
            "INSERT INTO usage_events
                (session_id, model, provider, ts, prompt, completion, cache_hit, cache_miss, total)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                event.session_id,
                event.model,
                event.provider,
                event.ts,
                event.prompt,
                event.completion,
                event.cache_hit,
                event.cache_miss,
                event.total
            ],
        )
        .map_err(|e| StorageError::Database(e.to_string()))?;
        Ok(())
    }

    fn usage_stats(&self, since_ts: i64) -> Result<Vec<UsageStatRow>, StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| StorageError::Other("连接锁中毒".into()))?;
        let mut stmt = conn
            .prepare(
                "SELECT strftime('%Y-%m-%d', ts, 'unixepoch', 'localtime') AS day,
                        model,
                        SUM(prompt), SUM(completion), SUM(cache_hit), SUM(cache_miss), SUM(total)
                 FROM usage_events
                 WHERE ts >= ?1
                 GROUP BY day, model
                 ORDER BY day ASC",
            )
            .map_err(|e| StorageError::Database(e.to_string()))?;
        let rows = stmt
            .query_map(params![since_ts], |row| {
                Ok(UsageStatRow {
                    day: row.get(0)?,
                    model: row.get(1)?,
                    prompt: row.get::<_, i64>(2)?.max(0) as u64,
                    completion: row.get::<_, i64>(3)?.max(0) as u64,
                    cache_hit: row.get::<_, i64>(4)?.max(0) as u64,
                    cache_miss: row.get::<_, i64>(5)?.max(0) as u64,
                    total: row.get::<_, i64>(6)?.max(0) as u64,
                })
            })
            .map_err(|e| StorageError::Database(e.to_string()))?;
        let mut list = Vec::new();
        for row in rows {
            list.push(row.map_err(|e| StorageError::Database(e.to_string()))?);
        }
        Ok(list)
    }

    fn upsert_deliverable(&self, row: &DeliverableRow) -> Result<(), StorageError> {
        let deliverables = serde_json::to_string(&row.deliverables)?;
        let changed_files = serde_json::to_string(&row.changed_files)?;
        let memory_ids = serde_json::to_string(&row.memory_ids)?;
        let conn = self
            .conn
            .lock()
            .map_err(|_| StorageError::Other("连接锁中毒".into()))?;
        conn.execute(
            "INSERT INTO session_deliverables
                (session_id, round, session_title, user_msg_seq, user_message,
                 deliverables, changed_files, memory_ids, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT(session_id, round) DO UPDATE SET
                session_title = excluded.session_title,
                user_msg_seq = excluded.user_msg_seq,
                user_message = excluded.user_message,
                deliverables = excluded.deliverables,
                changed_files = excluded.changed_files,
                memory_ids = excluded.memory_ids,
                updated_at = excluded.updated_at",
            params![
                row.session_id,
                row.round,
                row.session_title,
                row.user_msg_seq,
                row.user_message,
                deliverables,
                changed_files,
                memory_ids,
                row.created_at,
                row.updated_at
            ],
        )
        .map_err(|e| StorageError::Database(e.to_string()))?;
        Ok(())
    }

    fn list_deliverables(&self, session_id: &str) -> Result<Vec<DeliverableRow>, StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| StorageError::Other("连接锁中毒".into()))?;
        let mut stmt = conn
            .prepare(
                "SELECT session_id, round, session_title, user_msg_seq, user_message,
                        deliverables, changed_files, memory_ids, created_at, updated_at
                 FROM session_deliverables WHERE session_id = ?1 ORDER BY round ASC",
            )
            .map_err(|e| StorageError::Database(e.to_string()))?;
        let rows = stmt
            .query_map(params![session_id], row_to_deliverable)
            .map_err(|e| StorageError::Database(e.to_string()))?;
        let mut list = Vec::new();
        for row in rows {
            list.push(row.map_err(|e| StorageError::Database(e.to_string()))?);
        }
        Ok(list)
    }

    fn delete_deliverables_from(&self, session_id: &str, min_round: i32) -> Result<(), StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| StorageError::Other("连接锁中毒".into()))?;
        conn.execute(
            "DELETE FROM session_deliverables WHERE session_id = ?1 AND round >= ?2",
            params![session_id, min_round],
        )
        .map_err(|e| StorageError::Database(e.to_string()))?;
        Ok(())
    }

    fn list_all_deliverables(&self, limit: i64) -> Result<Vec<DeliverableRow>, StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| StorageError::Other("连接锁中毒".into()))?;
        let mut stmt = conn
            .prepare(
                "SELECT session_id, round, session_title, user_msg_seq, user_message,
                        deliverables, changed_files, memory_ids, created_at, updated_at
                 FROM session_deliverables ORDER BY updated_at DESC, session_id ASC, round DESC LIMIT ?1",
            )
            .map_err(|e| StorageError::Database(e.to_string()))?;
        let rows = stmt
            .query_map(params![limit], row_to_deliverable)
            .map_err(|e| StorageError::Database(e.to_string()))?;
        let mut list = Vec::new();
        for row in rows {
            list.push(row.map_err(|e| StorageError::Database(e.to_string()))?);
        }
        Ok(list)
    }
}

/// `session_deliverables` 行 → [`DeliverableRow`]（三个清单列反序列化，坏数据回退空数组）。
fn row_to_deliverable(row: &Row) -> rusqlite::Result<DeliverableRow> {
    fn parse_list<T: serde::de::DeserializeOwned + Default>(text: String) -> T {
        serde_json::from_str(&text).unwrap_or_default()
    }
    Ok(DeliverableRow {
        session_id: row.get(0)?,
        round: row.get(1)?,
        session_title: row.get(2)?,
        user_msg_seq: row.get(3)?,
        user_message: row.get(4)?,
        deliverables: parse_list(row.get(5)?),
        changed_files: parse_list(row.get(6)?),
        memory_ids: parse_list(row.get(7)?),
        created_at: row.get(8)?,
        updated_at: row.get(9)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(model: &str, ts: i64, total: u32) -> UsageEvent {
        UsageEvent {
            session_id: "s1".into(),
            model: model.into(),
            provider: "p".into(),
            ts,
            prompt: total,
            completion: 0,
            cache_hit: 0,
            cache_miss: 0,
            total,
        }
    }

    #[test]
    fn usage_events_aggregate_by_day_and_model() {
        let s = SqliteStorage::open(":memory:").expect("open in-memory");
        s.append_usage_event(&event("m1", 1_700_000_000, 10)).unwrap();
        s.append_usage_event(&event("m1", 1_700_000_100, 5)).unwrap();
        s.append_usage_event(&event("m2", 1_700_000_200, 7)).unwrap();

        let rows = s.usage_stats(0).unwrap();
        assert_eq!(rows.iter().map(|r| r.total).sum::<u64>(), 22);
        let m1 = rows.iter().find(|r| r.model == "m1").expect("m1 聚合行");
        assert_eq!(m1.total, 15);
        assert_eq!(m1.prompt, 15);
        assert!(m1.day.len() == 10 && m1.day.as_bytes()[4] == b'-', "day={}", m1.day);

        // since_ts 过滤：只保留 m2
        let recent = s.usage_stats(1_700_000_150).unwrap();
        assert_eq!(recent.iter().map(|r| r.total).sum::<u64>(), 7);
    }

    fn deliverable(session_id: &str, round: i32, memory_ids: &[&str]) -> DeliverableRow {
        DeliverableRow {
            session_id: session_id.into(),
            round,
            session_title: format!("标题{round}"),
            user_msg_seq: round as i64,
            user_message: format!("第 {round} 条用户消息"),
            deliverables: vec![serde_json::json!({ "path": "docs/a.md", "name": "a.md", "size": 8 })],
            changed_files: vec![serde_json::json!({
                "path": "src/a.rs", "name": "a.rs", "dir": "src/",
                "status": "M", "additions": 3, "deletions": 1
            })],
            memory_ids: memory_ids.iter().map(|s| s.to_string()).collect(),
            created_at: 1_700_000_000,
            updated_at: 1_700_000_000,
        }
    }

    #[test]
    fn deliverables_upsert_overwrites_and_lists_sorted() {
        let s = SqliteStorage::open(":memory:").expect("open in-memory");
        s.upsert_deliverable(&deliverable("s1", 2, &["b/x"])).unwrap();
        s.upsert_deliverable(&deliverable("s1", 1, &["a/y"])).unwrap();
        // 同轮重调覆盖（updated_at 变化 + 新清单），不产生第二行
        s.upsert_deliverable(&deliverable("s1", 1, &["a/y", "z/z"])).unwrap();

        let rows = s.list_deliverables("s1").unwrap();
        assert_eq!(rows.len(), 2, "同轮 upsert 覆盖而非追加");
        assert_eq!(rows[0].round, 1, "按轮次升序");
        assert_eq!(rows[0].memory_ids, vec!["a/y", "z/z"], "覆盖生效");
        assert_eq!(rows[0].deliverables[0]["name"], "a.md");
        assert_eq!(rows[0].changed_files[0]["additions"], 3);

        // 其他会话隔离；空会话返回空数组
        s.upsert_deliverable(&deliverable("s2", 1, &[])).unwrap();
        assert_eq!(s.list_deliverables("s2").unwrap().len(), 1);
        assert!(s.list_deliverables("nope").unwrap().is_empty());
    }

    #[test]
    fn deliverables_list_all_orders_by_updated_desc_with_limit() {
        let s = SqliteStorage::open(":memory:").expect("open in-memory");
        let mut old = deliverable("s1", 1, &[]);
        old.updated_at = 1_700_000_100;
        s.upsert_deliverable(&old).unwrap();
        let mut mid = deliverable("s2", 1, &[]);
        mid.updated_at = 1_700_000_200;
        s.upsert_deliverable(&mid).unwrap();
        let mut newest = deliverable("s1", 2, &[]);
        newest.updated_at = 1_700_000_300;
        s.upsert_deliverable(&newest).unwrap();

        let rows = s.list_all_deliverables(10).unwrap();
        assert_eq!(
            rows.iter().map(|r| r.updated_at).collect::<Vec<_>>(),
            vec![1_700_000_300, 1_700_000_200, 1_700_000_100],
            "跨会话按更新时间倒序"
        );
        // limit 限量取最新
        let top = s.list_all_deliverables(1).unwrap();
        assert_eq!(top.len(), 1);
        assert_eq!(top[0].updated_at, 1_700_000_300);
    }

    #[test]
    fn deliverables_delete_from_and_cascade_on_delete_session() {
        let s = SqliteStorage::open(":memory:").expect("open in-memory");
        for round in 1..=3 {
            s.upsert_deliverable(&deliverable("s1", round, &[])).unwrap();
        }
        // 撤回到保留 1 条用户消息：第 2、3 轮的行应被清理
        s.delete_deliverables_from("s1", 2).unwrap();
        let rows = s.list_deliverables("s1").unwrap();
        assert_eq!(rows.iter().map(|r| r.round).collect::<Vec<_>>(), vec![1]);

        // 删除会话级联清空交付物行
        s.upsert_deliverable(&deliverable("s9", 1, &[])).unwrap();
        s.delete_session("s9").unwrap();
        assert!(s.list_deliverables("s9").unwrap().is_empty());
    }
}