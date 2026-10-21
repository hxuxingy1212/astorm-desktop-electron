//! 熔断器：连续失败达到阈值后熔断，冷却后半开探测，避免对故障供应商持续打流。
//!
//! 状态机：`Closed`（统计连续失败）→ 连续失败 ≥ 阈值 → `Open`（拒绝一切请求，
//! 抛 [`LlmError::CircuitOpen`]）→ 冷却结束 → `HalfOpen`（放行少量探测请求）→
//! 探测成功恢复 `Closed` / 探测失败重新 `Open`。

use crate::{CircuitConfig, LlmError};
use std::sync::Mutex;
use std::time::Instant;

/// 熔断状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CircuitState {
    /// 正常：请求全部放行，统计连续失败
    Closed,
    /// 熔断打开：拒绝一切请求，抛 [`LlmError::CircuitOpen`]
    Open { opened_at: Instant },
    /// 半开：放行少量探测请求，成功则恢复，失败则重新熔断
    HalfOpen { allowed: u32 },
}

/// 熔断器（线程安全，可在多个请求间共享）。
pub struct CircuitBreaker {
    /// 供应商/模型标识（用于错误信息）
    name: String,
    config: CircuitConfig,
    inner: Mutex<BreakerInner>,
}

struct BreakerInner {
    state: CircuitState,
    consecutive_failures: u32,
}

impl CircuitBreaker {
    pub fn new(name: impl Into<String>, config: CircuitConfig) -> Self {
        Self {
            name: name.into(),
            config,
            inner: Mutex::new(BreakerInner {
                state: CircuitState::Closed,
                consecutive_failures: 0,
            }),
        }
    }

    /// 请求放行闸门：熔断打开且未到冷却 → 抛 [`LlmError::CircuitOpen`]；半开放满 → 同样拒绝。
    pub fn allow(&self) -> Result<(), LlmError> {
        let mut inner = self.inner.lock().unwrap();
        match &inner.state {
            CircuitState::Closed => Ok(()),
            CircuitState::Open { opened_at } => {
                if opened_at.elapsed() >= self.config.open_duration {
                    // 冷却结束 → 进入半开，放行第一个探测请求（转换即消耗一次配额）
                    inner.state = CircuitState::HalfOpen { allowed: 1 };
                    inner.consecutive_failures = 0;
                    Ok(())
                } else {
                    let remaining = self.config.open_duration.saturating_sub(opened_at.elapsed());
                    Err(LlmError::CircuitOpen(format!(
                        "供应商 '{}' 熔断中，{}s 后重试",
                        self.name,
                        remaining.as_secs()
                    )))
                }
            }
            CircuitState::HalfOpen { allowed } => {
                if *allowed < self.config.half_open_max {
                    inner.state = CircuitState::HalfOpen { allowed: allowed + 1 };
                    Ok(())
                } else {
                    Err(LlmError::CircuitOpen(format!(
                        "供应商 '{}' 半开探测中，请稍后重试",
                        self.name
                    )))
                }
            }
        }
    }

    /// 调用成功：Closed 下重置连续失败计数；HalfOpen 下恢复 Closed。
    pub fn on_success(&self) {
        let mut inner = self.inner.lock().unwrap();
        match inner.state {
            CircuitState::Closed => inner.consecutive_failures = 0,
            CircuitState::HalfOpen { .. } => {
                inner.state = CircuitState::Closed;
                inner.consecutive_failures = 0;
            }
            CircuitState::Open { .. } => {} // Open 时不会放行请求
        }
    }

    /// 调用失败：累计连续失败，达到阈值 → Open；HalfOpen 下失败 → 重新 Open。
    pub fn on_failure(&self) {
        let mut inner = self.inner.lock().unwrap();
        match inner.state {
            CircuitState::Closed => {
                inner.consecutive_failures += 1;
                if inner.consecutive_failures >= self.config.failure_threshold {
                    inner.state = CircuitState::Open { opened_at: Instant::now() };
                }
            }
            CircuitState::HalfOpen { .. } => {
                inner.state = CircuitState::Open { opened_at: Instant::now() };
            }
            CircuitState::Open { .. } => {} // 保持打开，冷却由 allow 推进
        }
    }

    /// 当前状态（观测用）。
    pub fn state(&self) -> CircuitState {
        self.inner.lock().unwrap().state.clone()
    }
}
