//! `browser` 工具单测：动作分类、CLI 参数拼装、台账 key、Plan 硬拒、结果整形。
//! 不触网、不起 CLI（spawn 路径由 §18 的端到端验证覆盖）。

use super::*;
use serde_json::json;

fn args(v: Value) -> BrowserArgs {
    serde_json::from_value(v).expect("参数解析")
}

#[test]
fn 页签管理动作映射到tab子命令() {
    // tab_list → `tab list`（只读）
    assert!(READ_ACTIONS.contains(&"tab_list"));
    assert_eq!(cli_args("tab_list", &args(json!({"action":"tab_list"}))).unwrap(), vec!["tab", "list"]);
    // tab_new [url]
    assert!(WRITE_ACTIONS.contains(&"tab_new"));
    assert_eq!(
        cli_args("tab_new", &args(json!({"action":"tab_new","url":"https://example.com"}))).unwrap(),
        vec!["tab", "new", "https://example.com"]
    );
    // tab_new 不带 url → 空页签
    assert_eq!(cli_args("tab_new", &args(json!({"action":"tab_new"}))).unwrap(), vec!["tab", "new"]);
    // tab_switch <ref>
    assert_eq!(
        cli_args("tab_switch", &args(json!({"action":"tab_switch","tab":"2"}))).unwrap(),
        vec!["tab", "2"]
    );
    // tab_close [ref] / 省略 ref 关闭当前页签
    assert_eq!(
        cli_args("tab_close", &args(json!({"action":"tab_close","tab":"docs"}))).unwrap(),
        vec!["tab", "close", "docs"]
    );
    assert_eq!(cli_args("tab_close", &args(json!({"action":"tab_close"}))).unwrap(), vec!["tab", "close"]);
    // tab_switch 缺 tab → 可读错误
    assert!(cli_args("tab_switch", &args(json!({"action":"tab_switch"}))).is_err());
}

#[test]
fn 只读与写类动作分类不重叠且覆盖schema枚举() {
    for a in READ_ACTIONS {
        assert!(!WRITE_ACTIONS.contains(&a), "{a} 不应同时是写类");
    }
    let schema = BrowserTool.parameters_schema();
    let enum_vals: Vec<&str> = schema["properties"]["action"]["enum"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    for a in READ_ACTIONS.iter().chain(WRITE_ACTIONS.iter()) {
        assert!(enum_vals.contains(a), "schema 缺少 {a}");
    }
    // 首版不暴露 eval
    assert!(!enum_vals.contains(&"eval"));
}

#[test]
fn snapshot_默认只要交互元素_可限定范围() {
    assert_eq!(cli_args("snapshot", &args(json!({"action":"snapshot"}))).unwrap(), vec!["snapshot", "-i"]);
    assert_eq!(
        cli_args("snapshot", &args(json!({"action":"snapshot","interactive":false,"selector":"#main"}))).unwrap(),
        vec!["snapshot", "-s", "#main"]
    );
}

#[test]
fn read_按what映射到get子命令() {
    assert_eq!(cli_args("read", &args(json!({"action":"read","what":"url"}))).unwrap(), vec!["get", "url"]);
    assert_eq!(
        cli_args("read", &args(json!({"action":"read","what":"attr","target":"@e1","attr":"href"}))).unwrap(),
        vec!["get", "attr", "@e1", "href"]
    );
    assert_eq!(
        cli_args("read", &args(json!({"action":"read","target":"h1"}))).unwrap(),
        vec!["get", "text", "h1"]
    );
    // 缺 target 的 read 变体要给出可读错误
    assert!(cli_args("read", &args(json!({"action":"read","what":"text"}))).is_err());
}

#[test]
fn type_清空与提交分别映射fill与press() {
    assert_eq!(
        cli_args("type", &args(json!({"action":"type","target":"#q","text":"rust"}))).unwrap(),
        vec!["type", "#q", "rust"]
    );
    assert_eq!(
        cli_args("type", &args(json!({"action":"type","target":"#q","text":"rust","clear":true}))).unwrap(),
        vec!["fill", "#q", "rust"]
    );
}

#[test]
fn scroll_目标优先走scrollintoview_否则方向加像素() {
    assert_eq!(
        cli_args("scroll", &args(json!({"action":"scroll","target":"#bottom"}))).unwrap(),
        vec!["scrollintoview", "#bottom"]
    );
    assert_eq!(
        cli_args("scroll", &args(json!({"action":"scroll","dir":"down","px":1800}))).unwrap(),
        vec!["scroll", "down", "1800"]
    );
    assert_eq!(cli_args("scroll", &args(json!({"action":"scroll"}))).unwrap(), vec!["scroll", "down", "600"]);
}

#[test]
fn wait_for_三种形态与缺参报错() {
    assert_eq!(
        cli_args("wait_for", &args(json!({"action":"wait_for","wait_text":"结果"}))).unwrap(),
        vec!["wait", "--text", "结果"]
    );
    assert_eq!(cli_args("wait_for", &args(json!({"action":"wait_for","ms":1500}))).unwrap(), vec!["wait", "1500"]);
    assert_eq!(
        cli_args("wait_for", &args(json!({"action":"wait_for","selector":".done"}))).unwrap(),
        vec!["wait", ".done"]
    );
    assert!(cli_args("wait_for", &args(json!({"action":"wait_for"}))).is_err());
}

#[test]
fn 写类动作参数校验() {
    assert_eq!(cli_args("click", &args(json!({"action":"click","target":"@e3"}))).unwrap(), vec!["click", "@e3"]);
    assert_eq!(cli_args("press", &args(json!({"action":"press","key":"Enter"}))).unwrap(), vec!["press", "Enter"]);
    assert!(cli_args("press", &args(json!({"action":"press"}))).is_err());
    assert_eq!(
        cli_args("select", &args(json!({"action":"select","target":"#city","values":["bj","sh"]}))).unwrap(),
        vec!["select", "#city", "bj", "sh"]
    );
    assert!(cli_args("select", &args(json!({"action":"select","target":"#city"}))).is_err());
    assert_eq!(
        cli_args("navigate", &args(json!({"action":"navigate","url":"https://example.com/"}))).unwrap(),
        vec!["open", "https://example.com/"]
    );
    assert!(cli_args("navigate", &args(json!({"action":"navigate"}))).is_err());
    assert_eq!(cli_args("history", &args(json!({"action":"history","history_action":"reload"}))).unwrap(), vec!["reload"]);
    assert!(cli_args("history", &args(json!({"action":"history","history_action":"jump"}))).is_err());
}

#[test]
fn 台账key稳定可重放() {
    let a = args(json!({"action":"click","target":"@e3"}));
    assert_eq!(ledger_key("click", &a), "browser:click:@e3");
    assert_eq!(ledger_key("click", &a), ledger_key("click", &args(json!({"action":"click","target":"@e3"}))));
    let n = args(json!({"action":"navigate","url":"https://example.com/x"}));
    assert_eq!(ledger_key("navigate", &n), "browser:navigate:https://example.com/x");
}

#[test]
fn 超时按动作分档且不超上限() {
    assert_eq!(timeout_for("click", &args(json!({"action":"click","target":"@e1"}))).as_secs(), 30);
    assert_eq!(timeout_for("navigate", &args(json!({"action":"navigate","url":"https://x/"}))).as_secs(), 60);
    assert_eq!(timeout_for("wait_for", &args(json!({"action":"wait_for","ms":100000}))).as_secs(), 115);
    assert_eq!(timeout_for("wait_for", &args(json!({"action":"wait_for","ms":1000}))).as_secs(), 16);
}

#[test]
fn 截图文件名带页签与时间戳且非法字符被清洗() {
    let session = "unit-test-session";
    let p = snapshot_path(session, Some("../../etc/passwd"), Some("tab-1/x")).unwrap();
    let name = p.file_name().unwrap().to_string_lossy().to_string();
    assert!(name.ends_with(".png"), "{name}");
    assert!(!name.contains('/'), "路径分隔符必须被清洗: {name}");
    assert!(!name.contains(".."), "上级目录必须被清洗: {name}");
    // 页签段只保留字母数字（'-' 被清洗）→ tab1x-<时间戳>-<清洗后的文件名>
    assert!(name.starts_with("tab1x-"), "页签前缀: {name}");
    assert!(name.contains("-passwd.png"), "文件名主体保留（目录部分被剥掉）: {name}");
    assert!(p.starts_with(resolve_workspace(session)), "必须落在会话工作区");
}

#[test]
fn 结果整形给网页内容加不可信边界() {
    let out = CliOutput {
        data: json!({"snapshot": "- heading \"Example Domain\""}),
        origin: Some("https://example.com".into()),
    };
    let ep = agent_cli::Endpoint {
        ws_url: "ws://127.0.0.1:1/astorm/t".into(),
        token: "t".into(),
        frame_url: "https://example.com/".into(),
        tab_id: Some("tab".into()),
        session_id: None,
        pid: None,
    };
    let text = format_result("snapshot", &args(json!({"action":"snapshot"})), &out, &ep.origin(), &dummy_ctx());
    assert!(text.contains("不可信"), "{text}");
    assert!(text.contains("https://example.com"), "{text}");
    assert!(text.contains("Example Domain"), "{text}");

    let shot = CliOutput { data: json!({"path": "/ws/.astorm/browser-snapshot/x.png"}), origin: None };
    let text = format_result("screenshot", &args(json!({"action":"screenshot"})), &shot, &ep.origin(), &dummy_ctx());
    assert!(text.contains("read_images"), "截图结果要提示可用 read_images 看图: {text}");
}

fn dummy_ctx() -> ToolContext {
    ToolContext::new(tokio_util::sync::CancellationToken::new())
        .with_session_id("sess-test")
        .with_tool_call_id("call-test")
}
