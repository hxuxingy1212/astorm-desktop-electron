//! `agent-browser` CLI 适配层：拼参数、注入 env、执行、解析 `--json` 信封、归一错误。
//!
//! **升级 agent-browser 只改这一层**（协议细节集中在此）：
//! - 端点来源：主进程写的 `userData/browser-agent.json`（`pkgid` 见 [`read_endpoint`]），
//!   **每次动作前重读** —— 桥随页签切换会换 token/端口，缓存住的 ws 一定是死的；
//! - 会话名 = `astorm-<tabId>-<token 前 8 位>`：一页签一实例；token 进名字后，换桥即换 daemon，
//!   不会复用一条已断开的旧连接（残留 daemon 由 `AGENT_BROWSER_IDLE_TIMEOUT_MS` 20 分钟回收）；
//! - 反注入：`--content-boundaries` 常开（`--json` 下它只是在信封里加 `_boundary.nonce/origin`，
//!   正文仍可正常解析）；`--max-output` 与工具侧截断保持一致；
//! - **只许附着、不许自启**：CLI 的 zero-config 用法（没有 `--cdp`/provider）会**自己拉起一个真实
//!   Chrome 窗口**（实测）；本层一律传 `--executable-path` 指向一个不存在的文件，把那条分支变成
//!   「明确报错」而不是「弹出本地浏览器」——附着路径不受影响（实测：`launched:false` 且命令正常）。
//!   另外再兜一道：信封里出现 `lifecycle.launched=true` 即判失败，避免它换条路又把窗口弹出来。
//! - 超时/取消由本层负责：agent-browser 没有全局 `--timeout`（实测），超时/`ctx.cancel` 直接杀子进程。

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;
use serde_json::Value;
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

use crate::app::App;

/// 单动作默认超时（秒）。
pub const DEFAULT_TIMEOUT_SECS: u64 = 30;
/// 单动作超时上限（秒）。
pub const MAX_TIMEOUT_SECS: u64 = 120;
/// 结果默认截断字符数（与 `--max-output` 同步下发给 CLI，本层再截一次兜底）。
pub const DEFAULT_MAX_OUTPUT: usize = 8000;
/// daemon 空闲回收（毫秒）：20 分钟无动作即销毁（事实 2）。
const IDLE_TIMEOUT_MS: &str = "1200000";

/// 桥接端点（主进程 `browser-agent.json` 的子集；多余字段忽略）。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Endpoint {
    /// 形如 `ws://127.0.0.1:53421/astorm/<token>`
    pub ws_url: String,
    pub token: String,
    /// 当前激活页签的页面 URL（站点 URL；审批 origin / 展示用）
    pub frame_url: String,
    #[serde(default)]
    pub tab_id: Option<String>,
    /// 端点所属的 astorm 会话 id：一桥一会话，工具据此校验"这条端点属于我这条会话"
    #[serde(default)]
    pub session_id: Option<String>,
    /// 主进程 pid：用于判断端点是活的还是上次遗留的
    #[serde(default)]
    pub pid: Option<u32>,
}

impl Endpoint {
    /// daemon 会话名：**一会话一实例** + token 前缀（换桥即换 daemon）。
    /// 端点是会话级的（覆盖该会话全部浏览器页签），故用 sessionId 命名；
    /// 老端点缺 sessionId 时回退 tabId（兼容）。
    pub fn session_name(&self) -> String {
        let key = self
            .session_id
            .as_deref()
            .filter(|s| !s.is_empty())
            .or_else(|| self.tab_id.as_deref().filter(|t| !t.is_empty()))
            .unwrap_or("tab");
        let key: String = key.chars().filter(|c| c.is_ascii_alphanumeric()).take(12).collect();
        let token: String = self.token.chars().take(8).collect();
        format!("astorm-{key}-{token}")
    }

    /// 站点 origin（`scheme://host[:port]`；解析失败回退整串）——授权粒度与审计用。
    pub fn origin(&self) -> String {
        origin_of(&self.frame_url)
    }
}

/// 从 URL 取 origin（不引 url crate：只需 scheme + authority 两段）。
pub fn origin_of(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else {
        return url.to_string();
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.is_empty() {
        return url.to_string();
    }
    format!("{scheme}://{authority}")
}

/// 端点文件路径：`{electron userData}/browser-agent.json`（Rust 侧 `AppDir.electron_dir` 与主进程一致）。
///
/// `ASTORM_BROWSER_ENDPOINT` 可覆盖（端到端冒烟测试在没有 App 单例的进程里直接指向运行中的应用）。
pub fn endpoint_path() -> PathBuf {
    if let Some(p) = std::env::var_os("ASTORM_BROWSER_ENDPOINT") {
        let pb = PathBuf::from(p);
        if !pb.as_os_str().is_empty() {
            return pb;
        }
    }
    App::try_global()
        .map(|app| app.dir().electron_dir.join("browser-agent.json"))
        .unwrap_or_default()
}

/// 读取当前端点；不存在 / 不完整 / 会话不符 → 给"先打开网页页签"的可操作错误。
pub fn read_endpoint(session_id: &str) -> Result<Endpoint, String> {
    let path = endpoint_path();
    let raw = std::fs::read_to_string(&path).map_err(|_| {
        format!(
            "当前没有可操控的网页页签：未检测到浏览器桥接端点（{}）。\
             请让用户先在内置浏览器里打开一个网页页签（右侧「+」→ 浏览器 → 输入网址），再重试。",
            path.display()
        )
    })?;
    let ep: Endpoint = serde_json::from_str(&raw)
        .map_err(|e| format!("浏览器桥接端点解析失败（{}）：{e}", path.display()))?;
    if ep.ws_url.trim().is_empty() {
        return Err(format!("浏览器桥接端点不完整（{}）：缺少 wsUrl", path.display()));
    }
    // 端点文件是「主进程 pid + 桥接端口」的快照，且本层就跑在主进程里（napi dll），
    // 所以 pid 对不上只可能是**上次进程遗留的文件**（端口早已回收，连上去只会得到难懂的
    // 连接错误）；`pid` 缺省时不做判断（兼容手写的端点）。
    if let Some(pid) = ep.pid.filter(|p| *p != 0 && *p != std::process::id()) {
        return Err(format!(
            "浏览器桥接端点已失效（{} 由 pid {pid} 写入，当前主进程 pid {}）：\
             请在内置浏览器面板里重新激活要操控的网页页签。",
            path.display(),
            std::process::id()
        ));
    }
    // 一桥一会话：端点属于另一个会话时不能拿来驱动（否则会操作到别的会话的页签）
    if let Some(owner) = ep.session_id.as_deref().filter(|s| !s.is_empty()) {
        if owner != session_id {
            return Err(format!(
                "当前浏览器页签属于另一个会话（端点会话 {owner}，本会话 {session_id}）：\
                 请切换到该会话的内置浏览器页签后再重试。"
            ));
        }
    }
    Ok(ep)
}

/// 定位 `agent-browser` 可执行文件：配置路径 → 环境变量 → PATH 里的 `agent-browser`。
pub fn resolve_binary() -> Result<PathBuf, String> {
    if let Some(app) = App::try_global() {
        if let Some(p) = app.config().browser.binary.as_deref().map(str::trim) {
            if !p.is_empty() {
                let pb = Path::new(p);
                if pb.is_file() {
                    return Ok(pb.to_path_buf());
                }
            }
        }
    }
    if let Some(p) = std::env::var_os("ASTORM_AGENT_BROWSER_BIN") {
        let pb = PathBuf::from(p);
        if pb.is_file() {
            return Ok(pb);
        }
    }
    // PATH 查找：统一走 PATHEXT 解析（Windows 上 npx 系安装可能只有 .cmd shim）
    if let Some(found) = astorm_sandbox::toolchain::which_path("agent-browser") {
        return Ok(found);
    }
    Err("未找到 agent-browser 可执行文件：请在应用里预置（resources/agent-browser/…）\
         或设置 ASTORM_AGENT_BROWSER_BIN 指向本地构建产物"
        .to_string())
}

/// 一次 CLI 调用结果。
pub struct CliOutput {
    /// `--json` 信封里的 `data`
    pub data: Value,
    /// `_boundary.origin`（页面来源；用于审计 / 授权粒度）
    pub origin: Option<String>,
}

/// `--json` 信封（agent-browser 稳定契约）。
#[derive(Debug, Deserialize)]
struct Envelope {
    success: bool,
    #[serde(default)]
    data: Value,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    code: Option<String>,
    #[serde(default, rename = "_boundary")]
    boundary: Option<Value>,
}

/// 让 agent-browser **无法**启动本地浏览器：`--executable-path` 指向一个永不存在的文件。
///
/// 语义：我们的每次动作都必须附着在内置浏览器页签的桥上（`--cdp`）。CLI 若因任何原因走到
/// 「自己 launch 一个浏览器」的分支（zero-config 用法），给不存在的可执行路径会让它以
/// `Failed to launch Chrome at …` 明确失败 —— 而不是给用户弹出一个本地 Chrome 窗口。
fn disabled_local_browser() -> String {
    endpoint_path()
        .with_file_name("agent-browser-disabled-local-browser")
        .to_string_lossy()
        .into_owned()
}

/// 基准参数（`--json` 信封 + 附着 + 禁自启）；单测可断言其中包含哪些开关。
pub fn base_args(ep: &Endpoint, max_output: usize) -> Vec<String> {
    [
        "--session",
        &ep.session_name(),
        "--cdp",
        &ep.ws_url,
        "--json",
        "--content-boundaries",
        "--max-output",
        &max_output.to_string(),
        "--executable-path",
        &disabled_local_browser(),
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

/// 执行一次 `agent-browser`（含超时与取消；超时/取消即杀子进程）。
pub async fn run(
    ep: &Endpoint,
    args: &[String],
    timeout: Duration,
    max_output: usize,
    cancel: CancellationToken,
) -> Result<CliOutput, String> {
    let binary = resolve_binary()?;
    let mut cmd = Command::new(&binary);
    cmd.args(base_args(ep, max_output))
        .args(args)
        .env("AGENT_BROWSER_IDLE_TIMEOUT_MS", IDLE_TIMEOUT_MS);

    // 超时/取消按进程树回收（CLI 会拉起 browser/daemon 子进程）。
    let out = crate::process::output_with_tree_kill(cmd, timeout, Some(&cancel))
        .await
        .map_err(|e| match e {
            crate::process::TreeRunError::Spawn(e) => {
                format!("启动 agent-browser 失败：{e}")
            }
            crate::process::TreeRunError::Io(e) => {
                format!("读取 agent-browser 输出失败：{e}")
            }
            crate::process::TreeRunError::Timeout => format!(
                "操作超时（{}s）已终止。可以改用更短的等待（wait_for 的 ms/超时参数）或先 snapshot 看状态。",
                timeout.as_secs()
            ),
            crate::process::TreeRunError::Cancelled => {
                "操作已取消（用户中断）。页面停留在当前状态，可继续 snapshot 观察。".to_string()
            }
        })?;

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let text = stdout.trim();
    if text.is_empty() {
        let hint = stderr.trim();
        return Err(if hint.is_empty() {
            format!("agent-browser 无输出（退出码 {:?}）", out.status.code())
        } else {
            format!("agent-browser 无输出：{hint}")
        });
    }
    let env: Envelope = serde_json::from_str(text)
        .map_err(|e| format!("agent-browser 输出无法解析（{e}）：{}", truncate(text, 400)))?;
    if !env.success {
        let code = env.code.unwrap_or_default();
        let msg = env.error.unwrap_or_else(|| "未知错误".into());
        return Err(if code.is_empty() {
            format!("agent-browser 失败：{msg}")
        } else {
            format!("agent-browser 失败（{code}）：{msg}")
        });
    }
    // 兜底：若它仍然自己起了浏览器（换了条路径），一律判失败，绝不让"两边同时操作"发生
    if env
        .data
        .get("lifecycle")
        .and_then(|l| l.get("launched"))
        .and_then(|v| v.as_bool())
        == Some(true)
    {
        return Err(
            "检测到 agent-browser 试图启动本地浏览器，已阻止：所有动作必须落在内置浏览器的网页页签里。\
             请确认页签仍然打开（面板里能看到页面），然后重试。"
                .to_string(),
        );
    }
    let origin = env
        .boundary
        .as_ref()
        .and_then(|b| b.get("origin"))
        .and_then(|o| o.as_str())
        .map(str::to_string);
    Ok(CliOutput { data: env.data, origin })
}

/// 按字符安全截断（UTF-8 边界）。
pub fn truncate(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        s.chars().take(max_chars).collect::<String>() + "…\n[已截断]"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ep(tab: &str, token: &str) -> Endpoint {
        Endpoint {
            ws_url: format!("ws://127.0.0.1:5/t/{}", token),
            token: token.to_string(),
            frame_url: "https://example.com/a/b?c=1".into(),
            tab_id: Some(tab.into()),
            // 会话级端点：sessionId 与 tabId 分开；此处用同值便于复用断言
            session_id: Some(tab.into()),
            pid: Some(1),
        }
    }

    #[test]
    fn 会话名按会话与token前缀() {
        // 一桥一会话：名字取 sessionId（此处 sessionId == tabId）
        assert_eq!(ep("mtwlagopxv7cvi", "9b9757ab14bf").session_name(), "astorm-mtwlagopxv7c-9b9757ab");
        // 老端点缺 sessionId → 回退 tabId
        let mut e = ep("sess12345678", "abcdef1234");
        e.session_id = None;
        assert_eq!(e.session_name(), "astorm-sess12345678-abcdef12");
        // 两者都缺 → tab
        e.tab_id = None;
        assert_eq!(e.session_name(), "astorm-tab-abcdef12");
    }

    #[test]
    fn origin_取自frame_url() {
        assert_eq!(ep("t", "x").origin(), "https://example.com");
        assert_eq!(origin_of("http://127.0.0.1:8099/tall.html"), "http://127.0.0.1:8099");
        assert_eq!(origin_of("about:blank"), "about:blank");
    }

    #[test]
    fn 信封解析与错误归一() {
        let ok: Envelope =
            serde_json::from_str(&json!({"success":true,"data":{"url":"https://x/"},"_boundary":{"origin":"https://x"}}).to_string())
                .unwrap();
        assert!(ok.success);
        assert_eq!(ok.data["url"], json!("https://x/"));
        assert_eq!(ok.boundary.unwrap()["origin"], json!("https://x"));

        let bad: Envelope = serde_json::from_str(
            &json!({"success":false,"error":"no such element","code":"not_found"}).to_string(),
        )
        .unwrap();
        assert!(!bad.success);
        assert_eq!(bad.code.as_deref(), Some("not_found"));
    }

    #[test]
    fn 基准参数禁自启且锁定内置页签() {
        let a = base_args(&ep("tab1", "tok12345678"), 8000);
        let has = |flag: &str| a.iter().any(|x| x == flag);
        assert!(has("--cdp"), "必须附着到内置页签的桥: {a:?}");
        assert!(has("--session"), "必须一页签一实例: {a:?}");
        assert!(has("--executable-path"), "必须禁掉本地浏览器自启: {a:?}");
        let sentinel = a[a.iter().position(|x| x == "--executable-path").unwrap() + 1].clone();
        assert!(!std::path::Path::new(&sentinel).exists(), "哨兵路径必须不存在: {sentinel}");
        assert!(a.iter().any(|x| x == "--content-boundaries"), "反注入边界常开: {a:?}");
        assert!(a.windows(2).any(|w| w[0] == "--max-output" && w[1] == "8000"));
    }

    #[test]
    fn 端点缺失或失效都给出可操作指引() {
        // SAFETY: 环境变量用例都集中在本测试里，避免并发设置相互干扰
        unsafe {
            std::env::set_var("ASTORM_BROWSER_ENDPOINT", "/nonexistent/browser-agent.json");
        }
        let err = read_endpoint("sess-a").expect_err("端点缺失应报错");
        assert!(err.contains("没有可操控的网页页签"), "{err}");
        assert!(err.contains("打开一个网页页签"), "要给出下一步动作: {err}");

        // 上次进程遗留的端点：pid 对不上 → 不能拿它去连已回收的端口
        let stale = std::env::temp_dir().join("astorm-browser-agent-stale-test.json");
        std::fs::write(
            &stale,
            json!({
                "wsUrl": "ws://127.0.0.1:1/astorm/x",
                "token": "x",
                "frameUrl": "https://example.com/",
                "pid": std::process::id() + 1,
            })
            .to_string(),
        )
        .unwrap();
        unsafe {
            std::env::set_var("ASTORM_BROWSER_ENDPOINT", &stale);
        }
        let err = read_endpoint("sess-a").expect_err("遗留端点应报错");
        assert!(err.contains("已失效"), "{err}");
        assert!(err.contains("重新激活"), "要给出下一步动作: {err}");

        // 端点属于另一个会话 → 拒绝（一桥一会话）
        let other = std::env::temp_dir().join("astorm-browser-agent-other-session-test.json");
        std::fs::write(
            &other,
            json!({
                "wsUrl": "ws://127.0.0.1:1/astorm/x",
                "token": "x",
                "frameUrl": "https://example.com/",
                "sessionId": "sess-b",
                "pid": std::process::id(),
            })
            .to_string(),
        )
        .unwrap();
        unsafe {
            std::env::set_var("ASTORM_BROWSER_ENDPOINT", &other);
        }
        let err = read_endpoint("sess-a").expect_err("跨会话端点应报错");
        assert!(err.contains("另一个会话"), "{err}");

        let _ = std::fs::remove_file(&stale);
        let _ = std::fs::remove_file(&other);
        unsafe {
            std::env::remove_var("ASTORM_BROWSER_ENDPOINT");
        }
    }

    #[test]
    fn 截断按字符不切字节() {
        assert_eq!(truncate("中文abc", 3), "中文a…\n[已截断]");
        assert_eq!(truncate("短", 10), "短");
    }
}
