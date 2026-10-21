//! 审计日志：每次安全拒绝 / 批准即追加写 `app_dir/log/audit/{session_id}.jsonl`。
//!
//! 单条记录为一行 JSON：
//! `{ session_id, time, type, detail, approval, approved_at?, command?, category?, source? }`
//! - `type`：file_read / file_write / bash / bash_exec / network / resource / browser
//! - `approval`：agree（手动同意）/ deny（手动拒绝）/ auto（自动放行或自动二跑）/ none（无审批）
//! - `category` / `source`：安全拦截分类与来源可信度（非拦截场景缺省不写；来源为
//!   分类的确定/推断依据，前端据此标注「疑似」）

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::tool::denial::DenialSource;
use serde::Serialize;

/// 审计操作类型。
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AuditKind {
    FileRead,
    FileWrite,
    Bash,
    Network,
    Resource,
    /// 浏览器自动化（browser 工具：审批放行的写操作 / 被拒的网页动作）
    Browser,
    /// 邮件操作（mail 工具：发信确认 / 移动 / Plan 硬拒）
    Mail,
    /// 普通命令执行（**每次** bash 执行一条轻量事实记录，不含审批决策）。
    /// 显式 `rename`：容器级 lowercase 会得到 `bashexec`，审计页筛选/人工 grep
    /// 都更需要可读的下划线分隔形式，故单独固定为 snake_case。
    #[serde(rename = "bash_exec")]
    BashExec,
}

/// 审批动作。
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AuditApproval {
    /// 手动同意（无防护重跑）
    Agree,
    /// 手动拒绝
    Deny,
    /// 自动放行 / 自动二跑
    Auto,
    /// 无审批（资源 / 危险命令拦截、普通命令执行等）
    None,
}

/// 单条审计记录。
#[derive(Debug, Serialize)]
pub struct AuditRecord {
    pub session_id: String,
    /// unix 毫秒
    pub time: u64,
    pub r#type: AuditKind,
    /// 沙箱 / 守卫反馈明细（尽量含命令 / 路径 / 目标）
    pub detail: String,
    pub approval: AuditApproval,
    /// 审批动作发生时间（unix 毫秒；拦截 / 待审批即 approval=None 时为 None）
    pub approved_at: Option<u64>,
    /// 执行前输入的原始命令（bash 原文 / file_operation 等；无则缺省不写）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// 安全拦截分类（拒绝读 / 拒绝写 / 危险命令 / 网络 / 资源 / 浏览器自动化；非拦截场景缺省不写）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<crate::tool::denial::DenialCategory>,
    /// 分类来源 / 可信度（静态预检 / 沙箱预检 / 沙箱运行期 / 退出码归因；无分类或来源未知时缺省不写）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<DenialSource>,
}

/// 审计写入器：按会话 id 追加到 `<log_dir>/audit/{session_id}.jsonl`。
pub struct AuditLog {
    dir: PathBuf,
    /// 串行化写，避免并发交错
    lock: Mutex<()>,
}

impl AuditLog {
    /// 创建（目录：`log_dir/audit`）。
    pub fn new(log_dir: PathBuf) -> Self {
        AuditLog {
            dir: log_dir.join("audit"),
            lock: Mutex::new(()),
        }
    }

    /// 写一条审计记录（目录不存在时创建；失败仅告警，不阻断执行）。
    /// approval=None（拦截 / 待审批）时 approved_at 置 None——审批动作尚未发生。
    /// `command` 记录执行前输入的原始命令（bash 原文等），供审计页精确还原，可为空。
    /// `category` / `source` 为安全拦截分类与来源可信度，非拦截场景传 None。
    #[allow(clippy::too_many_arguments)]
    pub fn record(
        &self,
        session_id: &str,
        kind: AuditKind,
        detail: &str,
        approval: AuditApproval,
        command: Option<&str>,
        category: Option<crate::tool::denial::DenialCategory>,
        source: Option<DenialSource>,
    ) {
        let now = unix_ms();
        let rec = AuditRecord {
            session_id: session_id.to_string(),
            time: now,
            r#type: kind,
            detail: detail.to_string(),
            approval,
            approved_at: if matches!(approval, AuditApproval::None) {
                None
            } else {
                Some(now)
            },
            command: command.map(|c| c.to_string()),
            category,
            source,
        };
        let _ = self.append_line(session_id, &rec);
    }

    fn append_line(&self, session_id: &str, rec: &AuditRecord) -> Result<(), String> {
        let _guard = self.lock.lock().map_err(|_| "审计锁中毒".to_string())?;
        if let Err(e) = fs::create_dir_all(&self.dir) {
            tracing::warn!("[audit] 创建审计目录失败: {e}");
            return Ok(());
        }
        let path = self.file_for(session_id);
        let line = serde_json::to_string(rec).unwrap_or_default();
        match OpenOptions::new().create(true).append(true).open(&path) {
            Ok(mut f) => {
                if let Err(e) = writeln!(f, "{line}") {
                    tracing::warn!("[audit] 写审计失败: {e}");
                }
            }
            Err(e) => tracing::warn!("[audit] 打开审计文件失败: {e}"),
        }
        Ok(())
    }

    fn file_for(&self, session_id: &str) -> PathBuf {
        // 会话 id 为 uuid（无路径字符）；兜底替换分隔符避免逃逸
        let safe: String = session_id
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
            .collect();
        self.dir.join(format!("{safe}.jsonl"))
    }

    /// 读取会话的完整审计记录，返回 JSON 数组字符串（`[{...}]`；文件缺失返回 `[]`）。
    /// 供设置页「审计日志」模块按会话展示；损坏行跳过。
    pub fn read_session_json(&self, session_id: &str) -> String {
        let path = self.file_for(session_id);
        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => return "[]".to_string(),
        };
        let items: Vec<serde_json::Value> = content
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect();
        serde_json::to_string(&items).unwrap_or_else(|_| "[]".to_string())
    }
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audit_writes_jsonl_per_session() {
        let dir = std::env::temp_dir().join(format!("astorm_audit_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let log = AuditLog::new(dir.clone());

        log.record(
            "abc",
            AuditKind::Bash,
            "curl example.com",
            AuditApproval::None,
            Some("curl example.com"),
            Some(crate::tool::denial::DenialCategory::Network),
            Some(DenialSource::SandboxRuntime),
        );
        log.record(
            "abc",
            AuditKind::FileWrite,
            "touch /etc/x",
            AuditApproval::None,
            Some("touch /etc/x"),
            Some(crate::tool::denial::DenialCategory::FileWrite),
            Some(DenialSource::StaticPrecheck),
        );

        let path = dir.join("audit").join("abc.jsonl");
        let content = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = content.lines().filter(|l| !l.trim().is_empty()).collect();
        assert_eq!(lines.len(), 2);
        let first: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(first["session_id"], "abc");
        assert_eq!(first["type"], "bash");
        assert_eq!(first["approval"], "none");
        assert_eq!(first["command"], "curl example.com");
        assert_eq!(first["category"], "network");
        assert_eq!(first["source"], "sandbox_runtime");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 轻量执行审计：每次 bash 执行一条（type=bash_exec），只带运行事实与命令原文，无审批决策。
    #[test]
    fn bash_exec_记录轻量执行事实() {
        let dir = std::env::temp_dir().join(format!("astorm_audit_exec_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let log = AuditLog::new(dir.clone());

        log.record(
            "exec",
            AuditKind::BashExec,
            "执行完成：exit=0 protected=true elapsed=12ms cwd=/ws",
            AuditApproval::None,
            Some("cargo build"),
            None,
            None,
        );

        let path = dir.join("audit").join("exec.jsonl");
        let content = std::fs::read_to_string(&path).unwrap();
        let line = content.lines().next().unwrap();
        let v: serde_json::Value = serde_json::from_str(line).unwrap();
        assert_eq!(v["type"], "bash_exec", "枚举串必须稳定为 bash_exec: {line}");
        assert_eq!(v["approval"], "none");
        assert_eq!(v["command"], "cargo build");
        assert_eq!(v["detail"], "执行完成：exit=0 protected=true elapsed=12ms cwd=/ws");
        assert!(v["approved_at"].is_null(), "无审批动作时 approved_at 应为 null");
        // 普通执行不是拦截场景：分类 / 来源字段缺省
        assert!(v.get("category").is_none(), "无分类时字段应缺省: {line}");
        assert!(v.get("source").is_none(), "无来源时字段应缺省: {line}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_session_json_returns_array() {
        let dir = std::env::temp_dir().join(format!("astorm_audit_read_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let log = AuditLog::new(dir.clone());

        // 未写过的会话 → 空数组
        assert_eq!(log.read_session_json("ghost"), "[]");

        log.record("abc", AuditKind::Bash, "ls -la", AuditApproval::None, Some("ls -la"), None, None);
        log.record(
            "abc",
            AuditKind::FileWrite,
            "cp x y",
            AuditApproval::Agree,
            Some("file_operation write x y"),
            Some(crate::tool::denial::DenialCategory::FileWrite),
            Some(DenialSource::StaticPrecheck),
        );

        let json = log.read_session_json("abc");
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        let arr = v.as_array().unwrap();
        assert_eq!(arr.len(), 2, "应返回两条记录: {json}");
        assert_eq!(arr[0]["detail"], "ls -la");
        assert_eq!(arr[0]["command"], "ls -la");
        assert!(arr[0].get("source").is_none(), "无来源时字段应缺省: {json}");
        assert_eq!(arr[1]["approval"], "agree");
        assert_eq!(arr[1]["command"], "file_operation write x y");
        assert_eq!(arr[1]["source"], "static_precheck");
        // 会话隔离
        assert_eq!(log.read_session_json("other"), "[]");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
