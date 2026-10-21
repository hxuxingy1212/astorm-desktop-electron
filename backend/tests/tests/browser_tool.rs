//! `browser` 工具**真机冒烟测试**：直接调用工具（不经 LLM），验证
//! 「Rust 工具 → agent-browser CLI → 主进程 iframe 作用域 CDP 桥 → 内置浏览器页签」这条链。
//!
//! 前置条件（缺一即跳过，不算失败）：
//! 1. 应用在运行，并且**已打开一个网页页签**（右侧「+」→ 浏览器 → 输入网址）；
//! 2. 主进程已写出端点文件（`userData/browser-agent.json`，随页签起停）；
//! 3. `agent-browser` 可执行文件可用（`resources/agent-browser/<os>-<arch>/` 已由
//!    `scripts/sync-agent-browser.mjs` 同步，或用 `ASTORM_AGENT_BROWSER_BIN` 指定）。
//!
//! 运行：
//! ```bash
//! cd backend
//! ASTORM_BROWSER_ENDPOINT="$HOME/Library/Application Support/astorm/electron/browser-agent.json" \
//! ASTORM_AGENT_BROWSER_BIN="backend/../frontend/resources/agent-browser/darwin-arm64/agent-browser" \
//! cargo test -p astorm-tests --test browser_tool -- --ignored --nocapture
//! ```
//!
//! 该测试**不初始化 App**（避免与运行中的应用抢 sqlite / 索引锁）：端点与二进制都经环境变量注入，
//! 工作区回退默认目录（截图落到 `~/astorm/workspace/.astorm/browser-snapshot/`）。

use astorm_core::tool::browser::BrowserTool;
use react_loop::{Tool, ToolContext};
use serde_json::json;
use tokio_util::sync::CancellationToken;

fn endpoint_file() -> Option<std::path::PathBuf> {
    let candidates = [
        std::env::var_os("ASTORM_BROWSER_ENDPOINT").map(std::path::PathBuf::from),
        dirs_home().map(|h| {
            h.join("Library/Application Support/astorm/electron/browser-agent.json")
        }),
    ];
    candidates
        .into_iter()
        .flatten()
        .find(|p| p.is_file())
}

fn dirs_home() -> Option<std::path::PathBuf> {
    std::env::var_os("HOME").map(std::path::PathBuf::from)
}

fn ctx() -> ToolContext {
    ToolContext::new(CancellationToken::new())
        .with_session_id("browser-tool-smoke")
        .with_tool_call_id("call-smoke")
}

#[test]
#[ignore = "需要应用运行且已打开网页页签（真机冒烟，见文件头注释）"]
fn browser_tool_reads_the_open_tab() {
    let Some(ep) = endpoint_file() else {
        println!("跳过：未找到 browser-agent.json（应用未运行 / 没有网页页签）");
        return;
    };
    // SAFETY: 单测试进程启动阶段设置，无并发读该变量
    unsafe {
        std::env::set_var("ASTORM_BROWSER_ENDPOINT", &ep);
    }
    println!("端点文件: {}", ep.display());
    println!("端点内容: {}", std::fs::read_to_string(&ep).unwrap_or_default().trim());

    let rt = tokio::runtime::Runtime::new().expect("runtime");
    rt.block_on(async {
        let tool = BrowserTool;

        let status = tool
            .execute(json!({ "action": "status" }), &ctx())
            .await
            .expect("status 应成功（检查应用里是否开着网页页签）");
        println!("\n--- status ---\n{status}");
        assert!(status.contains("URL:"), "status 应包含 URL 行: {status}");
        assert!(status.contains("http"), "status 应包含站点 http(s) 地址: {status}");

        let snap = tool
            .execute(json!({ "action": "snapshot" }), &ctx())
            .await
            .expect("snapshot 应成功");
        println!("\n--- snapshot(前 600 字) ---\n{}", snap.chars().take(600).collect::<String>());
        assert!(snap.contains("不可信"), "snapshot 必须带网页内容不可信提示");
        // 控制范围：不得出现应用自身 UI 文案
        for leak in ["新建", "选择目录", "手动授权", "Astorm"] {
            assert!(!snap.contains(leak), "快照泄漏了应用 UI 文案: {leak}");
        }

        let shot = tool
            .execute(json!({ "action": "screenshot" }), &ctx())
            .await
            .expect("screenshot 应成功");
        println!("\n--- screenshot ---\n{shot}");
        assert!(shot.contains("browser-snapshot"), "截图应落在工作区 .astorm/browser-snapshot/: {shot}");
        // 文件确实存在
        let path = shot
            .lines()
            .find_map(|l| l.split("已保存截图：").nth(1))
            .map(str::trim)
            .unwrap_or_default()
            .to_string();
        assert!(std::path::Path::new(&path).is_file(), "截图文件不存在: {path}");
        println!("\n截图大小: {} 字节", std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0));
    });
}

/// `eval` 首版不暴露：即使被显式请求也硬拒（拒绝分类 browser）。
#[test]
#[ignore = "可随时跑：不依赖应用运行（eval 在参数分流前就被拒）"]
fn browser_tool_rejects_eval() {
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    rt.block_on(async {
        let err = BrowserTool
            .execute(json!({ "action": "eval", "text": "1+1" }), &ctx())
            .await
            .expect_err("eval 应被拒");
        println!("\n--- err ---\n{err}");
        assert!(err.contains("eval"), "{err}");
        assert!(err.contains("browser"), "拒绝分类应为 browser: {err}");
    });
}
