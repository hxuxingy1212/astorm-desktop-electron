//! 使用统计：按「本地日期 × 模型」聚合 token 用量（设置页「使用统计」）。

use napi_derive::napi;

/// 聚合 token 用量（`since_ts` 为 unix 秒下限；返回 JSON 数组字符串）。
///
/// 元素形如：`{ day, model, prompt, completion, cacheHit, cacheMiss, total }`。
#[napi]
pub fn usage_stats(since_ts: i64) -> napi::Result<String> {
    let app = crate::app()?;
    let rows = app.usage_stats(since_ts).map_err(napi::Error::from_reason)?;
    Ok(serde_json::to_string(&rows).unwrap_or_else(|_| "[]".to_string()))
}
