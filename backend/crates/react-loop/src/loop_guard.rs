//! 本次运行的新调用流检测；每条回复只观察一次，覆盖全部调用。
use llm_provider::ToolCall;
use serde_json::Value;
use std::collections::VecDeque;

const WINDOW: usize = 12;

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum GuardAction {
    Continue,
    Warn,
    Stop,
}

#[derive(Default)]
pub(crate) struct LoopGuard {
    recent: VecDeque<(String, String)>,
}

// 显式排序对象键，避免 JSON 空白和键顺序变化绕过检测；数组顺序保留。
fn canonical(value: Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut entries: Vec<_> = map.into_iter().collect();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            Value::Object(
                entries
                    .into_iter()
                    .map(|(k, v)| (k, canonical(v)))
                    .collect(),
            )
        }
        Value::Array(values) => Value::Array(values.into_iter().map(canonical).collect()),
        other => other,
    }
}

impl LoopGuard {
    pub(crate) fn observe(&mut self, calls: &[ToolCall]) -> GuardAction {
        let mut action = GuardAction::Continue;
        for call in calls {
            let args = serde_json::from_str::<Value>(&call.function.arguments)
                .map(|v| canonical(v).to_string())
                .unwrap_or_else(|_| call.function.arguments.trim().to_owned());
            let key = (call.function.name.clone(), args);
            self.recent.push_back(key.clone());
            if self.recent.len() > WINDOW {
                self.recent.pop_front();
            }
            let count = self.recent.iter().filter(|item| **item == key).count();
            if count >= 6 {
                return GuardAction::Stop;
            }
            if count >= 3 {
                action = GuardAction::Warn;
            }
        }
        action
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn call(id: &str, name: &str, args: &str) -> ToolCall {
        ToolCall::new(id, name, args)
    }

    #[test]
    fn counts_every_call_including_same_batch_and_ignores_rounds_and_ids() {
        let mut guard = LoopGuard::default();
        let batch = vec![
            call("a", "read", "{}"),
            call("b", "read", "{}"),
            call("c", "read", "{}"),
        ];
        assert_eq!(guard.observe(&batch), GuardAction::Warn);
        // 新回复即使复用工具 ID 也是真实的新调用，不能漏算。
        assert_eq!(guard.observe(&batch), GuardAction::Stop);
    }

    #[test]
    fn detects_alternating_tools() {
        let mut guard = LoopGuard::default();
        let batch = vec![call("a", "read", "{}"), call("b", "shell", "{}")];
        assert_eq!(guard.observe(&batch), GuardAction::Continue);
        assert_eq!(guard.observe(&batch), GuardAction::Continue);
        assert_eq!(guard.observe(&batch), GuardAction::Warn);
        guard.observe(&batch);
        guard.observe(&batch);
        assert_eq!(guard.observe(&batch), GuardAction::Stop);
    }

    #[test]
    fn normalizes_nested_json_but_preserves_values_and_tool_names() {
        let mut guard = LoopGuard::default();
        for args in [
            r#"{"b":{"y":2,"x":1},"a":0}"#,
            r#"{ "a":0, "b":{"x":1,"y":2}}"#,
        ] {
            assert_eq!(
                guard.observe(&[call("a", "read", args)]),
                GuardAction::Continue
            );
        }
        assert_eq!(
            guard.observe(&[call("a", "read", r#"{"a":0,"b":{"x":1,"y":2}}"#)]),
            GuardAction::Warn
        );
        assert_eq!(
            guard.observe(&[call("a", "write", "{}")]),
            GuardAction::Continue
        );
        assert_eq!(
            guard.observe(&[call("a", "read", r#"{"a":1}"#)]),
            GuardAction::Continue
        );
    }

    #[test]
    fn old_calls_expire_and_new_runs_start_fresh() {
        let mut guard = LoopGuard::default();
        guard.observe(&[call("a", "read", "{}"), call("b", "read", "{}")]);
        for n in 0..12 {
            guard.observe(&[call("x", "read", &format!("{{\"n\":{n}}}"))]);
        }
        assert_eq!(
            guard.observe(&[call("a", "read", "{}")]),
            GuardAction::Continue
        );
        assert_eq!(
            LoopGuard::default().observe(&[call("a", "read", "{}")]),
            GuardAction::Continue
        );
    }
}
