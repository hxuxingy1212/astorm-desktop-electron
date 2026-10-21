//! `memory` 工具 + 会话侧注入的集成验证（真装配 + 真 Bundle 落盘）。
//!
//! 独立测试文件的原因同 `memory_auto_compile.rs`：`App::init` 是进程级单例。
//!
//! 覆盖：
//! 1. 工具三个只读动作（search / read / list）返回预期内容
//! 2. **越权读取被拒**：`read ../../etc/passwd` 必须报错（id 由模型填写，这是真实攻击面）
//! 3. 参数缺失 / 未知动作 → 明确报错；记忆未启用 → 可读说明（不是报错）
//! 4. 分类计数与列表筛选同口径（`concepts/` 被 fact/other 共用 + 存量 `type: Status`）
//! 5. `list` 的空结果给出**当前生效**的分类表（分类清单不写死在工具描述里）
//! 6. 注入门禁：`capability_line` / `digest_for` 在「无工具 / 未启用 / 关开关 / 条数 0 /
//!    无命中 / 空消息」六种情况下都必须不注入；有命中时内容含概念 id
//! 7. 索引缺失（子串兜底）与索引就绪（BM25）两条检索路径都给出一致结果

mod common;

use astorm_core::app::App;
use astorm_core::tool::memory::MemoryTool;
use futures::executor::block_on;
use react_loop::{Tool, ToolContext};
use serde_json::json;
use tokio_util::sync::CancellationToken;

use common::{boot, Fixture};

/// 走真实工具入口（`Tool::execute`）。
fn run(params: serde_json::Value) -> Result<String, String> {
    let ctx = ToolContext::new(CancellationToken::new()).with_session_id("test-session");
    block_on(MemoryTool.execute(params, &ctx))
}

#[test]
fn memory_tool_reads_and_injection_gates() {
    let f = Fixture::new("agent");
    let b = boot(&f, f.config_full(true, false, true, 5));
    let app = b.app;

    // 手工种两个概念（落盘形态与 `ConceptDocument::to_markdown` 一致）
    f.seed_concept(
        "decisions",
        "认证方案选型",
        "Decision",
        "认证方案用 JWT",
        "无状态鉴权选型，支持多端",
        "选择 JWT 而非 session，理由是支持多端无状态鉴权。",
    );
    f.seed_concept(
        "preferences",
        "中文回复",
        "Preference",
        "偏好中文回复",
        "用户要求始终用中文",
        "始终用中文回复，提交信息也用中文。",
    );

    // ---- 工具：list 的两级浏览（渐进式加载）----
    // 第一跳：不带 category ⇒ **目录页**（分类 + 计数 + 样例，不含描述）
    let out = run(json!({ "action": "list" })).expect("list 应成功");
    assert!(out.contains("长期记忆库共 2 个概念"), "{out}");
    assert!(out.contains("## decision（Decision）1"), "目录页应有分类与计数：{out}");
    assert!(out.contains("## preference（Preference）1"), "{out}");
    assert!(
        out.contains("## constraint（Constraint）0"),
        "**空分类也必须出现在目录页**（否则模型不知道分类表里有什么）：{out}"
    );
    assert!(out.contains("## fact（Fact）0") && out.contains("## other（Other）0"), "共用 concepts/ 的两类都要列：{out}");
    assert!(out.contains("- decisions/认证方案选型"), "目录页应给样例 id：{out}");
    assert!(
        !out.contains("无状态鉴权选型"),
        "目录页不该带描述（渐进式：描述留给分类页）：{out}"
    );
    // 目录页不受 limit 影响（它本身就是最省的那一跳）
    let out = run(json!({ "action": "list", "limit": 1 })).expect("list 应成功");
    assert!(out.contains("长期记忆库共 2 个概念"), "目录页不该被 limit 截断：{out}");
    // 第二跳：带 category ⇒ **分类页**（条目 + 描述 + 分组计数）
    let out = run(json!({ "action": "list", "category": "decision" })).expect("list 过滤应成功");
    assert!(out.contains("认证方案选型"), "{out}");
    assert!(out.contains("无状态鉴权选型"), "分类页应带描述：{out}");
    assert!(out.contains("## decision（Decision）1 条"), "分类页应给分组计数：{out}");
    assert!(!out.contains("中文回复"), "过滤后不该出现其他分类：{out}");
    // 目录名也是合法的过滤值（模型看到概念 id `decisions/…` 会自然这么传）
    let out = run(json!({ "action": "list", "category": "decisions" })).expect("目录名过滤应成功");
    assert!(out.contains("认证方案选型") && !out.contains("中文回复"), "{out}");
    // 分类过滤（用 type 名，大小写不敏感）
    let out = run(json!({ "action": "list", "category": "preference" })).expect("list 应成功");
    assert!(out.contains("中文回复") && !out.contains("认证方案选型"), "{out}");
    // 已删除的动作：status 必须被明确拒绝，而不是静默无输出
    let err = run(json!({ "action": "status" })).expect_err("status 动作已删除");
    assert!(err.contains("不支持的操作") && err.contains("list"), "{err}");

    // ---- 工具：search（此时索引未建 ⇒ 走子串兜底路径）----
    let out = run(json!({ "action": "search", "query": "JWT" })).expect("search 应成功");
    assert!(out.contains("decisions/认证方案选型"), "{out}");
    assert!(out.contains("Decision"), "摘要应带 type：{out}");
    let out = run(json!({ "action": "search", "query": "量子纠缠" })).expect("无命中也是成功");
    assert!(out.contains("没有检索到"), "{out}");

    // ---- 工具：read + 越权拒绝 ----
    let out = run(json!({ "action": "read", "concept_id": "decisions/认证方案选型" }))
        .expect("read 应成功");
    assert!(out.contains("选择 JWT 而非 session"), "{out}");
    assert!(out.contains("type: Decision"), "应带 frontmatter 供溯源：{out}");

    let err = run(json!({ "action": "read", "concept_id": "../../../etc/passwd" }))
        .expect_err("穿越 id 必须被拒绝");
    assert!(err.contains("相对路径段") || err.contains("越出"), "{err}");
    let err = run(json!({ "action": "read", "concept_id": "decisions/index" }))
        .expect_err("保留名必须被拒绝");
    assert!(err.contains("保留文件名"), "{err}");
    let err = run(json!({ "action": "read" })).expect_err("缺 concept_id 应报错");
    assert!(err.contains("concept_id"), "{err}");
    let err = run(json!({ "action": "search" })).expect_err("缺 query 应报错");
    assert!(err.contains("query"), "{err}");
    let err = run(json!({ "action": "写一条" })).expect_err("未知动作应报错");
    assert!(err.contains("不支持的操作"), "{err}");

    // ---- 注入门禁 ----
    let with_tool = vec!["memory".to_string(), "file_operation".to_string()];
    let without_tool = vec!["file_operation".to_string()];

    let line = app
        .memory()
        .capability_line(&with_tool)
        .expect("有工具+有概念应给能力行");
    assert!(line.contains("2 个概念") && line.contains("memory"), "{line}");
    assert!(
        app.memory().capability_line(&without_tool).is_none(),
        "会话没有 memory 工具时不该宣告能力"
    );

    let digest = app
        .memory()
        .digest_for("我们之前认证方案是怎么定的？", &with_tool)
        .expect("相关问题应命中并注入");
    assert!(digest.starts_with("【相关记忆】"), "{digest}");
    assert!(digest.contains("decisions/认证方案选型"), "{digest}");

    assert!(app.memory().digest_for("认证怎么选的", &without_tool).is_none(), "无工具不注入");
    assert!(app.memory().digest_for("量子纠缠实验", &with_tool).is_none(), "无命中不注入");
    assert!(app.memory().digest_for("   ", &with_tool).is_none(), "空消息不注入");

    // 关闭注入开关 → 不注入
    App::update_config(f.config_full(true, false, false, 5)).expect("热更失败");
    assert!(
        app.memory().digest_for("我们之前认证方案是怎么定的？", &with_tool).is_none(),
        "injectMemories=false 时不该注入"
    );
    // 条数 0 → 不注入
    App::update_config(f.config_full(true, false, true, 0)).expect("热更失败");
    assert!(
        app.memory().digest_for("我们之前认证方案是怎么定的？", &with_tool).is_none(),
        "injectedConcepts=0 时不该注入"
    );
    // 恢复注入，并验证「索引就绪（BM25）」路径给出同样结果
    App::update_config(f.config_full(true, false, true, 5)).expect("热更失败");
    let rebuilt = app.memory().reindex().expect("重建索引应成功");
    assert_eq!(rebuilt, 2, "应索引到 2 个概念");
    let out = run(json!({ "action": "search", "query": "JWT" })).expect("索引路径 search 应成功");
    assert!(out.contains("decisions/认证方案选型"), "{out}");
    let digest = app
        .memory()
        .digest_for("我们之前认证方案是怎么定的？", &with_tool)
        .expect("索引路径也应命中");
    assert!(digest.contains("decisions/认证方案选型"), "{digest}");

    // ---- 分类计数与列表筛选必须同口径 ----
    // `concepts/` 被 fact / other 共用（还有存量 type: Status 的老概念），
    // 若计数按 (目录, type) 精确聚合，这些概念会在 chip 上凭空消失，
    // 而列表仍按 category_id_of 把它们算进 fact —— 点进去条数对不上。
    App::update_config(f.config_full(true, false, true, 5)).expect("热更失败");
    f.seed_concept(
        "concepts",
        "构建脚本约定",
        "Other",
        "构建脚本约定",
        "杂项知识",
        "未归入以上分类的一条。",
    );
    f.seed_concept(
        "concepts",
        "旧版状态概念",
        "Status",
        "旧版状态概念",
        "存量 status 概念",
        "分类表已删 status，此条用于验证目录兜底。",
    );

    let status = app.memory().status().expect("status 应成功");
    let listed = app.memory().list_concepts().expect("list 应成功");
    assert_eq!(status.concept_total, 4, "共 4 条（含两条 concepts/）");

    // 列表侧按 category_id 分组 (列表页筛选就是按这个字段)
    let mut by_category: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for c in &listed {
        *by_category.entry(c.category_id.as_str()).or_insert(0) += 1;
    }
    // chip 计数必须与之一致
    for stat in &status.categories {
        assert_eq!(
            stat.count,
            by_category.get(stat.id.as_str()).copied().unwrap_or(0),
            "分类 {} 的 chip 计数与列表分组不一致",
            stat.id
        );
    }
    let count_of = |id: &str| {
        status
            .categories
            .iter()
            .find(|c| c.id == id)
            .map(|c| c.count)
            .unwrap_or(0)
    };
    assert_eq!(count_of("other"), 1, "concepts/ 下的 Other 应归 other");
    assert_eq!(
        count_of("fact"),
        1,
        "存量 type: Status 落不到任何分类，按目录兜底归 fact（而不是消失）"
    );
    // 徽标仍显示真实 type（存量概念不被改写）
    let legacy = listed
        .iter()
        .find(|c| c.concept_id.ends_with("旧版状态概念"))
        .expect("列表应含存量概念");
    assert_eq!(legacy.type_name, "Status");
    assert_eq!(legacy.category_id, "fact");

    // ---- list 空结果必须给出**当前生效**的分类 id ----
    // 分类表是配置驱动的，把清单写死在工具描述里必然漂移（加 procedure / 删 status 后
    // 那份硬编码清单会指着一个不存在的分类让模型去查）。
    // 先用**不存在的**分类探空结果：
    let out = run(json!({ "action": "list", "category": "runbook" })).expect("未知分类也是成功");
    assert!(out.contains("没有记忆概念"), "{out}");
    let listed = out
        .split("当前分类：")
        .nth(1)
        .expect("空结果应列出当前分类")
        .split('（')
        .next()
        .unwrap()
        .trim()
        .to_string();
    let expected: Vec<String> = astorm_memory::config::MemoryConfig::default_categories()
        .iter()
        .map(|c| c.id.clone())
        .collect();
    assert_eq!(listed, expected.join(" / "), "列出的分类应与配置里的分类表完全一致");
    // 而 `status` 虽已不是分类，仍能按 **type 名**筛到存量概念（老概念不被改写也不失联）
    let out = run(json!({ "action": "list", "category": "status" })).expect("type 名过滤应成功");
    assert!(out.contains("旧版状态概念"), "{out}");

    // ---- 记忆未启用 → 工具给可读说明而非报错 ----
    App::update_config(f.config_full(false, false, true, 5)).expect("热更失败");
    let out = run(json!({ "action": "list" })).expect("未启用时仍是「成功」");
    assert!(out.contains("未启用"), "{out}");
    assert!(
        app.memory().capability_line(&with_tool).is_none()
            && app.memory().digest_for("认证", &with_tool).is_none(),
        "记忆未启用时不该注入任何东西"
    );

    app.shutdown();
}
