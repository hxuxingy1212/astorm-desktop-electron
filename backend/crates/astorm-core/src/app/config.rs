//! 应用配置：前端以 JSON 传入，应用启动时解析并全局持有。
//!
//! 路径类字段（`workspace` / `log.dir`）由前端管理：`start` 时随 JSON 一起传入，
//! 后端解析后存入全局单例，启动即生效。

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

// 记忆管道的配置契约由 `astorm-memory` 拥有（模块解耦：core 只做应用集成，不实现记忆逻辑）。
// 此处再导出，使 `crate::app::config::MemoryConfig` 的既有引用保持有效。
pub use astorm_memory::config::{MemoryCategory, MemoryConfig};

/// 应用配置（前端 `start` 传入 JSON；缺失字段回退默认值）。
/// 普通会话的模型由每条会话请求（`CreateSessionRequest.model`，模型**引用 id**）携带，
/// 并可选携带 `provider`（供应商名）选择供应商；仅定时任务等无前端请求的场景
/// 使用 `llm.default_model` 默认值（可选）。
#[derive(Debug, Clone, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AppConfig {
    /// 日志配置（级别 + 目录路径）
    pub log: LogConfig,
    /// 默认工作目录路径（会话未指定 workspace 时使用）
    pub workspace: String,
    /// LLM 供应商配置（多供应商 × 多模型）
    pub llm: LlmConfig,
    /// 会话摘要配置（上下文压缩阈值）
    pub summary: SummaryConfig,
    /// 会话消息剪支配置（摘要前对超长历史工具参数/结果做截断归档）
    pub compact: CompactConfig,
    /// 目标循环配置（`/goal` 外循环的 token / 迭代预算）
    pub goal: GoalConfig,
    /// 页面渲染服务配置（websearch / webfetch 共用：复用内置 Chromium 的隐藏窗口渲染）
    pub web_render: WebRenderConfig,
    /// `browser` 工具配置（agent-browser 可执行路径；由主进程按开发/打包填绝对路径）
    pub browser: BrowserConfig,
    /// `office` 工具 + 内置 office skill 配置（astorm-office 套件；主进程按开发/打包填绝对路径）
    pub office: OfficeConfig,
    /// `search`(websearch) 工具配置（默认引擎 + 超时；多引擎免 key 抓取，失败自动回退）
    pub web_search: WebSearchConfig,
    /// 安全配置（沙箱防护策略：文件读写黑白名单 / 网络名单 / 危险命令 / 资源限制）
    pub security: SecurityConfig,
    /// 长期记忆配置（OKF 编译管道：开关 / 自动增量 / 分类表 / 各类预算）
    pub memory: MemoryConfig,
    /// 约定文件配置（AGENTS.md 注入开关：全局 + 会话空间）
    pub instructions: InstructionConfig,
    /// 用户语言偏好（前端设置页语言，如 `zh-CN` / `en-US`；会话环境信息展示用）
    pub locale: String,
}

/// 约定文件注入配置（前端设置页「全局约定」编辑，`update_config` 热更）。
///
/// 两份 AGENTS.md 都拼进**同一条 system 消息**（静态 → 全局 → 会话空间 → 智能体指引）：
/// - 全局：`<user_dir>/AGENTS.md`（`user_dir` = 本配置的 `workspace`，默认 `~/astorm`）
/// - 会话空间：`<会话工作区>/AGENTS.md`（[`crate::tool::resolve_workspace`] 推导）
///
/// 路径均由后端按上述约定自行推导，前端不传路径（两侧算法一致，见 `app::paths::user_dir`）。
#[derive(Debug, Clone, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct InstructionConfig {
    /// 注入用户目录下的全局 AGENTS.md（对所有会话生效）
    pub inject_global_agents: bool,
    /// 注入会话工作区根目录的 AGENTS.md（仅对该会话生效，冲突时覆盖全局）
    pub inject_workspace_agents: bool,
}

impl Default for InstructionConfig {
    fn default() -> Self {
        Self {
            inject_global_agents: true,
            inject_workspace_agents: true,
        }
    }
}

/// 安全配置：沙箱防护策略（前端设置页「安全配置」编辑，`update_config` 热更）。
///
/// 语义（叠加而非替换基线）：会话空间 `ws/**` 始终隐式注入读写白名单；此处
/// `file.readWhite/writeWhite` 为**额外放行**的目录（glob），黑名单始终优先于白名单；
/// `net` 为整体替换的网络策略（默认白名单模式 + 全空 = 默认拒绝，转可审批）；
/// `danger.black` 为自定义危险命令正则（与内置清单叠加）；`resource` 直接生效；
/// `toolchainDisabled` 为「本地环境」页禁用注入的工具 bin 名（取消其沙箱白名单自动注入）。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SecurityConfig {
    /// 防护总开关：false = bash 一律无防护直执行（默认 true）
    pub enabled: bool,
    /// 文件访问黑白名单（glob，如 `/Users/me/docs/**`）
    pub file: SecurityFileConfig,
    /// 网络访问黑白名单（`domain[:port]` / `ip[:port][/prefix]`）
    pub net: SecurityNetConfig,
    /// 危险命令黑名单（正则）
    pub danger: SecurityDangerConfig,
    /// 资源限制
    pub resource: SecurityResourceConfig,
    /// 已禁用沙箱自动注入的工具 bin 名（如 `["python3","go"]`；
    /// 本地环境页开关维护，沙箱注入层据此过滤 exec/读放行）
    pub toolchain_disabled: Vec<String>,
    /// **额外可执行目录**（如 `["/Library/Developer/Toolchains/swift-5.10.xctoolchain"]`）：
    /// 放行**非常规安装位置**的工具链/编译器——自动探测只覆盖 PATH 上能解析到的工具，
    /// 手动解压的编译器、独立工具链、自定义 SDK 进不了白名单，会以 EPERM 静默失败
    /// （`Operation not permitted`）。exec 与只读一并放行（dyld 要读安装树里的
    /// dylib/资源），见 `astorm_sandbox::toolchain::extra_dirs`。
    pub exec_white: Vec<String>,
}

/// 文件访问黑白名单（额外叠加项；会话空间基线自动注入）。
///
/// `read_black` / `write_black` 的默认值是**凭据路径默认清单**（见 [`credential_paths`]）：
/// 读虽是白名单制，但白名单被写宽（如 `/Users/x`）或会话空间设成家目录时，
/// 凭据文件就没有任何一层拦得住；设置页可见可编辑，**用户清空 = 关闭该防护**。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SecurityFileConfig {
    /// 读黑名单（命中即拒，优先于白名单）
    pub read_black: Vec<String>,
    /// 读额外白名单（会话空间外的放行目录）
    pub read_white: Vec<String>,
    /// 写黑名单
    pub write_black: Vec<String>,
    /// 写额外白名单
    pub write_white: Vec<String>,
}

/// 网络访问黑白名单（整体替换默认策略）。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SecurityNetConfig {
    /// 白名单开关：true = 名单外默认拒绝（默认）；false = 仅黑名单拦截
    pub use_whitelist: bool,
    pub in_black: Vec<String>,
    pub in_white: Vec<String>,
    pub out_black: Vec<String>,
    pub out_white: Vec<String>,
}

/// 危险命令黑名单。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SecurityDangerConfig {
    /// 内置清单（rm -rf /、mkfs、shutdown 等）
    pub use_builtin: bool,
    /// 内置清单的用户编辑副本（None = 跟随代码内置；Some = 替换内置清单，可增删改）
    pub builtin_patterns: Option<Vec<String>>,
    /// 自定义正则（与内置叠加）
    pub black: Vec<String>,
}

/// 资源限制（None = 不限；memory/disk/cpuSeconds 有温和默认值，见 Default）。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SecurityResourceConfig {
    /// 内存上限（MB）
    pub memory_mb: Option<u64>,
    pub disk_mb: Option<u64>,
    pub cpu_seconds: Option<u64>,
    /// CPU 使用率上限（**整机占比**百分比 1-100；macOS 采样节流 / Windows Job 硬上限）
    pub cpu_percent: Option<u8>,
    /// GPU 访问（默认允许；false 时沙箱阻断 iokit/mach GPU 服务）
    pub gpu: bool,
}

/// 页面渲染服务配置（**websearch / webfetch 共用**）：复用内置 Chromium（Electron）的
/// 常驻隐藏窗口渲染网页（支持 JS + stealth 反爬），带并发上限 / 排队 / 节流 / 预热 / 空闲回收。
#[derive(Debug, Clone, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct WebRenderConfig {
    /// Chromium 可执行文件（Electron 自身 `process.execPath`）。
    pub bin: Option<String>,
    /// 渲染服务脚本绝对路径（`page-render.cjs`）。
    pub script: Option<String>,
    /// 独立 userData 目录（避免与主应用/多进程争用 profile 锁；缺省则不覆盖）。
    pub profile_dir: Option<String>,
    /// 并发渲染窗口数（上限）。
    pub concurrency: usize,
    /// 排队上限：等待 + 执行中的请求超过 `concurrency + queue_max` 直接拒绝（快速回退）。
    pub queue_max: usize,
    /// 空闲回收秒数（0 = 常驻不回收）。空闲超时后退出以释放内存，下次请求再拉起。
    pub idle_secs: u64,
    /// 每次渲染后的最小间隔（毫秒），用于对引擎做节流、降低被反爬概率。
    pub min_interval_ms: u64,
}

impl Default for WebRenderConfig {
    fn default() -> Self {
        Self {
            bin: None,
            script: None,
            profile_dir: None,
            concurrency: 2,
            queue_max: 16,
            idle_secs: 300,
            min_interval_ms: 800,
        }
    }
}

/// `browser` 工具配置：经 `agent-browser` CLI 操控内置浏览器当前网页页签（iframe 作用域 CDP 桥）。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct BrowserConfig {
    /// `agent-browser` 可执行文件路径（未配置时回退环境变量 `ASTORM_AGENT_BROWSER_BIN` 与 PATH）。
    pub binary: Option<String>,
}

/// `office` 工具配置：随包分发的 astorm-office 五 CLI（统一入口 astorm-office +
/// json2docx/xlsx/pptx/pdf，Word/Excel/PPT/PDF 生成·编辑·转换）与内置 office skill 的资源目录。
#[derive(Debug, Clone, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct OfficeConfig {
    /// 五 CLI 所在目录（含 `astorm-office` / `json2docx` 等可执行文件；未配置时回退环境变量 `ASTORM_OFFICE_BIN_DIR`）。
    pub bin_dir: Option<String>,
    /// 内置 office skill 资源目录（kit 的 `skill/`，含总纲 SKILL.md 与四格式分册；
    /// 启动时复制进用户技能目录后注册为 builtin 技能，缺失则该技能不注入）。
    pub skill_dir: Option<String>,
    /// `render png` 的 Chromium 可执行文件（转发给 CLI 环境变量 `ASTORM_CHROME_BIN`）。
    pub chrome_bin: Option<String>,
    /// `convert office` 保真引擎的 soffice 路径（转发给 CLI 环境变量 `ASTORM_SOFFICE_PATH`）。
    pub soffice_path: Option<String>,
    /// 单次调用超时（秒；render/convert 大文件较慢，默认 180，上限 600）。
    pub timeout_sec: u64,
}

impl Default for OfficeConfig {
    fn default() -> Self {
        Self {
            bin_dir: None,
            skill_dir: None,
            chrome_bin: None,
            soffice_path: None,
            timeout_sec: 180,
        }
    }
}

/// `search`(websearch) 工具配置：多引擎 HTML 抓取（免 key）。
///
/// 引擎固定为 `baidu` / `sogou` / `bing` / `duckduckgo` / `google`；`default_engine` 为
/// 首选，命中限流 / 网络异常 / 空结果时按**内置顺序**（国内优先）自动切下一个。
#[derive(Debug, Clone, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct WebSearchConfig {
    /// 默认引擎（`baidu` / `sogou` / `bing` / `duckduckgo` / `google`；非法值回退 `baidu`）。
    pub default_engine: String,
    /// 单次请求超时（秒）。
    pub timeout_sec: u64,
}

impl Default for WebSearchConfig {
    fn default() -> Self {
        Self { default_engine: "baidu".into(), timeout_sec: 30 }
    }
}

/// 会话摘要配置（会话循环在达到阈值时对旧历史做 LLM 摘要并注入下一轮）。
#[derive(Debug, Clone, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SummaryConfig {
    /// 触发摘要的会话历史估算 token 阈值（估算口径：`(消息字符数 + 工具参数字符数) / 4`）。
    pub token_threshold: usize,
    /// 触发摘要所需的最少历史条数（避免对极短会话做无谓摘要）。
    pub min_messages: usize,
    /// 保留尾部（新近消息）的 token 预算：从尾部往回累积估算 token，超过该值界定保留范围。
    pub keep_token_threshold: usize,
    /// 保留尾部的轮数：保留最近 N 个 assistant 消息及其后的消息。
    pub keep_tail_rounds: usize,
}

/// 会话消息剪支配置：摘要前对较旧且超长的工具调用参数/结果做截断，原内容归档，便于 LLM 回溯。
#[derive(Debug, Clone, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CompactConfig {
    /// 触发剪支的会话历史估算上下文字符数阈值（纯字符：Σcontent.len + Σarguments.len）。
    pub compact_threshold: usize,
    /// 最近多少轮（assistant 消息）之前才参与剪支（更新近轮完整保留）。
    pub compact_recent_rounds: usize,
    /// 剪支判断：消息字符数超过该值才做初判（默认 10240）。
    pub compact_byte_count: usize,
    /// 剪支后保留的头部字符数（默认 200）。
    pub compact_head_chars: usize,
    /// 剪支后保留的尾部字符数（默认 400）。
    pub compact_tail_chars: usize,
}

/// 目标循环配置（`/goal`）：一次目标运行期间允许消耗的 token 上限与最大续跑迭代次数。
#[derive(Debug, Clone, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct GoalConfig {
    /// 单次目标运行累计消耗 token 上限（含每轮判定调用自身用量）。
    pub max_tokens: u64,
    /// 单次目标运行的最大续跑迭代次数（判定为「继续」并注入用户消息的次数）。
    pub max_iterations: u32,
}

/// 日志配置。
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct LogConfig {
    /// 日志级别（trace / debug / info / warn / error，默认 info）
    pub level: String,
    /// 日志目录路径（绝对路径原样使用；相对路径或未设置时解析到应用根目录下，
    /// 默认 `<数据根>/log`，见 [`crate::app::paths::resolve_log_dir`]）
    pub dir: Option<String>,
}

/// LLM 供应商配置（多供应商 × 多模型；前端 `start` 传入）。
///
/// 前端 JSON 结构（camelCase）：
/// ```json
/// { "llm": { "providers": [
///     { "name": "OpenAI", "baseUrl": "https://api.openai.com/v1", "apiKey": "sk-...",
///       "models": [ { "id": "m1ab2c3", "name": "gpt-4o" } ] }
/// ] } }
/// ```
/// 语义：`model.id` 为前端引用键（会话请求 `model` 字段填写）；`model.name` 为
/// 实际上送 LLM API（`body["model"]`）的标识。
#[derive(Debug, Clone, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct LlmConfig {
    /// 供应商列表
    #[serde(default)]
    pub providers: Vec<LlmProvider>,
    /// 默认供应商名（缺省 = 列表第一个；会话请求未携带 `provider` 时回退）
    #[serde(default)]
    pub default_provider: Option<String>,
    /// 默认模型引用 id（定时任务等无前端会话请求的场景使用；缺省 = 默认供应商第一模型）
    #[serde(default)]
    pub default_model: Option<String>,
}

/// 单个供应商（OpenAI 兼容 / Anthropic / Responses 协议）。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmProvider {
    /// 供应商名（会话请求 `provider` 字段引用；需唯一）
    pub name: String,
    /// API 地址
    #[serde(alias = "base_url")]
    pub base_url: String,
    /// 密钥（OpenAI 系为 Bearer；Anthropic 为 x-api-key）
    #[serde(alias = "api_key")]
    pub api_key: String,
    /// API 协议（wire 格式）：缺省 `chat_completions`（向后兼容）
    #[serde(default)]
    pub api_format: llm_provider::Protocol,
    /// 供应商模型列表
    #[serde(default)]
    pub models: Vec<LlmModel>,
}

/// 单个模型：`id` 为前端引用键，`name` 为上送 LLM API 的模型标识。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmModel {
    /// 模型引用 id（前端选择模型 / 会话请求 `model` 字段填写）
    pub id: String,
    /// API 模型标识（实际上送 chat/completions 的 `model`）
    pub name: String,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            log: LogConfig::default(),
            workspace: "workspace".into(),
            llm: LlmConfig::default(),
            summary: SummaryConfig::default(),
            compact: CompactConfig::default(),
            goal: GoalConfig::default(),
            web_render: WebRenderConfig::default(),
            browser: BrowserConfig::default(),
            office: OfficeConfig::default(),
            web_search: WebSearchConfig::default(),
            security: SecurityConfig::default(),
            memory: MemoryConfig::default(),
            instructions: InstructionConfig::default(),
            locale: "zh-CN".into(),
        }
    }
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            file: SecurityFileConfig::default(),
            net: SecurityNetConfig::default(),
            danger: SecurityDangerConfig::default(),
            resource: SecurityResourceConfig::default(),
            toolchain_disabled: Vec::new(),
            exec_white: Vec::new(),
        }
    }
}

impl Default for SecurityFileConfig {
    fn default() -> Self {
        // 读写黑名单共用同一份凭据清单（黑名单始终优先于白名单；用户清空 = 关闭该防护）
        let credentials = credential_paths();
        Self {
            read_black: credentials.clone(),
            read_white: vec![],
            write_black: credentials,
            write_white: vec![],
        }
    }
}

/// 凭据路径默认清单（`readBlack` / `writeBlack` 的默认值，均为绝对路径）。
///
/// 背景：读是白名单制，但用户把 `file.readWhite` 写宽（如 `/Users/x`）或把工作区
/// 设成家目录时，凭据文件就没有任何一层拦得住——这里预填一份凭据清单（设置页可见
/// 可编辑，**用户清空 = 关闭**）。
///
/// 写法约定（消费者两侧都按绝对路径/glob 处理，见 `astorm_sandbox` 的
/// `sandbox::seatbelt_builder::path_rules` 与 `sandbox::windows::glob_fixed_prefix`）：
/// - 目录树 = `<绝对路径>/**`（含通配符才会展开成 Seatbelt subpath / ACL 固定前缀）；
/// - 单个文件 = 完整绝对路径（Seatbelt 按 literal 精确匹配）；
/// - **不写 `~/...` 字面量**：任何一层都不会展开 `~`，必须在 Rust 侧用 `dirs` 拼绝对路径；
/// - glob 只在确有必要时用（Windows 侧会把通配符前的固定前缀整段 ACL 拒绝）。
///
/// `dirs::home_dir()` 拿不到时（极少数无 HOME 的环境）返回空表：宁可没有默认清单，
/// 也不写错路径污染黑白名单。
pub fn credential_paths() -> Vec<String> {
    credential_paths_for(dirs::home_dir())
}

/// 由 home 目录推导凭据清单；`None` = 取不到 home → 空表（不 panic）。
fn credential_paths_for(home: Option<PathBuf>) -> Vec<String> {
    let Some(home) = home else {
        return Vec::new();
    };
    let mut out = vec![
        // SSH 私钥 / known_hosts / 跳板机配置
        tree(&home, ".ssh"),
        // AWS access key（credentials 文件）与 CLI 配置
        tree(&home, ".aws"),
        // GPG 私钥与信任库
        tree(&home, ".gnupg"),
        // kubeconfig（集群 CA / 客户端证书 / token）
        tree(&home, ".kube"),
        // Docker registry 登录凭据（base64 auth / credsStore）
        file(&home, ".docker/config.json"),
        // git 明文 HTTPS 凭据存储
        file(&home, ".git-credentials"),
        // curl / ftp 的机器登录密码
        file(&home, ".netrc"),
        // npm registry token
        file(&home, ".npmrc"),
        // PyPI 上传 token
        file(&home, ".pypirc"),
        // Terraform Cloud / 私有 registry token
        file(&home, ".terraformrc"),
        // Vault token 明文文件
        file(&home, ".vault-token"),
        // GitHub CLI 的 OAuth token
        tree(&home, ".config/gh"),
        // gcloud 用户凭据（refresh token）
        tree(&home, ".config/gcloud"),
        // Azure CLI 凭据缓存
        tree(&home, ".azure"),
        // 注：`.gitconfig` 刻意不在清单内——用户级 git 配置在本产品里是正常读取项
    ];
    out.extend(platform_credential_paths(&home));
    out
}

/// 平台专属凭据路径（macOS：钥匙串 + Chrome 登录数据；Windows：漫游 AppData 下
/// 的 gh / gcloud 凭据目录，它们不在 `~/.config`）。
#[cfg(target_os = "macos")]
fn platform_credential_paths(home: &Path) -> Vec<String> {
    vec![
        // 系统 / 应用密码的钥匙串数据库
        tree(home, "Library/Keychains"),
        // Chrome 保存的登录密码：多 Profile 需一层 `*`；只拦登录数据文件，
        // 不整目录拦（浏览器数据导入/自动化控制是正常功能）。注意 `*` 只在应用层
        // globset 生效，Seatbelt 的 subpath 不支持通配符（内核层靠应用层兜底）。
        file(home, "Library/Application Support/Google/Chrome/*/Login Data"),
    ]
}

#[cfg(windows)]
fn platform_credential_paths(_home: &Path) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(roaming) = dirs::config_dir() {
        // gh / gcloud 在 Windows 用漫游 AppData（`%APPDATA%\<app>`）
        out.push(tree(&roaming, "GitHub CLI"));
        out.push(tree(&roaming, "gcloud"));
    }
    out
}

#[cfg(not(any(target_os = "macos", windows)))]
fn platform_credential_paths(_home: &Path) -> Vec<String> {
    Vec::new()
}

/// 目录树条目：`<base>/<rel>/**`（含通配符才会被沙箱层展开为 subpath / ACL 前缀）。
fn tree(base: &Path, rel: &str) -> String {
    format!("{}/**", file(base, rel))
}

/// 精确文件条目：`<base>/<rel>`（Seatbelt literal；Windows 同样只拒绝该路径）。
fn file(base: &Path, rel: &str) -> String {
    base.join(rel).to_string_lossy().into_owned()
}

impl Default for SecurityNetConfig {
    fn default() -> Self {
        Self {
            use_whitelist: true,
            in_black: vec![],
            in_white: vec![],
            out_black: vec![],
            out_white: vec![],
        }
    }
}

impl Default for SecurityDangerConfig {
    fn default() -> Self {
        Self {
            use_builtin: true,
            builtin_patterns: None,
            black: vec![],
        }
    }
}

impl Default for SecurityResourceConfig {
    fn default() -> Self {
        // 宽松默认：防跑飞但不拖慢正常任务（整机 CPU 95% 基本只在真跑飞时才介入）；
        // 显式 null（前端「留空」）= 不限
        Self {
            memory_mb: Some(4096),
            disk_mb: Some(1024),
            cpu_seconds: Some(600),
            cpu_percent: Some(95),
            gpu: true,
        }
    }
}

impl Default for LogConfig {
    fn default() -> Self {
        Self {
            level: "info".into(),
            dir: None,
        }
    }
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            providers: Vec::new(),
            default_provider: None,
            default_model: None,
        }
    }
}

impl Default for SummaryConfig {
    fn default() -> Self {
        Self {
            token_threshold: 200_000,
            min_messages: 8,
            keep_token_threshold: 20_000,
            keep_tail_rounds: 5,
        }
    }
}

impl Default for CompactConfig {
    fn default() -> Self {
        Self {
            compact_threshold: 32_768,
            compact_recent_rounds: 5,
            compact_byte_count: 10_240,
            compact_head_chars: 200,
            compact_tail_chars: 400,
        }
    }
}

impl Default for GoalConfig {
    fn default() -> Self {
        Self {
            max_tokens: 10_000_000,
            max_iterations: 5,
        }
    }
}

impl AppConfig {
    /// 从 JSON 字符串解析（缺失字段回退默认值）。
    pub fn from_json(json: &str) -> Result<Self, String> {
        serde_json::from_str(json).map_err(|e| format!("解析应用配置失败: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 清单里是否有 `<home>/<rel>/**` 条目（按 Path 组件比较尾部，兼容 Windows 分隔符）
    fn has_tree(list: &[String], rel: &str) -> bool {
        list.iter()
            .any(|pat| Path::new(pat).parent().is_some_and(|p| p.ends_with(rel)))
    }

    #[test]
    fn default_file_black_lists_are_credential_paths() {
        let file = SecurityFileConfig::default();
        // 读写黑名单默认同一份凭据清单
        assert_eq!(file.read_black, file.write_black);
        if dirs::home_dir().is_some() {
            assert!(has_tree(&file.read_black, ".ssh"), "{:?}", file.read_black);
            assert!(has_tree(&file.read_black, ".aws"), "{:?}", file.read_black);
            for pat in &file.read_black {
                // seatbelt path_rules / Windows ACL 都按绝对路径处理，`~` 不会被展开
                assert!(Path::new(pat).is_absolute(), "非绝对路径: {pat}");
                assert!(!pat.contains('~'), "含 ~ 字面量: {pat}");
            }
            // 用户级 git 配置在本产品里是正常读取项，不得默认拦
            assert!(!file.read_black.iter().any(|p| p.contains(".gitconfig")));
            // 白名单默认仍为空（放行只来自会话空间基线）
            assert!(file.read_white.is_empty() && file.write_white.is_empty());
        } else {
            // 取不到 home：退化为空表而不是 panic
            assert!(file.read_black.is_empty());
        }
    }

    #[test]
    fn credential_paths_without_home_is_empty() {
        assert!(credential_paths_for(None).is_empty());
    }

    #[test]
    fn security_defaults_serialize_as_camel_case() {
        let v = serde_json::to_value(SecurityConfig::default()).expect("默认安全配置可序列化");
        assert!(v["file"]["readBlack"].is_array());
        assert!(v["file"]["writeBlack"].is_array());
        assert_eq!(v["enabled"], serde_json::json!(true));
        assert!(v.get("file").is_some() && v.get("toolchainDisabled").is_some());
    }
}