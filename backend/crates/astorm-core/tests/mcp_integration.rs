//! MCP 客户端**真装配**集成验证：`App::init` + `mcp_servers.json` + 真实 stdio 子进程。
//!
//! 为什么单独一个测试文件：`App::init` 是**进程级单例**（`OnceLock`），同一测试二进制里
//! 只能初始化一次；独立文件的测试目标是独立进程。共用脚手架见 `common/mod.rs`。
//!
//! 覆盖（夹具 = `tests/fixtures/mock-mcp-server.mjs`，node 实现的极简 MCP 服务器）：
//! 1. **启动自动连接**：配置里 `enabled`（缺省）的服务器在启动后被连上（`connected`）
//! 2. **工具发现 + 命名规则**：`echo` / `add` 原名暴露；与内置工具同名的 `bash`
//!    暴露为 `mock__bash`（内置优先）
//! 3. **工具进入会话工具集**：`session_tool_names` 含 MCP 工具名
//! 4. **真实调用**：`tools/call` 往返（正常 / isError 两种）
//! 5. **断开**：断开后工具从会话工具集消失
//!
//! 无 node 解释器时整体跳过（打印提示）——夹具是 JS 实现，缺解释器无法跑协议。
//!
//! 所有异步调用都经 `app.handle()` 在**应用自己的 runtime** 上驱动：stdio 子进程的
//! I/O 资源绑定在创建它的 runtime 上，跨 runtime 轮询不稳定（同邮件连接池的注记）。

mod common;

use std::time::Duration;

use common::{Fixture, wait_until};

/// 夹具脚本绝对路径（`CARGO_MANIFEST_DIR/tests/fixtures/...`）。
fn fixture_script() -> String {
    format!(
        "{}/tests/fixtures/mock-mcp-server.mjs",
        env!("CARGO_MANIFEST_DIR")
    )
}

/// node 是否可用（不可用 → 跳过，不算失败）。
fn node_available() -> bool {
    std::process::Command::new("node")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[test]
fn mcp_stdio_connect_discover_call_and_disconnect() {
    if !node_available() {
        eprintln!("跳过：未找到 node（MCP 夹具需要 node 解释器）");
        return;
    }
    let f = Fixture::new("mcp");
    // 配置文件落在 <应用根>/config/mcp_servers.json（AppDir::config_dir）
    let config_dir = f.app_root.join("config");
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::write(
        config_dir.join("mcp_servers.json"),
        serde_json::json!({
            "mcpServers": {
                "mock": { "command": "node", "args": [fixture_script()] }
            }
        })
        .to_string(),
    )
    .unwrap();

    let b = common::boot(&f, f.config_full(false, false, false, 0));
    let app = b.app;
    let handle = app.handle();

    // ---- 1. 启动自动连接 ----
    let connected = wait_until(Duration::from_secs(20), || {
        !app.mcp().connected_servers().is_empty()
    });
    assert!(connected, "启用中的 MCP 服务器未在启动后自动连接");
    let rows = app.mcp().list_servers();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].name, "mock");
    assert_eq!(rows[0].transport_type, "stdio");
    assert!(rows[0].connected, "连接后 connected 应为 true");

    // ---- 2. 工具发现 + 命名规则 ----
    let tools = app.mcp().list_tools("mock");
    let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
    assert!(names.contains(&"echo"), "缺 echo: {names:?}");
    assert!(names.contains(&"add"), "缺 add: {names:?}");
    assert!(
        names.contains(&"mock__bash"),
        "与内置工具同名的 MCP 工具应加 `mock__` 前缀: {names:?}"
    );
    assert!(!names.contains(&"bash"), "内置名不应被 MCP 工具占用: {names:?}");

    // ---- 3. 工具进入会话工具集（空可见集 + 空加载集也应带上 MCP 工具）----
    let session_tools = app.session_tool_names(&[], &[]);
    assert!(
        session_tools.iter().any(|n| n == "echo"),
        "会话工具集缺 MCP 工具: {session_tools:?}"
    );
    assert!(
        session_tools.iter().any(|n| n == "mock__bash"),
        "会话工具集缺重命名后的 MCP 工具: {session_tools:?}"
    );

    // ---- 4. 真实调用（经循环侧句柄 → tools/call）----
    let handles = app.mcp().connected_tools();
    let echo = handles
        .iter()
        .find(|t| t.name() == "echo")
        .expect("缺 echo 句柄")
        .clone();
    let cancel = tokio_util::sync::CancellationToken::new();
    let ctx = react_loop::ToolContext::new(cancel);
    let out = handle
        .block_on(echo.execute(serde_json::json!({"text": "hello-mcp"}), &ctx))
        .expect("echo 调用失败");
    assert_eq!(out, "echo: hello-mcp");

    let boom = handles
        .iter()
        .find(|t| t.name() == "boom")
        .expect("缺 boom 句柄")
        .clone();
    let err = handle
        .block_on(boom.execute(serde_json::json!({}), &ctx))
        .expect_err("isError 应转为 Err");
    assert!(err.contains("工具执行失败"), "错误原文未透传: {err}");

    // ---- 5. 断开 → 工具从会话工具集消失 ----
    app.mcp().disconnect("mock").expect("断开失败");
    assert!(app.mcp().connected_servers().is_empty());
    let after = app.session_tool_names(&[], &[]);
    assert!(
        !after.iter().any(|n| n == "echo"),
        "断开后 MCP 工具仍在下发: {after:?}"
    );

    // ---- 6. 连接不存在的服务器 → 可读错误 ----
    let err = handle
        .block_on(app.mcp().connect("nope"))
        .expect_err("连接不存在的服务器应报错");
    assert!(err.contains("不存在"), "错误文案: {err}");
}
