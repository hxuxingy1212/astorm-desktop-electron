//! 中断台账：会话级「待用户决议」的统一登记表（安全审批 + 向用户提问）。
//!
//! 工具在需要中断时经 `policy::needs_approval` / `tool::chat` 登记一条 [`PendingInterrupt`]，
//! 并把中断信封作为 tool 结果返回；引擎据此中断循环、发带 `payload` 的 `interrupt` 事件。
//! 用户在前端决定后经 `submitInterrupt`（`InterruptResolution`）交回后端：
//! **入队时**在此取走待决项（不存在 → 拒绝，避免重复/失效决议），**出队时**由
//! `agent::interrupt` 就地把决议写回那条 tool 消息并续跑。
//!
//! 台账为进程内存态；**持久化载体是历史 tool 消息的 `metadata.interrupt`**
//! （见 `agent::interrupt::recover_pending_interrupts`）——冷启动时据此重建本台账。
//!
//! **待决项不设 TTL / 不过期**：审批（或提问）挂起后一直有效，直到被决议——同意 / 拒绝 /
//! 回答 / 忽略，或被同一会话的新用户消息取代（`superseded`）。冷启动重建后仍可继续决议。
//!
//! 另存「允许一次性重放 / 已拒绝」两个**审批专用**闸门（`allowed_once` / `denied`）：
//! 工具重放时经 `consume_allowed_rerun` / `is_denied` 查询，实现「同意后整条无防护执行一次」。
//! 这两个闸门有 60s 窗口（[`GATE_TTL`]），与待决项寿命无关——它们只覆盖「决议之后的一次重放」。

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use astorm_common::session::InterruptType;
use serde_json::Value;

/// 一条待决议的中断请求（登记 / 决议 / 持久化恢复共用）。
#[derive(Debug, Clone)]
pub struct PendingInterrupt {
    /// 中断请求唯一标识（前端决议时携带；`uuid`）。
    pub request_id: String,
    /// 中断类型（审批 / 提问）。
    pub interrupt_type: InterruptType,
    /// 被中断的工具调用 id（决议时就地改写对应 tool 消息）。
    pub tool_call_id: String,
    /// 发起中断的工具名（审批重放时按名查找工具）。
    pub tool_name: String,
    /// 完整中断信封（工具返回的那份 JSON；随 metadata.interrupt 持久化，供前端重绘）。
    pub request: Value,
    /// 审批专用：被拦操作原文（重放闸门 key；form 为 None）。
    pub command: Option<String>,
    /// 登记时刻（unix 秒）。
    pub requested_at: i64,
}

impl PendingInterrupt {
    /// 审批信封中的被拦操作原文（`approval.command`）。
    pub fn command_of(request: &Value) -> Option<String> {
        request
            .get("approval")
            .and_then(|a| a.get("command"))
            .and_then(|c| c.as_str())
            .map(str::to_string)
    }
}

/// 中断台账（进程级；挂在 AppState）。所有操作快速、无阻塞。
#[derive(Default)]
pub struct InterruptRegistry {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    /// 待决议：`(session_id, request_id) → 待决项`；**无 TTL**，只随决议 / 取代移除
    pending: HashMap<(String, String), PendingInterrupt>,
    /// 审批：已允许、待消费的重放额度 `(session_id, command) → 允许时刻`（一次性 + 60s 窗口）
    allowed_once: HashMap<(String, String), Instant>,
    /// 审批：已拒绝 `(session_id, command) → 拒绝时刻`（供工具文案提示；60s 窗口）
    denied: HashMap<(String, String), Instant>,
}

/// 审批**闸门**窗口（同意后的待消费重放额度 / 拒绝后的同命令短路）。
///
/// 注意：这**不是**审批请求的过期时间——待决项无 TTL（见 [`InterruptRegistry::register_pending`]），
/// 挂起直到被决议。本窗口只影响「决议已作出之后」的重放闸门：决议出队即重放，
/// 正常在毫秒级完成，故窗口只需覆盖一次重放；超窗口的额度/拒绝标记由 `sweep` 清理。
const GATE_TTL: Duration = Duration::from_secs(60);

impl InterruptRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// 登记一条待决议中断（同 `request_id` 幂等覆盖）。
    ///
    /// 待决项**不设 TTL**：持久到被决议（同意 / 拒绝 / 回答 / 忽略 / 被新消息取代），
    /// 与历史 tool 消息上的 `metadata.interrupt` 标记同寿命。
    pub fn register_pending(&self, session_id: &str, item: PendingInterrupt) {
        self.sweep();
        let mut g = self.inner.lock().unwrap();
        g.pending
            .insert((session_id.to_string(), item.request_id.clone()), item);
    }

    /// 取走一条待决项（决议入队时调用；不存在 → None，调用方据此拒绝该决议）。
    pub fn take(&self, session_id: &str, request_id: &str) -> Option<PendingInterrupt> {
        self.inner
            .lock()
            .unwrap()
            .pending
            .remove(&(session_id.to_string(), request_id.to_string()))
    }

    /// 移除并返回该会话全部待决项（普通用户消息取代 pending 时清理）。
    pub fn take_session_pending(&self, session_id: &str) -> Vec<PendingInterrupt> {
        let mut g = self.inner.lock().unwrap();
        let keys: Vec<(String, String)> = g
            .pending
            .keys()
            .filter(|(sid, _)| sid == session_id)
            .cloned()
            .collect();
        keys.iter().filter_map(|k| g.pending.remove(k)).collect()
    }

    /// 该会话当前是否有待决中断（只读；决议续跑后判断是否应保持「等待用户处理」态）。
    pub fn has_session_pending(&self, session_id: &str) -> bool {
        self.inner
            .lock()
            .unwrap()
            .pending
            .keys()
            .any(|(sid, _)| sid == session_id)
    }

    /// 审批同意：登记一次性「允许重放」额度（工具内 `consume_allowed_rerun` 命中即无防护执行）。
    pub fn allow_oneshot(&self, session_id: &str, command: &str) {
        let mut g = self.inner.lock().unwrap();
        g.denied.remove(&(session_id.to_string(), command.to_string()));
        g.allowed_once
            .insert((session_id.to_string(), command.to_string()), Instant::now());
    }

    /// 审批拒绝：记录拒绝（供同命令的后续尝试直接短路）。
    pub fn deny_command(&self, session_id: &str, command: &str) {
        let mut g = self.inner.lock().unwrap();
        g.allowed_once.remove(&(session_id.to_string(), command.to_string()));
        g.denied
            .insert((session_id.to_string(), command.to_string()), Instant::now());
    }

    /// 该命令当前是否已被拒绝（TTL 内）；供工具错误文案提示。
    pub fn is_denied(&self, session_id: &str, command: &str) -> bool {
        let g = self.inner.lock().unwrap();
        g.denied
            .get(&(session_id.to_string(), command.to_string()))
            .map(|t| t.elapsed() < GATE_TTL)
            .unwrap_or(false)
    }

    /// 尝试消费一次"允许重放"（命中即移除，一次性）。命中返回 true → 本次无防护执行。
    pub fn consume_allow(&self, session_id: &str, command: &str) -> bool {
        let mut g = self.inner.lock().unwrap();
        let key = (session_id.to_string(), command.to_string());
        match g.allowed_once.get(&key) {
            Some(t) if t.elapsed() < GATE_TTL => {
                g.allowed_once.remove(&key);
                true
            }
            _ => false,
        }
    }

    /// 清理过期的允许 / 拒绝闸门项（**待决项不清理**——它们不设 TTL，只随决议移除；
    /// 在登记路径顺带触发）。
    fn sweep(&self) {
        let mut g = self.inner.lock().unwrap();
        let now = Instant::now();
        g.allowed_once
            .retain(|_, t| now.duration_since(*t) < GATE_TTL);
        g.denied.retain(|_, t| now.duration_since(*t) < GATE_TTL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn approval(request_id: &str, command: &str) -> PendingInterrupt {
        let request = json!({
            "interrupt": true, "interrupt_type": "approval", "request_id": request_id,
            "tool_call_id": format!("call_{request_id}"),
            "approval": { "kind": "file_write", "command": command, "reason": "测试" },
        });
        PendingInterrupt {
            request_id: request_id.into(),
            interrupt_type: InterruptType::Approval,
            tool_call_id: format!("call_{request_id}"),
            tool_name: "bash".into(),
            command: PendingInterrupt::command_of(&request),
            request,
            requested_at: 1,
        }
    }

    #[test]
    fn take_is_atomic_and_unknown_returns_none() {
        let r = InterruptRegistry::new();
        r.register_pending("s1", approval("req1", "touch /etc/x"));
        let taken = r.take("s1", "req1").expect("首次取走应命中");
        assert_eq!(taken.command.as_deref(), Some("touch /etc/x"));
        assert!(r.take("s1", "req1").is_none(), "重复决议应取不到（前端会收到失效错误）");
        assert!(r.take("s2", "req1").is_none(), "会话不匹配不命中");
    }

    #[test]
    fn allow_oneshot_consumed_once_deny_blocks() {
        let r = InterruptRegistry::new();
        r.allow_oneshot("s1", "curl x");
        assert!(r.consume_allow("s1", "curl x"));
        assert!(!r.consume_allow("s1", "curl x"), "一次性额度消费后不再放行");

        r.deny_command("s1", "rm -rf /tmp/y");
        assert!(r.is_denied("s1", "rm -rf /tmp/y"));
        assert!(!r.consume_allow("s1", "rm -rf /tmp/y"), "拒绝的命令无放行额度");
    }

    #[test]
    fn take_session_pending_clears_only_that_session() {
        let r = InterruptRegistry::new();
        r.register_pending("s1", approval("req1", "a"));
        r.register_pending("s1", approval("req2", "b"));
        r.register_pending("s2", approval("req3", "c"));
        assert_eq!(r.take_session_pending("s1").len(), 2);
        assert!(r.take_session_pending("s1").is_empty());
        assert_eq!(r.take_session_pending("s2").len(), 1);
    }

    #[test]
    fn command_of_extracts_only_approval_command() {
        assert_eq!(
            PendingInterrupt::command_of(&json!({ "approval": { "command": "ls -la" } })),
            Some("ls -la".into())
        );
        assert_eq!(PendingInterrupt::command_of(&json!({ "form": { "fields": [] } })), None);
    }
}
