//! subscribe：订阅全局事件总线（返回 Web ReadableStream）。

use napi::bindgen_prelude::{Env, ReadableStream};
use napi_derive::napi;
use serde_json::Value;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;

/// 订阅全局事件流：所有会话的协议事件经此单一流回传，
/// 前端按 `session_id` + `type` 分发处理。
///
/// 注意：广播无回放，需在创建会话前订阅；lag 时降级为 error 事件而非终止流。
#[napi]
pub fn subscribe(env: Env) -> napi::Result<ReadableStream<'static, Value>> {
    let rx = crate::app()?.subscribe();
    let stream = BroadcastStream::new(rx).map(|item| match item {
        Ok(value) => Ok(value),
        Err(e) => {
            // 广播 lag：降级为一条 error 事件，保持订阅流存活。
            Ok(serde_json::json!({
                "type": "error",
                "session_id": null,
                "message": format!("事件总线积压丢帧: {e}"),
            }))
        }
    });
    ReadableStream::new(&env, stream)
}
