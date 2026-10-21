//! 多供应商 LLM 客户端注册表：供应商名 → OpenAI 兼容客户端 + 模型表。
//!
//! 会话请求经它按（`provider`, `model`）路由：`provider` 决定客户端（缺省回退默认
//! 供应商），`model`（引用 id）在供应商模型表里解析为 API 标识（`name`）——未匹配
//! 时原样透传（允许自定义模型名直连）。

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use llm_provider::{ChatClient, ClientOption};

use super::config::{LlmConfig, LlmModel};

/// 多供应商 LLM 客户端注册表（进程内按供应商名各持一个 [`ChatClient`]）。
pub struct LlmRegistry {
    /// 供应商名 → OpenAI 兼容客户端（baseUrl + apiKey）
    clients: HashMap<String, Arc<ChatClient>>,
    /// 供应商名 → 模型表（id = 前端引用键，name = API 标识）
    models: HashMap<String, Vec<LlmModel>>,
    /// 默认供应商名（`default_provider` 有效时优先，否则列表第一个）
    default_provider: Option<String>,
    /// 默认模型引用 id（`default_model`，供定时任务等无前端会话请求场景）
    default_model: Option<String>,
}

impl LlmRegistry {
    /// 由配置构建（空供应商列表允许：测试 / 未配置 LLM 场景；会话创建时会校验拒绝）。
    pub fn from_config(cfg: &LlmConfig) -> Self {
        let mut clients = HashMap::new();
        let mut models = HashMap::new();
        for p in &cfg.providers {
            clients.insert(
                p.name.clone(),
                Arc::new(ChatClient::with_option(
                    p.base_url.clone(),
                    p.api_key.clone(),
                    ClientOption::default().with_protocol(p.api_format),
                )),
            );
            models.insert(p.name.clone(), p.models.clone());
        }
        let default_provider = cfg
            .default_provider
            .clone()
            .filter(|n| cfg.providers.iter().any(|p| &p.name == n))
            .or_else(|| cfg.providers.first().map(|p| p.name.clone()));
        Self {
            clients,
            models,
            default_provider,
            default_model: cfg.default_model.clone(),
        }
    }

    /// 供应商名列表（启动日志 / 观测）。
    pub fn provider_names(&self) -> Vec<String> {
        self.clients.keys().cloned().collect()
    }

    /// 供应商是否存在。
    pub fn has_provider(&self, name: &str) -> bool {
        self.clients.contains_key(name)
    }

    /// 是否未配置任何供应商。
    pub fn is_empty(&self) -> bool {
        self.clients.is_empty()
    }

    /// 按供应商名取客户端；`None` = 默认供应商。
    pub fn client_for(&self, provider: Option<&str>) -> Option<Arc<ChatClient>> {
        let name = provider.or(self.default_provider.as_deref())?;
        self.clients.get(name).cloned()
    }

    /// 默认客户端（默认供应商；无供应商时 None）。
    pub fn default_client(&self) -> Option<Arc<ChatClient>> {
        self.default_provider.as_deref().and_then(|n| self.clients.get(n).cloned())
    }

    /// 解析模型 API 标识：供应商模型表先按 `id == model_ref` 匹配取 `name`，
    /// 再按 `name` 精确命中（同值）；都未命中则原样透传（允许自定义模型名）。
    pub fn resolve_model_name(&self, provider: Option<&str>, model_ref: &str) -> String {
        let name = provider.or(self.default_provider.as_deref());
        if let Some(models) = name.and_then(|n| self.models.get(n)) {
            if let Some(m) = models.iter().find(|m| m.id == model_ref) {
                return m.name.clone();
            }
            if let Some(m) = models.iter().find(|m| m.name == model_ref) {
                return m.name.clone();
            }
        }
        model_ref.to_string()
    }

    /// 默认模型引用 id（`default_model` 或默认供应商第一模型 id；无则 None）。
    pub fn default_model(&self) -> Option<String> {
        if let Some(m) = &self.default_model {
            return Some(m.clone());
        }
        let name = self.default_provider.as_deref()?;
        self.models.get(name)?.first().map(|m| m.id.clone())
    }
}

/// 可热更新的 LLM 注册表句柄：会话持有的是句柄（克隆），LLM 配置热更新时
/// 整体替换内部注册表（[`LlmRegistryHandle::swap`]），进行中会话的下一轮推理
/// 立即读到新配置，无需 shutdown（不中断会话、不重开存储）。
#[derive(Clone)]
pub struct LlmRegistryHandle(Arc<RwLock<Arc<LlmRegistry>>>);

impl LlmRegistryHandle {
    pub fn new(registry: LlmRegistry) -> Self {
        Self(Arc::new(RwLock::new(Arc::new(registry))))
    }

    /// 快照：读锁 + 克隆（会话创建校验 / 每轮请求构建时调用，临界区极短）。
    pub(crate) fn snapshot(&self) -> Arc<LlmRegistry> {
        self.0.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// 热更新：整体替换注册表（新客户端 + 新模型表 + 默认供应商）。
    pub fn swap(&self, registry: LlmRegistry) {
        *self.0.write().unwrap_or_else(|e| e.into_inner()) = Arc::new(registry);
    }

    // ---- 透传只读查询（签名与 LlmRegistry 一致，调用侧无需改动） ----
    pub fn client_for(&self, provider: Option<&str>) -> Option<Arc<ChatClient>> {
        self.snapshot().client_for(provider)
    }
    pub fn default_client(&self) -> Option<Arc<ChatClient>> {
        self.snapshot().default_client()
    }
    pub fn resolve_model_name(&self, provider: Option<&str>, model_ref: &str) -> String {
        self.snapshot().resolve_model_name(provider, model_ref)
    }
    pub fn default_model(&self) -> Option<String> {
        self.snapshot().default_model()
    }
    pub fn is_empty(&self) -> bool {
        self.snapshot().is_empty()
    }
    pub fn has_provider(&self, name: &str) -> bool {
        self.snapshot().has_provider(name)
    }
    pub fn provider_names(&self) -> Vec<String> {
        self.snapshot().provider_names()
    }
}

#[cfg(test)]
mod tests {
    use crate::app::LlmProvider;
    use super::*;

    fn cfg() -> LlmConfig {
        LlmConfig {
            providers: vec![
                LlmProvider {
                    name: "openai".into(),
                    base_url: "https://api.openai.com/v1".into(),
                    api_key: "k1".into(),
                    api_format: Default::default(),
                    models: vec![
                        LlmModel { id: "m1ab2c3".into(), name: "gpt-4o".into() },
                        LlmModel { id: "m4d5e6f7".into(), name: "gpt-4o-mini".into() },
                    ],
                },
                LlmProvider {
                    name: "deepseek".into(),
                    base_url: "https://api.deepseek.com".into(),
                    api_key: "k2".into(),
                    api_format: Default::default(),
                    models: vec![LlmModel { id: "ds".into(), name: "deepseek-chat".into() }],
                },
            ],
            default_provider: None,
            default_model: None,
        }
    }

    #[test]
    fn routes_client_by_provider_with_default_fallback() {
        let reg = LlmRegistry::from_config(&cfg());
        assert!(reg.has_provider("openai"));
        assert!(reg.has_provider("deepseek"));
        assert!(!reg.is_empty());
        assert!(reg.client_for(None).is_some(), "None 应回落默认供应商");
        assert!(reg.client_for(Some("deepseek")).is_some());
        assert!(reg.client_for(Some("nope")).is_none(), "未知供应商应返回 None（调用侧校验拒绝）");
        let names = reg.provider_names();
        assert!(names.iter().any(|n| n == "openai") && names.iter().any(|n| n == "deepseek"));
    }

    #[test]
    fn resolves_model_id_to_api_name_scoped_by_provider() {
        let reg = LlmRegistry::from_config(&cfg());
        // 引用 id → API 标识（name）
        assert_eq!(reg.resolve_model_name(Some("openai"), "m1ab2c3"), "gpt-4o");
        // 默认供应商同样解析
        assert_eq!(reg.resolve_model_name(None, "m4d5e6f7"), "gpt-4o-mini");
        // 已是 API 标识 → 原样
        assert_eq!(reg.resolve_model_name(Some("openai"), "gpt-4o"), "gpt-4o");
        // 未匹配 → 原样透传（允许自定义模型名直连）
        assert_eq!(reg.resolve_model_name(Some("openai"), "custom-model"), "custom-model");
        // 供应商隔离：`ds` 只属于 deepseek，在 openai 下未匹配 → 透传
        assert_eq!(reg.resolve_model_name(Some("openai"), "ds"), "ds");
        assert_eq!(reg.resolve_model_name(Some("deepseek"), "ds"), "deepseek-chat");
    }

    #[test]
    fn default_model_prefers_config_then_first_provider() {
        // 缺省 → 默认供应商（第一个）第一模型 id
        assert_eq!(LlmRegistry::from_config(&cfg()).default_model().as_deref(), Some("m1ab2c3"));
        // 显式 default_model 优先
        let mut c = cfg();
        c.default_model = Some("ds".into());
        assert_eq!(LlmRegistry::from_config(&c).default_model().as_deref(), Some("ds"));
        // 空供应商 → 全空
        let empty = LlmRegistry::from_config(&LlmConfig::default());
        assert!(empty.is_empty());
        assert!(empty.default_model().is_none());
        assert!(empty.default_client().is_none());
    }
}