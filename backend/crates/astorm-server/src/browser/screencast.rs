//! 面板画面流（容器形态）：把会话浏览器的活动页以 `Page.startScreencast` 帧推给前端面板，
//! 并把面板的鼠标 / 键盘 / 导航 / 页签操作回传给同一个 Page。
//!
//! 与 AI 操控**共用同一浏览器实例**：`browser` 工具的动作与用户在面板上的操作作用在同一页，
//! 因此"AI 点哪里，用户看得见"；帧是 ack 驱动（Chrome 收到 ack 才发下一帧），不会打满带宽。
//!
//! 协议（JSON 文本帧）：
//! - 下行 `{"type":"state", url, title, width, height, tabs:[{index,title,url,active}]}`
//! - 下行 `{"type":"frame", data:<base64 jpeg>, width, height}`
//! - 下行 `{"type":"error", message}`
//! - 上行 `{"type":"mouse", action:"click|move|wheel", x, y, button?, clicks?, deltaX?, deltaY?}`
//! - 上行 `{"type":"text", text}` / `{"type":"press", key}`
//! - 上行 `{"type":"nav", url}` / `{"type":"history", action:"back|forward|reload"}`
//! - 上行 `{"type":"tab"|"tab_new"|"tab_close", index?, url?}`

use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket};
use chromiumoxide::cdp::browser_protocol::input::{
    DispatchMouseEventParams, DispatchMouseEventType, InsertTextParams, MouseButton,
};
use futures::{SinkExt, StreamExt};
use serde_json::{json, Value};

use super::BrowserHub;

/// 处理一条面板 WS 连接（鉴权已在路由层用查询串令牌校验）。
pub async fn serve(hub: Arc<BrowserHub>, sid: String, socket: WebSocket) {
    if let Err(e) = run(hub, sid, socket).await {
        tracing::debug!("[browser] 画面流结束: {e}");
    }
}

async fn run(hub: Arc<BrowserHub>, sid: String, socket: WebSocket) -> Result<(), String> {
    // 画面流：按会话复用一份 CDP 监听（多面板可同时看同一页）
    let mut frames = hub.subscribe_frames(&sid).await?;
    let page = hub.ensure_page(&sid).await?;
    let (mut sink, mut stream) = socket.split();
    // 单一写出口：帧任务与输入处理都往 channel 里塞 JSON，写任务独占 sink
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let writer = tokio::spawn(async move {
        while let Some(text) = rx.recv().await {
            if sink.send(Message::Text(text.into())).await.is_err() {
                break;
            }
        }
    });
    let state = hub.state_of(&sid).await;
    let _ = tx.send(json!({ "type": "state", "data": state }).to_string());

    // 帧 → 客户端
    let frame_tx = tx.clone();
    let frames_task = tokio::spawn(async move {
        loop {
            match frames.recv().await {
                Ok(msg) => {
                    if frame_tx.send(msg).is_err() {
                        break;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => break,
            }
        }
    });

    // 客户端 → 页面
    while let Some(Ok(msg)) = stream.next().await {
        let Message::Text(text) = msg else { continue };
        let Ok(cmd) = serde_json::from_str::<Value>(&text) else { continue };
        let kind = cmd.get("type").and_then(|t| t.as_str()).unwrap_or("");
        let result = match kind {
            "mouse" => mouse(&page, &cmd).await,
            "text" => page
                .execute(InsertTextParams::new(
                    cmd.get("text").and_then(|t| t.as_str()).unwrap_or("").to_string(),
                ))
                .await
                .map(|_| ())
                .map_err(|e| e.to_string()),
            "press" => {
                super::press_key(&page, cmd.get("key").and_then(|k| k.as_str()).unwrap_or("")).await
            }
            "nav" => {
                let url = cmd.get("url").and_then(|u| u.as_str()).unwrap_or("").to_string();
                if url.is_empty() {
                    Err("nav 需要 url".to_string())
                } else {
                    page.goto(url).await.map(|_| ()).map_err(|e| e.to_string())
                }
            }
            "history" => {
                let action = cmd.get("action").and_then(|a| a.as_str()).unwrap_or("reload");
                let expr = match action {
                    "back" => "history.back()",
                    "forward" => "history.forward()",
                    _ => "location.reload()",
                };
                page.evaluate(expr.to_string()).await.map(|_| ()).map_err(|e| e.to_string())
            }
            "tab" | "tab_new" | "tab_close" => hub.ws_tab(&sid, &cmd).await,
            "ping" => Ok(()),
            other => Err(format!("未知消息类型: {other}")),
        };
        if let Err(e) = result {
            let _ = tx.send(json!({ "type": "error", "message": e }).to_string());
        }
        // 页面可能变了（导航/页签切换）：回一次 state，并把端点里的 frameUrl 拉新
        let state = hub.state_of(&sid).await;
        let _ = tx.send(json!({ "type": "state", "data": state }).to_string());
        hub.refresh_endpoint(&sid).await;
    }
    frames_task.abort();
    writer.abort();
    Ok(())
}

/// 鼠标事件：点击（按下 + 抬起）/ 移动 / 滚轮。
async fn mouse(page: &chromiumoxide::Page, cmd: &Value) -> Result<(), String> {
    let action = cmd.get("action").and_then(|a| a.as_str()).unwrap_or("click");
    let x = cmd.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let y = cmd.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let button = match cmd.get("button").and_then(|b| b.as_str()).unwrap_or("left") {
        "right" => MouseButton::Right,
        "middle" => MouseButton::Middle,
        _ => MouseButton::Left,
    };
    match action {
        "move" => {
            let ev = DispatchMouseEventParams::builder()
                .r#type(DispatchMouseEventType::MouseMoved)
                .x(x)
                .y(y)
                .button(MouseButton::None)
                .build()
                .map_err(|e| e.to_string())?;
            page.execute(ev).await.map(|_| ()).map_err(|e| e.to_string())
        }
        "wheel" => {
            let dx = cmd.get("deltaX").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let dy = cmd.get("deltaY").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let ev = DispatchMouseEventParams::builder()
                .r#type(DispatchMouseEventType::MouseWheel)
                .x(x)
                .y(y)
                .delta_x(dx)
                .delta_y(dy)
                .build()
                .map_err(|e| e.to_string())?;
            page.execute(ev).await.map(|_| ()).map_err(|e| e.to_string())
        }
        _ => {
            let clicks = cmd.get("clicks").and_then(|v| v.as_i64()).unwrap_or(1).clamp(1, 3);
            for _ in 0..clicks {
                let down = DispatchMouseEventParams::builder()
                    .r#type(DispatchMouseEventType::MousePressed)
                    .x(x)
                    .y(y)
                    .button(button.clone())
                    .click_count(1)
                    .build()
                    .map_err(|e| e.to_string())?;
                page.execute(down).await.map_err(|e| e.to_string())?;
                let up = DispatchMouseEventParams::builder()
                    .r#type(DispatchMouseEventType::MouseReleased)
                    .x(x)
                    .y(y)
                    .button(button.clone())
                    .click_count(1)
                    .build()
                    .map_err(|e| e.to_string())?;
                page.execute(up).await.map_err(|e| e.to_string())?;
            }
            Ok(())
        }
    }
}
