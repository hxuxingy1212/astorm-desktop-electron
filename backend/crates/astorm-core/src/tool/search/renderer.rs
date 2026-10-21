//! 页面渲染服务：**复用内置 Chromium（Electron）的常驻隐藏窗口**渲染网页，
//! 供 `websearch`（百度/谷歌）与 `webfetch` 共用。
//!
//! 为什么常驻：每次调用都冷启动一个 Electron 进程代价高（数百 ms~2s + 进程内存），
//! 且并发/回退链会放大。这里维护**单实例服务**，通过 stdin/stdout 行分隔 JSON 收发：
//!   → `{"id":1,"mode":"html|text","url":"https://…"}`
//!   ← `{"id":1,"ok":true,"data":"…"}` / `{"id":1,"ok":false,"error":"…"}`
//!
//! 治理：
//! - **预热**：`warm()` 提前拉起（窗口在服务端预建）；
//! - **并发上限 + 排队**：`Semaphore(concurrency)`，超过 `concurrency + queue_max` 直接拒绝；
//! - **节流**：每次渲染后至少间隔 `min_interval_ms`，降低被反爬概率；
//! - **空闲回收**：空闲超过 `idle_secs` 退出释放内存（0 = 常驻），下次请求再拉起；
//! - **自愈**：服务退出（stdout EOF）→ 置空并在下次请求重启；写失败同理；
//! - **进程兜底**：`kill_on_drop(true)`，Service 被 drop 即杀进程（修此前超时孤儿泄漏）。

use std::collections::HashMap;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{oneshot, Semaphore};

use crate::app::config::WebRenderConfig;

/// 单个请求发往服务的行。
#[derive(Serialize)]
struct Req<'a> {
    id: u64,
    mode: &'a str,
    url: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    selector: Option<&'a str>,
}

/// 服务返回的行（`ready` 为启动握手行）。
#[derive(Deserialize)]
struct Resp {
    #[serde(default)]
    id: u64,
    #[serde(default)]
    ready: bool,
    #[serde(default)]
    ok: bool,
    #[serde(default)]
    data: Option<String>,
    #[serde(default)]
    error: Option<String>,
}

/// 运行中的渲染服务进程。
struct Service {
    /// 保持 Child 存活：`kill_on_drop(true)` 使其在本结构被 drop 时被杀（兜底）。
    _child: Child,
    stdin: ChildStdin,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<Result<String, String>>>>>,
    /// 代次：退出/重启时避免旧读线程清掉新服务。
    generation: u64,
}

/// 页面渲染服务（AppState 持有单例）。
pub struct WebRender {
    cfg: Arc<RwLock<WebRenderConfig>>,
    svc: Arc<tokio::sync::Mutex<Option<Service>>>,
    sem: Arc<Semaphore>,
    inflight: AtomicUsize,
    next_id: AtomicU64,
    last_used: Arc<Mutex<Instant>>,
    generation: Arc<AtomicU64>,
    rt: tokio::runtime::Handle,
}

impl WebRender {
    /// 创建服务（不立即拉起；由 `warm()` 或首个请求触发）。内部启动空闲回收任务。
    pub fn new(cfg: Arc<RwLock<WebRenderConfig>>, rt: tokio::runtime::Handle) -> Arc<Self> {
        let concurrency = cfg.read().map(|c| c.concurrency).unwrap_or(2).max(1);
        let me = Arc::new(Self {
            cfg,
            svc: Arc::new(tokio::sync::Mutex::new(None)),
            sem: Arc::new(Semaphore::new(concurrency)),
            inflight: AtomicUsize::new(0),
            next_id: AtomicU64::new(1),
            last_used: Arc::new(Mutex::new(Instant::now())),
            generation: Arc::new(AtomicU64::new(0)),
            rt,
        });
        me.clone().spawn_reaper();
        me
    }

    /// 预热：后台拉起服务（不阻塞）。
    pub fn warm(self: &Arc<Self>) {
        let me = self.clone();
        self.rt.spawn(async move {
            let mut guard = me.svc.lock().await;
            if let Err(e) = me.ensure(&mut guard).await {
                tracing::debug!("[render] 预热失败: {e}");
            }
        });
    }

    /// 配置热更：若 bin/script/profile 变化则重启（下次请求生效），并刷新节流/空闲参数。
    pub fn on_config_changed(self: &Arc<Self>) {
        let me = self.clone();
        self.rt.spawn(async move {
            // 简单起见：配置变更后直接停止当前服务，下次请求按新配置重启
            let mut guard = me.svc.lock().await;
            if guard.take().is_some() {
                tracing::info!("[render] 配置变更，渲染服务将按新配置重启");
            }
        });
    }

    /// 关闭：停止服务（drop Service 即触发 kill_on_drop）。
    pub async fn shutdown(&self) {
        let mut guard = self.svc.lock().await;
        *guard = None;
    }

    /// 渲染一个 URL：`mode` = `html`（outerHTML）/ `text`（可读正文）/ `markdown` / `links`。
    pub async fn render(&self, mode: &str, url: &str, timeout: Duration) -> Result<String, String> {
        self.render_ex(mode, url, None, timeout).await
    }

    /// 同 `render`，但可指定 `selector` 把处理范围收窄到某个 CSS 选择器（跳过导航/侧栏/页脚）。
    pub async fn render_ex(
        &self,
        mode: &str,
        url: &str,
        selector: Option<&str>,
        timeout: Duration,
    ) -> Result<String, String> {
        let (concurrency, queue_max, min_interval_ms) = {
            let c = self.cfg.read().map_err(|_| "渲染配置锁中毒".to_string())?;
            (c.concurrency.max(1), c.queue_max, c.min_interval_ms)
        };

        // 排队上限（含执行中）
        let cur = self.inflight.fetch_add(1, Ordering::SeqCst) + 1;
        if cur > concurrency + queue_max {
            self.inflight.fetch_sub(1, Ordering::SeqCst);
            return Err("渲染队列已满，请稍后重试".into());
        }

        // 并发闸门（等待也计入整体 timeout）
        let permit = match tokio::time::timeout(timeout, self.sem.clone().acquire_owned()).await {
            Ok(Ok(p)) => p,
            _ => {
                self.inflight.fetch_sub(1, Ordering::SeqCst);
                return Err("渲染繁忙（排队超时）".into());
            }
        };

        let started = Instant::now();
        let result = self.render_inner(mode, url, selector, timeout).await;

        // 节流：距上次渲染不足 `min_interval_ms` 则补足（降低被反爬概率）
        let elapsed = started.elapsed();
        let min = Duration::from_millis(min_interval_ms);
        if elapsed < min {
            tokio::time::sleep(min - elapsed).await;
        }
        if let Ok(mut t) = self.last_used.lock() {
            *t = Instant::now();
        }
        drop(permit);
        self.inflight.fetch_sub(1, Ordering::SeqCst);
        result
    }

    async fn render_inner(
        &self,
        mode: &str,
        url: &str,
        selector: Option<&str>,
        timeout: Duration,
    ) -> Result<String, String> {
        let mut guard = self.svc.lock().await;
        self.ensure(&mut guard).await?;
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel::<Result<String, String>>();
        let line = {
            let svc = guard.as_ref().ok_or("渲染服务未启动")?;
            svc.pending.lock().map_err(|_| "渲染待决表锁中毒".to_string())?.insert(id, tx);
            serde_json::to_string(&Req { id, mode, url, selector }).map_err(|e| e.to_string())? + "\n"
        };
        // 写请求
        {
            let svc = guard.as_mut().ok_or("渲染服务未启动")?;
            if let Err(e) = svc.stdin.write_all(line.as_bytes()).await {
                // 写失败：服务已死，置空（drop 触发 kill_on_drop）
                let dead = guard.take();
                if let Some(s) = dead {
                    if let Ok(mut p) = s.pending.lock() {
                        p.remove(&id);
                    }
                }
                return Err(format!("渲染服务写入失败: {e}"));
            }
        }
        // 释放服务锁后再等待：允许多个请求并发（服务端窗口池按 concurrency 处理）
        drop(guard);
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(Ok(data))) => Ok(data),
            Ok(Ok(Err(e))) => Err(e),
            Ok(Err(_)) => Err("渲染服务无响应".into()),
            Err(_) => {
                // 请求超时：只丢弃该待决项，服务整体继续（窗口由服务端自行恢复）
                let g = self.svc.lock().await;
                if let Some(svc) = g.as_ref() {
                    if let Ok(mut p) = svc.pending.lock() {
                        p.remove(&id);
                    }
                }
                Err("渲染超时".into())
            }
        }
    }

    /// 确保服务已启动（`guard` 为 None 时拉起并接好读线程）。
    async fn ensure(&self, guard: &mut Option<Service>) -> Result<(), String> {
        if guard.is_some() {
            return Ok(());
        }
        let (bin, script, concurrency, profile) = {
            let c = self.cfg.read().map_err(|_| "渲染配置锁中毒".to_string())?;
            (
                c.bin.clone().filter(|s| !s.trim().is_empty()),
                c.script.clone().filter(|s| !s.trim().is_empty()),
                c.concurrency.max(1),
                c.profile_dir.clone().filter(|s| !s.trim().is_empty()),
            )
        };
        let bin = bin.ok_or("未配置渲染器可执行文件（webRender.bin）")?;
        let script = script.ok_or("未配置渲染器脚本（webRender.script）")?;

        let mut cmd = Command::new(&bin);
        cmd.arg(&script)
            .arg("--concurrency")
            .arg(concurrency.to_string());
        if let Some(dir) = profile {
            cmd.arg("--profile").arg(dir);
        }
        cmd.env_remove("ELECTRON_RUN_AS_NODE")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        // bin 通常为 Electron(GUI 子系统,本就不弹窗),但配置指向 node.exe 等控制台程序时
        // GUI 宿主拉起会闪黑窗,统一静默兜底
        #[cfg(windows)]
        cmd.creation_flags(0x0800_0000);

        let mut child = cmd.spawn().map_err(|e| format!("启动渲染服务失败: {e}"))?;
        let stdin = child.stdin.take().ok_or("无法获取渲染服务 stdin")?;
        let stdout = child.stdout.take().ok_or("无法获取渲染服务 stdout")?;
        let stderr = child.stderr.take().ok_or("无法获取渲染服务 stderr")?;
        let pending: Arc<Mutex<HashMap<u64, oneshot::Sender<Result<String, String>>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;

        // stdout 读线程：按 id 派发；EOF = 服务退出 → 置空并失败所有待决
        {
            let pending = pending.clone();
            let svc = self.svc.clone();
            let generation_id = generation;
            tokio::spawn(async move {
                let mut lines = BufReader::new(stdout).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    let Ok(resp) = serde_json::from_str::<Resp>(&line) else {
                        continue;
                    };
                    if resp.ready {
                        tracing::debug!("[render] 服务就绪");
                        continue;
                    }
                    let Some(tx) = pending.lock().ok().and_then(|mut p| p.remove(&resp.id)) else {
                        continue;
                    };
                    let r = if resp.ok {
                        Ok(resp.data.unwrap_or_default())
                    } else {
                        Err(resp.error.unwrap_or_else(|| "渲染失败".into()))
                    };
                    let _ = tx.send(r);
                }
                // EOF：服务退出
                if let Ok(mut g) = svc.try_lock() {
                    if g.as_ref().map(|s| s.generation) == Some(generation_id) {
                        *g = None;
                    }
                }
                if let Ok(mut p) = pending.lock() {
                    for (_, tx) in p.drain() {
                        let _ = tx.send(Err("渲染服务已退出".into()));
                    }
                }
            });
        }
        // stderr 收日志
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                tracing::debug!("[render] {line}");
            }
        });

        *guard = Some(Service { _child: child, stdin, pending, generation });
        Ok(())
    }

    /// 空闲回收：周期检查，空闲超过 `idle_secs`（>0）则退出服务。
    fn spawn_reaper(self: Arc<Self>) {
        let rt = self.rt.clone();
        rt.spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(15)).await;
                let idle = self.cfg.read().map(|c| c.idle_secs).unwrap_or(300);
                if idle == 0 {
                    continue;
                }
                let idle_for = self
                    .last_used
                    .lock()
                    .map(|t| t.elapsed())
                    .unwrap_or_default();
                let busy = self.inflight.load(Ordering::SeqCst) > 0;
                if !busy && idle_for >= Duration::from_secs(idle) {
                    let mut g = self.svc.lock().await;
                    if g.take().is_some() {
                        tracing::info!("[render] 空闲 {idle}s，已回收渲染服务");
                    }
                }
            }
        });
    }
}
