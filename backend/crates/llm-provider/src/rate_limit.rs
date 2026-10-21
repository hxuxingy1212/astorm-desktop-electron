//! 令牌桶限流：按速率平滑限制对供应商的请求频率。
//!
//! 桶容量 = 突发上限（burst），令牌按 rps/秒 惰性补充（按时间差计算，无后台定时器）。
//! 令牌不足时等待补充（可取消），等待超过 [`MAX_WAIT`] 抛 [`LlmError::RateLimited`]。

use crate::{LlmError, RateLimitConfig};
use std::sync::Mutex;
use std::time::Duration;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

/// 等待令牌的最长时间，超时抛 [`LlmError::RateLimited`]。
const MAX_WAIT: Duration = Duration::from_secs(30);

/// 令牌桶限流器（线程安全，可在多个请求间共享）。
pub struct TokenBucket {
    rps: f64,
    capacity: f64,
    state: Mutex<BucketState>,
}

struct BucketState {
    tokens: f64,
    updated_at: Instant,
}

impl TokenBucket {
    /// 创建令牌桶；`rps` 必须大于 0。
    pub fn new(config: RateLimitConfig) -> Self {
        assert!(config.rps > 0.0, "rps 必须大于 0");
        let capacity = config.burst.max(1) as f64;
        Self {
            rps: config.rps,
            capacity,
            state: Mutex::new(BucketState {
                tokens: capacity,
                updated_at: Instant::now(),
            }),
        }
    }

    /// 获取一个令牌；令牌不足时等待补充（支持取消），总等待超过 [`MAX_WAIT`] 抛限流错误。
    pub async fn acquire(&self, cancel: &CancellationToken) -> Result<(), LlmError> {
        let started = Instant::now();
        loop {
            // 锁内检查并扣减；释放锁后再 sleep，避免持锁跨 await
            let wait = {
                let mut st = self.state.lock().unwrap();
                self.refill(&mut st);
                if st.tokens >= 1.0 {
                    st.tokens -= 1.0;
                    return Ok(());
                }
                let need = 1.0 - st.tokens;
                Duration::from_secs_f64(need / self.rps)
            };

            if started.elapsed() + wait > MAX_WAIT {
                return Err(LlmError::RateLimited(format!(
                    "请求过于频繁：等待令牌超过 {}s（rps={}）",
                    MAX_WAIT.as_secs(),
                    self.rps
                )));
            }
            tokio::select! {
                biased;
                _ = cancel.cancelled() => return Err(LlmError::Cancelled),
                _ = tokio::time::sleep(wait) => {}
            }
        }
    }

    /// 按时间差惰性补充令牌（封顶到桶容量）。
    fn refill(&self, st: &mut BucketState) {
        let now = Instant::now();
        let elapsed = now.duration_since(st.updated_at).as_secs_f64();
        if elapsed > 0.0 {
            st.tokens = (st.tokens + elapsed * self.rps).min(self.capacity);
            st.updated_at = now;
        }
    }
}
