//! 长期记忆**自动编译钩子**的集成验证（真装配：`App::init` + StorageManager + 真管道）。
//!
//! 为什么单独一个测试文件：`App::init` 是**进程级单例**（`OnceLock`），同一个测试二进制里
//! 只能初始化一次；独立文件的测试目标是独立进程，互不干扰。共用脚手架见 `common/mod.rs`。
//!
//! 覆盖：
//! 1. `autoCompile` **关闭**时钩子是空操作 —— 不建 Bundle、不落盘、不发事件
//! 2. 打开后钩子真的驱动管道 —— 会话被归一化落进 `raw/sessions/`，且事件流有 `started`
//! 3. **子会话不进记忆** —— 批次层面排除 + 收尾调用不唤醒编译
//! 4. 跑完后 `running` 复位、失败会话仍待编译（可重试）
//!
//! LLM 用指向**本机不可达端口**的假供应商：提取必然失败，但 `raw/` 落盘发生在提取**之前**，
//! 于是「钩子是否真的驱动了管道」完全可观测，且全程零真实 API 额度消耗。

mod common;

use std::time::Duration;

use astorm_common::types::record::SessionType;
use astorm_common::types::Storage;
use astorm_core::app::App;

use common::{boot, record, take_memory_events, wait_until, Fixture, CHILD_SID, NORMAL_SID};

#[test]
fn auto_compile_hook_switches_and_targets() {
    let f = Fixture::new("hook");
    f.seed_sessions();

    // 存储句柄同时给应用与测试用：这样无需为测试暴露 AppState 内部字段，
    // 也能把「子会话」记录写进同一份 sqlite（钩子按 session_type 判定跳过）。
    let b = boot(&f, f.config_full(true, false, true, 5));
    b.storage
        .upsert_session(&record(CHILD_SID, SessionType::Child))
        .expect("写入子会话记录失败");
    let app = b.app;
    let mut rx = app.subscribe();

    // ---- 阶段 1：autoCompile 关闭 ⇒ 空操作 ----
    app.memory().auto_compile_after_session(NORMAL_SID);
    std::thread::sleep(Duration::from_millis(500));
    assert!(
        !f.bundle().exists(),
        "autoCompile 关闭时不该建 Bundle：{}",
        f.bundle().display()
    );
    assert!(
        take_memory_events(&mut rx).is_empty(),
        "autoCompile 关闭时不该有任何记忆事件"
    );

    // ---- 阶段 2：热更打开 ⇒ 钩子驱动管道 ----
    App::update_config(f.config_full(true, true, true, 5)).expect("热更配置失败");
    assert!(
        app.memory_config().is_some_and(|c| c.auto_compile),
        "热更应把 autoCompile 写入 memory_cfg"
    );
    app.memory().auto_compile_after_session(NORMAL_SID);

    assert!(
        wait_until(Duration::from_secs(30), || !f.raw_stems().is_empty()),
        "打开 autoCompile 后钩子应驱动管道，落出 raw 会话"
    );
    let stems = f.raw_stems();
    assert_eq!(stems.len(), 1, "只该有普通会话那一条：{stems:?}");
    assert!(
        stems[0].contains("11111111"),
        "raw 文件应来自普通会话：{stems:?}"
    );
    let raw = std::fs::read_to_string(f.bundle().join(format!("raw/sessions/{}.md", stems[0])))
        .expect("读取 raw 失败");
    assert!(raw.contains("JWT"), "raw 应含归一化后的会话正文：{raw}");
    assert!(raw.contains("session_id: "), "raw frontmatter 应带 session_id：{raw}");

    // 事件流：至少要有 started（证明钩子走的是同一条带进度的管道）
    let events = take_memory_events(&mut rx);
    assert!(
        events
            .iter()
            .any(|e| e.get("kind").and_then(|k| k.as_str()) == Some("started")),
        "应有 started 事件：{events:?}"
    );

    // ---- 阶段 3：子会话不进记忆 ----
    // ① 批次层面：上一步那次编译遍历整个会话目录，子会话**不该**被带进来
    let stems = f.raw_stems();
    assert_eq!(
        stems.len(),
        1,
        "子会话不该随批次进入 raw（来源集合层面过滤）：{stems:?}"
    );
    // ② 触发层面：子会话收尾调用钩子应完全不唤醒编译 —— 连事件都不该有
    //    （只断言"没多出文件"太弱：跑起来但无新会话也会满足它）
    assert!(
        wait_until(Duration::from_secs(30), || !app.memory().is_running()),
        "上一轮编译应已收尾"
    );
    let _ = take_memory_events(&mut rx); // 清掉上一轮残留
    app.memory().auto_compile_after_session(CHILD_SID);
    std::thread::sleep(Duration::from_millis(800));
    let events = take_memory_events(&mut rx);
    assert!(
        events.is_empty(),
        "子会话收尾不该唤醒编译（事件流应静默）：{events:?}"
    );
    assert_eq!(
        f.raw_stems().len(),
        1,
        "子会话收尾后 raw 不该新增：{:?}",
        f.raw_stems()
    );

    // ---- 阶段 4：收尾状态一致 ----
    assert!(!app.memory().is_running(), "跑完后 running 应复位");
    let status = app.memory().status().expect("读状态失败");
    assert_eq!(
        status.pending, 1,
        "只有普通会话进入来源集合；假供应商必然失败 ⇒ 它仍未编译，应待编译：{status:?}"
    );
    assert_eq!(status.concept_total, 0, "没有任何概念产出：{status:?}");

    app.shutdown();
}
