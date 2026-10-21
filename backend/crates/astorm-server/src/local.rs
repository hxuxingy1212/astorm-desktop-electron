//! 服务端本地状态：设置 / 供应商配置 / 约定文件的持久化（data 目录 JSON 文件）。
//!
//! 桌面版这些在 Electron 主进程（settings.json / providers.json + safeStorage）。
//! 服务端形态：明文 JSON 落 `<data>/config/`（容器卷权限即边界；providers 的 apiKey
//! 支持环境变量占位 `${VAR}`，加载时展开）。任何保存动作后重建 AppConfig 并
//! `update_config` 热更 core（LLM 注册表 / 安全配置 / 摘要阈值等下一轮即生效）。
//!
//! settings.json 的形状与桌面版 `backend.ts get/save_settings` 完全对齐（聚合：
//! locale/summary/compact/goal/webSearch/workspace/security/memory/instructions，
//! 各设置页「读全量→改自己段→写全量」）。**summary/compact/goal 的存储键是
//! snake_case**（桌面 mapSummary/mapCompact/mapGoal 在推送时才转 camel），sync 时
//! 在本模块做同样的映射；security/memory/instructions/webSearch 存储即 camelCase
//! 后端形状，直接透传。

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use astorm_core::app::{App, AppConfig};
use serde_json::{json, Value};

/// 服务端本地文件存储（懒建目录）。
pub struct LocalStore {
    config_dir: Mutex<Option<PathBuf>>,
}

impl LocalStore {
    pub fn new() -> Arc<Self> {
        Arc::new(Self { config_dir: Mutex::new(None) })
    }

    fn dir(&self) -> PathBuf {
        let mut guard = self.config_dir.lock().unwrap_or_else(|p| p.into_inner());
        if guard.is_none() {
            let dir = App::try_global()
                .map(|app| app.dir().config_dir)
                .unwrap_or_else(std::env::temp_dir);
            let _ = std::fs::create_dir_all(&dir);
            *guard = Some(dir);
        }
        guard.clone().unwrap()
    }

    /// 读 JSON 文件（缺失/损坏 → Null）。
    pub fn read(&self, name: &str) -> Value {
        let path = self.dir().join(name);
        match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text).unwrap_or(Value::Null),
            Err(_) => Value::Null,
        }
    }

    /// 写 JSON 文件（pretty）。
    pub fn write(&self, name: &str, value: &Value) -> Result<(), String> {
        let path = self.dir().join(name);
        let text = serde_json::to_string_pretty(value).map_err(|e| e.to_string())?;
        astorm_common::fsx::write_atomic(&path, text.as_bytes())
            .map_err(|e| format!("写入 {} 失败: {e}", path.display()))
    }

    /// 会话工作区目录内的会话附件基目录（web 上传附件的 staging 落点）。
    pub fn attachment_dir(&self) -> PathBuf {
        let dir = App::try_global()
            .map(|app| app.dir().data_dir.join("attachments").join("staging"))
            .unwrap_or_else(std::env::temp_dir);
        let _ = std::fs::create_dir_all(&dir);
        dir
    }
}

/* ================= settings 聚合（与桌面 get_settings 对齐） ================= */

/// get_settings：文件缺失/缺段时用默认值补齐（各设置页「读全量→改自己段→写全量」，
/// 缺段会导致别处保存时被静默抹掉——桌面同款语义）。
pub fn settings_aggregate(store: &LocalStore) -> Value {
    let mut out = store.read("settings.json");
    if !out.is_object() {
        out = json!({});
    }
    let cfg = App::try_global().map(|app| app.config().clone());
    let mut default = |key: &str, v: Value| {
        if out.get(key).map(|v| v.is_null()).unwrap_or(true) {
            out[key] = v;
        }
    };
    default("locale", json!(cfg.as_ref().map(|c| c.locale.as_str()).unwrap_or("zh-CN")));
    // summary/compact/goal 存储键为 snake_case（默认值与桌面 DEFAULT_* 一致）
    default(
        "summary",
        json!({
            "token_threshold": 200000,
            "min_messages": 8,
            "keep_token_threshold": 20000,
            "keep_tail_rounds": 5,
        }),
    );
    default(
        "compact",
        json!({
            "compact_threshold": 32768,
            "compact_recent_rounds": 5,
            "compact_byte_count": 10240,
            "compact_head_chars": 200,
            "compact_tail_chars": 400,
        }),
    );
    default("goal", json!({ "max_tokens": 10000000, "max_iterations": 5 }));
    default("webSearch", json!({ "defaultEngine": "baidu", "timeoutSec": 30 }));
    default("workspace", json!(cfg.as_ref().map(|c| c.workspace.clone()).unwrap_or_default()));
    default(
        "security",
        serde_json::to_value(astorm_core::app::config::SecurityConfig::default()).unwrap_or(json!({})),
    );
    // memory 段：桌面直接存后端 camel 形状；默认开关开
    default("memory", json!({ "enabled": true }));
    default(
        "instructions",
        json!({ "injectGlobalAgents": true, "injectWorkspaceAgents": true }),
    );
    out
}

/// save_settings：`args.settings` 整体覆盖（与桌面同语义），保存后热更。
pub fn save_settings(store: &LocalStore, settings: &Value) -> Result<(), String> {
    store.write("settings.json", settings)?;
    sync_config_to_core(store)
}

/* ================= 配置同步（复刻桌面 buildFullNapiConfig 映射） ================= */

/// summary（snake 存储）→ AppConfig camel 形状。
fn map_summary(v: &Value) -> Value {
    json!({
        "tokenThreshold": v.get("token_threshold").cloned().unwrap_or(json!(200000)),
        "minMessages": v.get("min_messages").cloned().unwrap_or(json!(8)),
        "keepTokenThreshold": v.get("keep_token_threshold").cloned().unwrap_or(json!(20000)),
        "keepTailRounds": v.get("keep_tail_rounds").cloned().unwrap_or(json!(5)),
    })
}

/// compact（snake 存储）→ camel。
fn map_compact(v: &Value) -> Value {
    json!({
        "compactThreshold": v.get("compact_threshold").cloned().unwrap_or(json!(32768)),
        "compactRecentRounds": v.get("compact_recent_rounds").cloned().unwrap_or(json!(5)),
        "compactByteCount": v.get("compact_byte_count").cloned().unwrap_or(json!(10240)),
        "compactHeadChars": v.get("compact_head_chars").cloned().unwrap_or(json!(200)),
        "compactTailChars": v.get("compact_tail_chars").cloned().unwrap_or(json!(400)),
    })
}

/// goal（snake 存储）→ camel。
fn map_goal(v: &Value) -> Value {
    json!({
        "maxTokens": v.get("max_tokens").cloned().unwrap_or(json!(10000000)),
        "maxIterations": v.get("max_iterations").cloned().unwrap_or(json!(5)),
    })
}

/// 展开 providers apiKey 里的 `${ENV}` 占位，并把 UI DTO 的 models（{name,order}）
/// 映射为 AppConfig 形状（{id,name}，id 即模型名——与桌面 providers.ts:124 一致）。
/// pub(crate)：providers/test 端点也要走同一转换（否则 AppConfig 反序列化失败）。
pub(crate) fn expand_providers(providers: &mut Value) {
    let Some(arr) = providers.get_mut("providers").and_then(|p| p.as_array_mut()) else {
        return;
    };
    for p in arr {
        if let Some(key) = p.get("apiKey").and_then(|k| k.as_str()) {
            if let Some(var) = key.strip_prefix("${").and_then(|k| k.strip_suffix('}')) {
                if let Ok(real) = std::env::var(var) {
                    p["apiKey"] = json!(real);
                }
            }
        }
        if let Some(models) = p.get_mut("models").and_then(|m| m.as_array_mut()) {
            for m in models {
                if let Some(name) = m.get("name").and_then(|n| n.as_str()) {
                    m["id"] = json!(name);
                }
            }
        }
    }
}

/// 由 settings.json（聚合形状）+ providers.json 重建 AppConfig 并热更 core。
pub fn sync_config_to_core(store: &LocalStore) -> Result<(), String> {
    let settings = settings_aggregate(store);
    let mut value = json!({});

    // 各段映射/透传（webSearch/security/memory/instructions 存储即后端形状）
    value["summary"] = map_summary(settings.get("summary").unwrap_or(&json!({})));
    value["compact"] = map_compact(settings.get("compact").unwrap_or(&json!({})));
    value["goal"] = map_goal(settings.get("goal").unwrap_or(&json!({})));
    if let Some(ws) = settings.get("webSearch") {
        value["webSearch"] = ws.clone();
    }
    if let Some(sec) = settings.get("security") {
        value["security"] = sec.clone();
    }
    if let Some(mem) = settings.get("memory") {
        value["memory"] = mem.clone();
    }
    if let Some(ins) = settings.get("instructions") {
        value["instructions"] = ins.clone();
    }
    if let Some(locale) = settings.get("locale") {
        value["locale"] = locale.clone();
    }
    // 工作空间根（用户目录）：设置页显式保存的值优先；否则保留**当前运行配置**
    // 的值（启动时已按 --workspace / 数据根解析，见 config::build_app_config）
    match settings.get("workspace").and_then(|w| w.as_str()) {
        Some(ws) if !ws.is_empty() => value["workspace"] = json!(ws),
        _ => {
            if let Some(app) = App::try_global() {
                value["workspace"] = json!(app.config().workspace);
            }
        }
    }

    // LLM：providers.json 非空列表（用户显式编辑）优先；否则保留**当前运行配置**
    // 的 llm（serde(default) 会让缺失键落回空配置——update_config 是整对象替换，
    // 不回填就等于清空。App.vue 的 models 深度 watcher 启动即 persist 空列表，
    // 若照单全收会把 env 注入清成 providers=0——实测踩坑）。
    let mut providers = store.read("providers.json");
    let non_empty = providers
        .get("providers")
        .and_then(|p| p.as_array())
        .map(|a| !a.is_empty())
        .unwrap_or(false);
    if non_empty {
        expand_providers(&mut providers);
        value["llm"] = providers;
    } else if let Some(app) = App::try_global() {
        let cfg = &app.config().llm;
        if !cfg.providers.is_empty() {
            value["llm"] = json!({
                "providers": cfg.providers.iter().map(|p| json!({
                    "name": p.name,
                    "baseUrl": p.base_url,
                    "apiKey": p.api_key,
                    "apiFormat": p.api_format,
                    "models": p.models.iter().map(|m| json!({"id": m.id, "name": m.name})).collect::<Vec<_>>(),
                })).collect::<Vec<_>>(),
                "defaultProvider": cfg.default_provider,
                "defaultModel": cfg.default_model,
            });
        }
    }

    // webRender：env 注入（与容器镜像约定一致）
    if let (Ok(bin), Ok(script)) = (
        std::env::var("ASTORM_WEB_RENDER_BIN"),
        std::env::var("ASTORM_WEB_RENDER_SCRIPT"),
    ) {
        value["webRender"] = json!({ "bin": bin, "script": script });
    }

    // office：bin 目录 + 内置 skill 目录（同 webRender——热更是整对象替换，
    // 不回填会在设置保存后被清掉）
    if let Some(dir) = std::env::var("ASTORM_OFFICE_BIN_DIR").ok().filter(|s| !s.is_empty()) {
        value["office"]["binDir"] = json!(dir);
    }
    if let Some(dir) = std::env::var("ASTORM_OFFICE_SKILL_DIR").ok().filter(|s| !s.is_empty()) {
        value["office"]["skillDir"] = json!(dir);
    }

    let config: AppConfig = serde_json::from_value(value).map_err(|e| format!("重建应用配置失败: {e}"))?;
    App::update_config(config)
}

/// 生效的 LLM 供应商摘要（UI DTO 形状：models 为 `{name, order}`，另补 `id`）。
/// **providers.json 非空优先**——Web 设置页写的就是真源；空/缺失才回退运行配置
/// （env 预置）。`/providers` 与 `/status` 共用，避免「保存后 status 仍报旧值」。
pub fn effective_providers(store: &LocalStore, cfg: &AppConfig) -> Value {
    let mut file = store.read("providers.json");
    let non_empty = file
        .get("providers")
        .and_then(|p| p.as_array())
        .map(|a| !a.is_empty())
        .unwrap_or(false);
    if non_empty {
        expand_providers(&mut file);
        // 默认供应商 = 列表第一项（与 UI「列表第一项即默认」一致），默认模型 = 其第一个模型
        let first = file
            .get("providers")
            .and_then(|p| p.as_array())
            .and_then(|a| a.first());
        let dp = first.and_then(|p| p.get("name")).and_then(|n| n.as_str());
        let dm = first
            .and_then(|p| p.get("models"))
            .and_then(|m| m.as_array())
            .and_then(|a| a.first())
            .and_then(|m| m.get("name"))
            .and_then(|n| n.as_str());
        return json!({
            "providers": file.get("providers").cloned().unwrap_or(json!([])),
            "defaultProvider": dp,
            "defaultModel": dm,
        });
    }
    // 配置回退：默认模型 = default_model，缺省取默认供应商（无则第一项）的第一个模型
    let fallback_model = cfg.llm.default_model.clone().or_else(|| {
        cfg.llm
            .default_provider
            .as_deref()
            .and_then(|name| cfg.llm.providers.iter().find(|p| p.name == name))
            .or_else(|| cfg.llm.providers.first())
            .and_then(|p| p.models.first().map(|m| m.id.clone()))
    });
    json!({
        "providers": cfg.llm.providers.iter().map(|p| json!({
            "name": p.name,
            "baseUrl": p.base_url,
            "apiKey": p.api_key,
            "apiFormat": p.api_format,
            "order": 0,
            "models": p.models.iter().enumerate().map(|(i, m)| json!({"id": m.id, "name": m.name, "order": i})).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
        "defaultProvider": cfg.llm.default_provider,
        "defaultModel": fallback_model,
    })
}

/// 生效的默认模型引用 id：providers.json 第一供应商第一模型优先（UI「列表第一项即
/// 默认」），空则回退启动配置的 `default_model` / `defaultProvider` 第一模型。
pub fn effective_default_model(store: &LocalStore, cfg: &AppConfig) -> Option<String> {
    let summary = effective_providers(store, cfg);
    summary
        .get("defaultModel")
        .and_then(|m| m.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// 启动种子：providers.json 缺失且 env 注入了 LLM 供应商时，把它们物化成
/// providers.json（UI DTO 形状：models 用 {name, order}，与渲染层 fromDto 对齐）。
pub fn seed_providers(store: &LocalStore, config: &AppConfig) {
    if store.read("providers.json").is_null() && !config.llm.providers.is_empty() {
        let providers: Vec<Value> = config
            .llm
            .providers
            .iter()
            .map(|p| {
                json!({
                    "name": p.name,
                    "baseUrl": p.base_url,
                    "apiKey": p.api_key,
                    "apiFormat": p.api_format,
                    "order": 0,
                    "models": p.models.iter().enumerate().map(|(i, m)| json!({
                        "name": m.name, "order": i,
                    })).collect::<Vec<_>>(),
                })
            })
            .collect();
        let _ = store.write(
            "providers.json",
            &json!({
                "providers": providers,
                "defaultProvider": config.llm.default_provider,
                "defaultModel": config.llm.default_model,
            }),
        );
    }
}

/// 用户目录全局 AGENTS.md 路径（`<workspace>/AGENTS.md`，与桌面 settings-sync 一致）。
fn global_agents_path() -> PathBuf {
    App::try_global()
        .map(|app| {
            let ws = &app.config().workspace;
            let p = std::path::Path::new(ws);
            if p.is_absolute() {
                p.join("AGENTS.md")
            } else {
                app.dir()
                    .data_dir
                    .parent()
                    .unwrap_or(&app.dir().data_dir)
                    .to_path_buf()
                    .join(ws)
                    .join("AGENTS.md")
            }
        })
        .unwrap_or_else(|| std::env::temp_dir().join("AGENTS.md"))
}

/// 读全局 AGENTS.md（缺失返回空串）。
pub fn read_global_agents() -> String {
    std::fs::read_to_string(global_agents_path()).unwrap_or_default()
}

/// 写全局 AGENTS.md（自动建父目录）。
pub fn write_global_agents(content: &str) -> Result<(), String> {
    let path = global_agents_path();
    astorm_common::fsx::write_atomic(&path, content.as_bytes())
        .map_err(|e| format!("写入 AGENTS.md 失败: {e}"))
}
