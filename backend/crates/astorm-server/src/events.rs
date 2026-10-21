//! 事件中枢：常驻订阅 core 广播总线 + 每会话环形缓冲 + SSE 直播/回放。
//!
//! 为什么需要它：core 的事件总线是 `tokio::broadcast`，**无重放**（容量 4096，超出
//! 即丢给最慢订阅者）。桌面版约定"先订阅再建会话"所以没问题；HTTP 下客户端必然是
//! 「POST 建会话 → 再开 SSE」，中间的事件只能丢。本模块在服务端启动时就常驻订阅，
//! 给每条事件分配**单调递增 seq**，按 `session_id` 分桶存环形缓冲；SSE 连接先回放
//! `?after=` 之后的缓冲，再续播直播流，并按 seq 去重——语义上等价于一条可断线续传
//! 的事件流（SSE `id:` / `Last-Event-ID` 同款机制）。

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard};

use astorm_core::app::AppState;
use serde_json::Value;
use tokio::sync::broadcast;
use tokio::sync::broadcast::error::RecvError;

/// 缓冲条目：服务端 seq + 原始事件 JSON。
pub type SeqEvent = (u64, Value);

struct SessionChannel {
    ring: VecDeque<SeqEvent>,
    tx: broadcast::Sender<SeqEvent>,
}

struct Inner {
    next_seq: u64,
    global: VecDeque<SeqEvent>,
    sessions: HashMap<String, SessionChannel>,
    /// 全局直播流（含所有会话 + llm + 系统事件）
    global_tx: broadcast::Sender<SeqEvent>,
}

impl Inner {
    fn new() -> Self {
        // seq 起点取启动时刻的 unix 毫秒：进程重启后新 seq 恒大于旧进程发过的任何
        // seq——旧页面重连带 `after=<旧高水位>` 不会把新事件全部过滤掉
        // （broadcast 无重放 + 内存缓冲都是进程态，跨重启的连续性以毫秒起点近似）。
        let boot = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        Inner {
            next_seq: boot,
            global: VecDeque::new(),
            sessions: HashMap::new(),
            global_tx: broadcast::channel(1024).0,
        }
    }
}

/// 事件中枢（进程内单例，由 [`EventHub::start`] 拉起泵任务）。
pub struct EventHub {
    app: Arc<AppState>,
    capacity: usize,
    inner: Mutex<Inner>,
}

impl EventHub {
    /// 创建中枢并启动常驻泵任务（挂在应用 runtime 上）。
    pub fn start(app: Arc<AppState>, capacity: usize) -> Arc<Self> {
        let capacity = capacity.max(1);
        let hub = Arc::new(Self {
            app,
            capacity,
            inner: Mutex::new(Inner::new()),
        });
        let pump = hub.clone();
        hub.app.handle().spawn(async move {
            pump.pump().await;
        });
        hub
    }

    /// 常驻泵：core 总线 → seq 分配 → 环形缓冲 + 直播扇出。
    async fn pump(self: Arc<Self>) {
        let mut rx = self.app.subscribe();
        loop {
            match rx.recv().await {
                Ok(event) => self.record(event),
                // 最慢订阅者丢事件：中枢消费极快（入队+扇出），lag 视为系统过载，继续即可
                Err(RecvError::Lagged(n)) => {
                    tracing::warn!("[events] 中枢落后 {n} 条事件，环形缓冲可能出现空洞");
                }
                Err(RecvError::Closed) => break,
            }
        }
    }

    /// 记录一条事件：分配 seq → 推环形缓冲 → 扇出直播。
    /// `broadcast::send` 非阻塞且不 await，临界区极短，可安全持 std 锁完成。
    fn record(&self, event: Value) {
        let sid = event
            .get("session_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let mut inner = self.lock();
        inner.next_seq += 1;
        let seq = inner.next_seq;

        push_ring(&mut inner.global, (seq, event.clone()), self.capacity);
        let _ = inner.global_tx.send((seq, event.clone()));

        if !sid.is_empty() {
            let entry = inner.sessions.entry(sid).or_insert_with(|| SessionChannel {
                ring: VecDeque::new(),
                tx: broadcast::channel(1024).0,
            });
            push_ring(&mut entry.ring, (seq, event.clone()), self.capacity);
            let _ = entry.tx.send((seq, event));
        }
    }

    /// 全局直播订阅。
    pub fn subscribe_global(&self) -> broadcast::Receiver<SeqEvent> {
        self.lock().global_tx.subscribe()
    }

    /// 全局回放。
    pub fn replay_global(&self) -> Vec<SeqEvent> {
        self.lock().global.iter().cloned().collect()
    }

    /// 会话直播订阅（通道不存在则创建）。
    pub fn subscribe_session(&self, session_id: &str) -> broadcast::Receiver<SeqEvent> {
        let mut inner = self.lock();
        inner
            .sessions
            .entry(session_id.to_string())
            .or_insert_with(|| SessionChannel {
                ring: VecDeque::new(),
                tx: broadcast::channel(1024).0,
            })
            .tx
            .subscribe()
    }

    /// 会话回放。
    pub fn replay_session(&self, session_id: &str) -> Vec<SeqEvent> {
        self.lock()
            .sessions
            .get(session_id)
            .map(|c| c.ring.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// 会话删除后清理缓冲与扇出通道。
    pub fn forget_session(&self, session_id: &str) {
        self.lock().sessions.remove(session_id);
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }
}

/// 环形缓冲推入（超容量丢最旧）。
fn push_ring(ring: &mut VecDeque<SeqEvent>, item: SeqEvent, capacity: usize) {
    if ring.len() >= capacity {
        ring.pop_front();
    }
    ring.push_back(item);
}

/// SSE 流类型别名（keep_alive 会把流包进 KeepAliveStream，故按包装后类型命名）。
pub type SseStream = axum::response::Sse<
    axum::response::sse::KeepAliveStream<
        tokio_stream::wrappers::ReceiverStream<
            Result<axum::response::sse::Event, std::convert::Infallible>,
        >,
    >,
>;

/// 把「回放 + 直播」装成一个 SSE 事件流。
///
/// 顺序保证：先订阅直播通道（订阅点之后的进缓冲），再取回放快照并逐条发送
/// （记录已发送的最大 seq），最后消费直播缓冲，跳过 `seq <= 已发送` 的重复项。
/// 回放窗口内的短暂乱序可接受（桌面版事件本就无序号语义）。
pub fn sse_stream(
    live: broadcast::Receiver<SeqEvent>,
    backlog: Vec<SeqEvent>,
    after: u64,
    shutdown: tokio::sync::watch::Receiver<bool>,
) -> SseStream {
    use axum::response::sse::{Event, KeepAlive};
    use tokio_stream::wrappers::ReceiverStream;

    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Event, std::convert::Infallible>>(256);
    tokio::spawn(async move {
        let mut last = after;
        // 1) 回放
        for (seq, ev) in backlog {
            if seq > last {
                last = seq;
                if tx.send(Ok(to_sse_event(seq, ev))).await.is_err() {
                    return; // 客户端断开
                }
            }
        }
        // 2) 直播（去重 + shutdown 掐断）
        let mut live = live;
        let mut shutdown = shutdown;
        loop {
            tokio::select! {
                item = live.recv() => match item {
                    Ok((seq, ev)) => {
                        if seq > last {
                            last = seq;
                            if tx.send(Ok(to_sse_event(seq, ev))).await.is_err() {
                                return;
                            }
                        }
                    }
                    Err(RecvError::Lagged(_)) => continue,
                    Err(RecvError::Closed) => break,
                },
                _ = tx.closed() => return,
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        let _ = tx.send(Ok(Event::default().event("shutdown"))).await;
                        break;
                    }
                }
            }
        }
    });
    axum::response::Sse::new(ReceiverStream::new(rx)).keep_alive(
        KeepAlive::new()
            .interval(std::time::Duration::from_secs(15))
            .text("ping"),
    )
}

/// 单条事件 → SSE 帧（`id:` 为服务端 seq，供断线续传；`event:` 为业务类型）。
fn to_sse_event(seq: u64, ev: Value) -> axum::response::sse::Event {
    use axum::response::sse::Event;
    let kind = ev.get("type").and_then(|v| v.as_str()).unwrap_or("event").to_string();
    Event::default()
        .id(seq.to_string())
        .event(kind)
        .data(ev.to_string())
}
