//! LLM 调用抽象：`MemoryLlm` trait + 生产实现与两个测试/离线替身。
//!
//! 管道只依赖 [`MemoryLlm`] trait，因此：
//! - 生产环境注入 [`ChatLlm`]（`llm-provider` 的 `ChatClient`，OpenAI 兼容 `/chat/completions`）
//! - 单测注入 [`ScriptedLlm`]（按阶段返回预置 JSON）⇒ **全管道离线可测、可断言幂等**
//! - `--dry-run` 注入 [`DryRunLlm`]（只记录提示词、不发请求）⇒ 无密钥也能验证提示词构造
//!
//! 关于「重复实现 JSON 补全」：`astorm-core::util::llm_util::complete_json` 语义相同，但本 crate
//! 为保持**独立可发布、不依赖 astorm-core**（模块解耦要求）而自带一份；逻辑很短（强制
//! `json_object` + 注入含 "json" 的提示 + 围栏剥离 + 首 `{` 至末 `}` 回退）。
//!
//! 重试 / 退避 / 熔断交给 `llm-provider` 的 `ClientOption`；流中传输中断后自动重开
//! （`LlmEvent::Restart`）时必须**作废重试前累积的半截文本**，否则会拼成乱码。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use futures::StreamExt;
use llm_provider::{ChatClient, ChatMessage, ChatRequest, LlmEvent, TokenUsage};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

/// JSON 补全结果。
#[derive(Debug, Clone)]
pub struct LlmJsonOutcome {
    /// 解析后的 JSON。
    pub value: Value,
    pub usage: TokenUsage,
    pub finish_reason: String,
}

impl LlmJsonOutcome {
    /// 空结果（离线替身与容错路径用）。
    pub fn empty() -> Self {
        Self {
            value: json!({}),
            usage: TokenUsage::default(),
            finish_reason: "offline".into(),
        }
    }
}

/// 提示词所属阶段（供 [ScriptedLlm] 分派预置响应，与调用顺序解耦）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PromptStage {
    /// 原子记忆提取。
    Extract,
    /// 提取二遍自查。
    ExtractSelfCheck,
    /// 精选（去噪 + 合并同主题）。
    Curate,
    /// 记忆 → OKF 概念编译。
    Compile,
    /// 合并判定。
    Judge,
    Unknown,
}

/// 由 system 提示词判定阶段（**顺序无关**：脚本替身不依赖调用次序）。
pub fn detect_stage(system: &str) -> PromptStage {
    if system.contains("OKF Concept Compiler") {
        PromptStage::Compile
    } else if system.contains("memory consolidation judge") {
        PromptStage::Judge
    } else if system.contains("memory curator") {
        PromptStage::Curate
    } else if system.contains("memory-unit quality auditor") {
        PromptStage::ExtractSelfCheck
    } else if system.contains("information extraction engine") {
        PromptStage::Extract
    } else {
        PromptStage::Unknown
    }
}

/// JSON 补全抽象（管道唯一依赖的 LLM 接口）。
#[async_trait]
pub trait MemoryLlm: Send + Sync {
    /// 单次调用：给定 system / user，返回严格 JSON。
    async fn complete_json(
        &self,
        system: &str,
        user: &str,
        cancel: CancellationToken,
    ) -> Result<LlmJsonOutcome, String>;

    /// 模型标识（写入概念 frontmatter 的 `generated.model`）。
    fn model_label(&self) -> String;
}

/// 生产实现：经 `llm-provider` 调用 OpenAI 兼容端点。
#[derive(Clone)]
pub struct ChatLlm {
    client: Arc<ChatClient>,
    model: String,
}

impl ChatLlm {
    pub fn new(client: Arc<ChatClient>, model: impl Into<String>) -> Self {
        Self {
            client,
            model: model.into(),
        }
    }
}

/// 注入到 system 提示末尾的 JSON 输出要求。
///
/// DeepSeek 的 `json_object` 模式要求 prompt 中出现 "json" 字样，故显式注入。
const JSON_PROMPT_HINT: &str = "请以 JSON 格式输出最终结果。";

#[async_trait]
impl MemoryLlm for ChatLlm {
    async fn complete_json(
        &self,
        system: &str,
        user: &str,
        cancel: CancellationToken,
    ) -> Result<LlmJsonOutcome, String> {
        if self.model.trim().is_empty() {
            return Err("记忆编译缺少模型名（MemoryConfig.model 与默认模型均为空）".into());
        }
        // 强制 JSON 输出模式；并把含 "json" 的要求追加到 system 末尾
        let system_with_hint = format!("{system}\n\n{JSON_PROMPT_HINT}");
        let request = ChatRequest::new(vec![ChatMessage::system(&system_with_hint), ChatMessage::user(user)])
            .with_model(&self.model)
            .with_response_format(json!({ "type": "json_object" }));

        let mut stream = self
            .client
            .chat_stream(&self.model, request, cancel.child_token())
            .await
            .map_err(|e| format!("LLM 请求失败: {e}"))?;

        let mut content = String::new();
        let mut usage = TokenUsage::default();
        let mut finish_reason = String::new();
        while let Some(item) = stream.next().await {
            let event = item.map_err(|e| format!("LLM 流错误: {e}"))?;
            match event {
                LlmEvent::Chunk(c) => content.push_str(&c.content),
                // 传输中断后自动重开：作废重试前累积的半截文本（否则拼接成乱码）
                LlmEvent::Restart => content.clear(),
                LlmEvent::End(e) => {
                    finish_reason = e.finish_reason;
                    if let Some(u) = e.usage {
                        usage = u;
                    }
                    break;
                }
                LlmEvent::Start | LlmEvent::ReasoningChunk(_) | LlmEvent::ToolCalls(_) => {}
                LlmEvent::Error(msg) => return Err(format!("LLM 服务错误: {msg}")),
            }
        }

        let value = extract_json(&content).ok_or_else(|| {
            let shown: String = content.chars().take(200).collect();
            format!("LLM 输出不是合法 JSON: {shown}")
        })?;
        Ok(LlmJsonOutcome {
            value,
            usage,
            finish_reason,
        })
    }

    fn model_label(&self) -> String {
        self.model.clone()
    }
}

/// 记录的提示词（`--dry-run` 输出与调试用）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedPrompt {
    pub stage: PromptStage,
    pub system: String,
    pub user: String,
}

/// 离线替身：**不发起任何请求**，只记录提示词并返回空对象。
///
/// 用于 `okf-agent compile --dry-run`：确定性阶段照跑（归一化 / 增量 / 落盘），
/// LLM 阶段返回空 ⇒ 不产出概念，但提示词被完整捕获可供审阅。
#[derive(Default)]
pub struct DryRunLlm {
    prompts: Mutex<Vec<RecordedPrompt>>,
}

impl DryRunLlm {
    pub fn new() -> Self {
        Self::default()
    }

    /// 取回已记录的提示词（按调用顺序）。
    pub fn prompts(&self) -> Vec<RecordedPrompt> {
        self.prompts.lock().map(|p| p.clone()).unwrap_or_default()
    }

    /// 已记录条数。
    pub fn len(&self) -> usize {
        self.prompts.lock().map(|p| p.len()).unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[async_trait]
impl MemoryLlm for DryRunLlm {
    async fn complete_json(
        &self,
        system: &str,
        user: &str,
        _cancel: CancellationToken,
    ) -> Result<LlmJsonOutcome, String> {
        let stage = detect_stage(system);
        if let Ok(mut prompts) = self.prompts.lock() {
            prompts.push(RecordedPrompt {
                stage,
                system: system.to_string(),
                user: user.to_string(),
            });
        }
        Ok(LlmJsonOutcome::empty())
    }

    fn model_label(&self) -> String {
        "dry-run".into()
    }
}

/// 脚本替身：按**阶段**返回预置 JSON，并记录全部提示词；不发起任何请求。
///
/// 顺序无关（按 [`detect_stage`] 分派），故可安全用于幂等回归：第二次运行时
/// 提取/编译阶段返回不同（或相同）预置值，均可断言「全部 SKIP」。
#[derive(Default)]
pub struct ScriptedLlm {
    responses: Mutex<HashMap<PromptStage, Vec<Value>>>,
    prompts: Mutex<Vec<RecordedPrompt>>,
    /// 是否在缺预置响应时报错（默认 true，避免静默返回空导致假通过）。
    strict: bool,
}

impl ScriptedLlm {
    pub fn new() -> Self {
        Self {
            strict: true,
            ..Default::default()
        }
    }

    /// 宽松模式：缺预置响应时返回空对象而非报错。
    pub fn lenient() -> Self {
        Self::default()
    }

    /// 为某阶段排队一个响应（同阶段多次调用按排队顺序消费，耗尽后复用最后一个）。
    ///
    /// 返回 `&Self` 便于在已有绑定上链式调用（如 `llm.push(a, x).push(b, y);`）。
    /// 若要在 `let` 中直接构建，请用 [`Self::with`]（否则会绑定到临时值的引用）。
    pub fn push(&self, stage: PromptStage, value: Value) -> &Self {
        if let Ok(mut responses) = self.responses.lock() {
            responses.entry(stage).or_default().push(value);
        }
        self
    }

    /// by-value 构建器：`ScriptedLlm::new().with(A, x).with(B, y)`。
    ///
    /// 与 [`Self::push`] 的区别是消费并返回 `Self`，故可安全地直接绑定到变量
    /// （`push` 返回引用，`let x = ..push(..)` 会得到 `&ScriptedLlm`）。
    pub fn with(self, stage: PromptStage, value: Value) -> Self {
        self.push(stage, value);
        self
    }

    /// 取回已记录的提示词。
    pub fn prompts(&self) -> Vec<RecordedPrompt> {
        self.prompts.lock().map(|p| p.clone()).unwrap_or_default()
    }

    /// 某阶段被调用的次数。
    pub fn calls(&self, stage: PromptStage) -> usize {
        self.prompts()
            .iter()
            .filter(|p| p.stage == stage)
            .count()
    }
}

#[async_trait]
impl MemoryLlm for ScriptedLlm {
    async fn complete_json(
        &self,
        system: &str,
        user: &str,
        _cancel: CancellationToken,
    ) -> Result<LlmJsonOutcome, String> {
        let stage = detect_stage(system);
        if let Ok(mut prompts) = self.prompts.lock() {
            prompts.push(RecordedPrompt {
                stage,
                system: system.to_string(),
                user: user.to_string(),
            });
        }
        let mut responses = self.responses.lock().map_err(|_| "脚本响应锁中毒".to_string())?;
        let queue = responses.entry(stage).or_default();
        if queue.is_empty() {
            if self.strict {
                return Err(format!("脚本 LLM 未为阶段 {stage:?} 预置响应"));
            }
            return Ok(LlmJsonOutcome::empty());
        }
        // 排队的响应按序消费；多于一个时依次弹出，最后一个用完后保留复用
        let value = if queue.len() > 1 {
            queue.remove(0)
        } else {
            queue[0].clone()
        };
        Ok(LlmJsonOutcome {
            value,
            usage: TokenUsage::new(100, 50),
            finish_reason: "stop".into(),
        })
    }

    fn model_label(&self) -> String {
        "scripted".into()
    }
}

/// 从 LLM 原始输出中提取 JSON：
/// 去 ` ```json ` 围栏后直接解析，失败回退取首 `{` 至末 `}` 的子串。
pub fn extract_json(content: &str) -> Option<Value> {
    let trimmed = content.trim();
    let stripped = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```"))
        .map(|s| s.trim_end_matches("```").trim())
        .unwrap_or(trimmed);

    serde_json::from_str(stripped).ok().or_else(|| {
        // 仅数组/对象两种容器；优先对象（本管道的契约都是对象）
        for (open, close) in [('{', '}'), ('[', ']')] {
            if let (Some(start), Some(end)) = (stripped.find(open), stripped.rfind(close)) {
                if start < end {
                    if let Ok(v) = serde_json::from_str::<Value>(&stripped[start..=end]) {
                        return Some(v);
                    }
                }
            }
        }
        None
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stage_detection_matches_prompt_builders() {
        use crate::prompt;
        use crate::types::CategoryTable;

        let table = CategoryTable::new(crate::config::MemoryConfig::default().categories);
        assert_eq!(detect_stage(&prompt::extract_system(&table)), PromptStage::Extract);
        assert_eq!(
            detect_stage(prompt::EXTRACT_SELF_CHECK_SYSTEM),
            PromptStage::ExtractSelfCheck
        );
        assert_eq!(detect_stage(&prompt::compile_system(&table)), PromptStage::Compile);
        assert_eq!(detect_stage(&prompt::judge_system()), PromptStage::Judge);
        assert_eq!(detect_stage("你是别的助手"), PromptStage::Unknown);
    }

    #[test]
    fn extract_json_handles_fences_and_noise() {
        assert_eq!(extract_json(r#"{"a":1}"#), Some(json!({"a": 1})));
        assert_eq!(extract_json("```json\n{\"a\":1}\n```"), Some(json!({"a": 1})));
        assert_eq!(extract_json("好的：{\"a\":1} 以上。"), Some(json!({"a": 1})));
        assert_eq!(extract_json("```\n[1,2]\n```"), Some(json!([1, 2])));
        assert_eq!(extract_json("完全不是 JSON"), None);
        assert_eq!(extract_json(""), None);
    }

    #[tokio::test]
    async fn dry_run_records_prompts_without_requesting() {
        let llm = DryRunLlm::new();
        assert!(llm.is_empty());
        let out = llm
            .complete_json(
                "You are an information extraction engine.",
                "原文",
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(out.value, json!({}), "dry-run 返回空对象");
        assert_eq!(out.finish_reason, "offline");
        assert_eq!(llm.len(), 1);
        let recorded = &llm.prompts()[0];
        assert_eq!(recorded.stage, PromptStage::Extract);
        assert_eq!(recorded.user, "原文");
        assert_eq!(llm.model_label(), "dry-run");
    }

    #[tokio::test]
    async fn scripted_llm_dispatches_by_stage_not_order() {
        let llm = ScriptedLlm::new();
        llm.push(PromptStage::Judge, json!({"decisions": []}));
        llm.push(PromptStage::Extract, json!({"events": [{"event_text": "x"}]}));

        // 故意先调「判定」再调「提取」，验证按阶段分派而非按顺序
        let judge = llm.complete_json("memory consolidation judge", "u", CancellationToken::new()).await.unwrap();
        assert_eq!(judge.value, json!({"decisions": []}));
        let extract = llm
            .complete_json("information extraction engine", "u", CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(extract.value["events"][0]["event_text"], "x");
        assert_eq!(extract.usage.total, 150);
        assert_eq!(llm.calls(PromptStage::Extract), 1);
        assert_eq!(llm.calls(PromptStage::Judge), 1);
    }

    #[tokio::test]
    async fn scripted_llm_strict_mode_errors_on_missing_response() {
        let llm = ScriptedLlm::new();
        let err = llm
            .complete_json("OKF Concept Compiler", "u", CancellationToken::new())
            .await
            .unwrap_err();
        assert!(err.contains("Compile"), "缺预置响应应报错而非静默通过: {err}");

        let lenient = ScriptedLlm::lenient();
        let ok = lenient
            .complete_json("OKF Concept Compiler", "u", CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(ok.value, json!({}));
    }

    #[tokio::test]
    async fn scripted_llm_consumes_queued_responses_in_order_then_repeats_last() {
        let llm = ScriptedLlm::new();
        llm.push(PromptStage::Extract, json!({"n": 1}));
        llm.push(PromptStage::Extract, json!({"n": 2}));
        for expected in [1, 2, 2] {
            let out = llm
                .complete_json("information extraction engine", "u", CancellationToken::new())
                .await
                .unwrap();
            assert_eq!(out.value["n"], expected);
        }
    }

    #[tokio::test]
    async fn chat_llm_rejects_empty_model() {
        let client = Arc::new(ChatClient::new("http://127.0.0.1:1/v1", "k"));
        let llm = ChatLlm::new(client, "   ");
        let err = llm
            .complete_json("sys", "user", CancellationToken::new())
            .await
            .unwrap_err();
        assert!(err.contains("模型名"), "{err}");
    }
}
