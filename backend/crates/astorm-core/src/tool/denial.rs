//! 拒绝分类：安全拦截 / 资源限制的**面向应用**统一分类。
//!
//! 沙箱内部的 [`astorm_sandbox::sandbox::Category`] 是平台/策略维度的枚举
//! （`net_in` / `net_out` 分方向），本模块把它收敛为应用的 7 分类契约：
//! `file_read` / `file_write` / `danger_command` / `cwd_change` / `network` / `resource` / `browser`。
//! 其中文件、网络、资源、危险命令来自沙箱类别映射（进/出站合并为「网络」），
//! `cwd_change` 与 `browser` 来自应用自身的策略（bash 禁 cd / 浏览器自动化门禁），不经沙箱。
//!
//! 浏览器自动化相关拒绝（`Browser`）来自 browser 工具自身策略（Plan 模式拒写 / 未暴露的
//! 危险动作 / 域名门禁），不经沙箱，故 `DenialSource` 为静态预检。
//!
//! 每个拒绝同时带**来源**（`DenialSource`）——它表达分类的可信度：
//! 静态预检与沙箱事件是确定的；运行期内核拒绝（Seatbelt / AppContainer 不产生事件）
//! 只能靠 stderr 特征 + 静态分析归因，退出码归因同理，前端据此提示「疑似」。

use astorm_sandbox::sandbox::Category;
use serde::Serialize;
use serde_json::{json, Value};

/// 拒绝分类（应用契约，7 值；serde snake_case）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DenialCategory {
    /// 拒绝读：文件读被拒（空间外读 / 读黑名单）
    FileRead,
    /// 拒绝写：文件写被拒（空间外写 / 写黑名单 / 技能目录只读 / Plan 模式禁写）
    FileWrite,
    /// 危险命令：危险命令黑名单命中
    DangerCommand,
    /// 目录切换：命令里出现 cd / pushd / popd / chdir（硬规则：命令必须基于会话工作区执行，
    /// 需要子目录请用 bash 工具的 cwd 参数）。**单独分类**是因为它既不是「危险」也不是文件
    /// 拦截 —— 归到 danger_command 会被误读成危险命令黑名单命中。
    CwdChange,
    /// 网络：进/出站被拒（默认拒绝、黑名单、白名单未命中）
    Network,
    /// 资源限制：内存 / 磁盘 / CPU 时间 / 超时
    Resource,
    /// 浏览器自动化：网页写操作被拒（Plan 模式 / 危险动作 / 域名门禁）
    Browser,
    /// 邮件操作：发信 / 移动等被拦（Plan 模式 / 待用户确认）
    Mail,
}

impl DenialCategory {
    /// 稳定字符串（与 serde 序列化一致，用于日志 / 审计过滤）。
    pub fn as_str(&self) -> &'static str {
        match self {
            DenialCategory::FileRead => "file_read",
            DenialCategory::FileWrite => "file_write",
            DenialCategory::DangerCommand => "danger_command",
            DenialCategory::CwdChange => "cwd_change",
            DenialCategory::Network => "network",
            DenialCategory::Resource => "resource",
            DenialCategory::Browser => "browser",
            DenialCategory::Mail => "mail",
        }
    }
}

impl From<Category> for DenialCategory {
    fn from(c: Category) -> Self {
        match c {
            Category::FileRead => DenialCategory::FileRead,
            Category::FileWrite => DenialCategory::FileWrite,
            Category::DangerCommand => DenialCategory::DangerCommand,
            Category::NetIn | Category::NetOut => DenialCategory::Network,
            Category::Resource => DenialCategory::Resource,
        }
    }
}

/// 分类来源 / 可信度。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DenialSource {
    /// 执行前静态预检（解析命令 + 策略比对）：分类确定
    StaticPrecheck,
    /// 沙箱预检拦截（黑名单命中，命令未执行）：分类确定
    SandboxPreflight,
    /// 沙箱运行期结构化事件（危险命令实时终止 / 驱逐）：分类确定
    SandboxRuntime,
    /// 运行期内核拒绝的 stderr 归因（**历史来源**：内核文件拒绝不产生分类，现已不再产生
    /// 卡片——bash 改按普通失败返回；枚举保留以兼容历史 `metadata.interrupt`）
    StderrAttribution,
    /// 退出码归因（rlimit 信号无事件）：分类为推断
    ExitCode,
}

/// 一次拒绝的结构化描述。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Denial {
    pub category: DenialCategory,
    pub source: DenialSource,
    /// 人类可读明细（进审批卡 / 工具结果 / 审计）
    pub detail: String,
    /// 涉及目标（路径 / 主机；无则空）
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub targets: Vec<String>,
    /// 被拦操作原文（bash 命令 / file_operation 描述）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
}

impl Denial {
    pub fn new(category: DenialCategory, source: DenialSource, detail: impl Into<String>) -> Self {
        Denial { category, source, detail: detail.into(), targets: Vec::new(), command: None }
    }

    /// 附上涉及目标。
    pub fn with_targets(mut self, targets: Vec<String>) -> Self {
        self.targets = targets;
        self
    }

    /// 附上被拦操作原文。
    pub fn with_command(mut self, command: impl Into<String>) -> Self {
        self.command = Some(command.into());
        self
    }

    /// 结构化 JSON：`{category, source, detail, targets?, command?}`。
    pub fn to_json(&self) -> Value {
        serde_json::to_value(self).unwrap_or_else(|_| json!({}))
    }
}

/// 硬拒绝的**工具错误信封**（纯 JSON，作为 `Err` 原文返回）。
///
/// 与审批中断信封同构：工具产出纯 JSON，引擎不解释；`Tool::result_meta` 按 `denied=true`
/// 识别后转成 tool_result 事件的元数据（`denial` 字段）与消息 `metadata.denial`。
/// `message` 保留人类可读整句（模型/日志直接可读），`denial` 为结构化分类。
pub fn denial_error(denial: &Denial) -> String {
    json!({
        "denied": true,
        // 人类可读整句提到顶层：只读文本的一侧（模型 / 日志 / 旧前端）也能直接读懂
        "message": denial.detail,
        "denial": denial.to_json(),
    })
    .to_string()
}

/// 从工具**错误原文**识别拒绝信封 → 结果元数据 `{"status":"error","denial":{…}}`。
///
/// 严格纯 JSON（与 `interrupt_request_from_result` 同风格）：含引导文本或非 JSON 一律不识别。
pub fn denial_meta_from_error(err: &str) -> Option<Value> {
    let v: Value = serde_json::from_str(err.trim()).ok()?;
    if v.get("denied").and_then(|d| d.as_bool()) != Some(true) {
        return None;
    }
    let denial = v.get("denial")?.clone();
    Some(json!({ "status": "error", "denial": denial }))
}

// ============================ 测试 ============================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 沙箱类别收敛为应用分类() {
        assert_eq!(DenialCategory::from(Category::FileRead), DenialCategory::FileRead);
        assert_eq!(DenialCategory::from(Category::FileWrite), DenialCategory::FileWrite);
        assert_eq!(DenialCategory::from(Category::DangerCommand), DenialCategory::DangerCommand);
        assert_eq!(DenialCategory::from(Category::Resource), DenialCategory::Resource);
        // 进/出站合并为「网络」
        assert_eq!(DenialCategory::from(Category::NetIn), DenialCategory::Network);
        assert_eq!(DenialCategory::from(Category::NetOut), DenialCategory::Network);
    }

    #[test]
    fn json形状与稳定字符串() {
        let d = Denial::new(
            DenialCategory::FileWrite,
            DenialSource::StaticPrecheck,
            "命令试图写入会话空间外路径：/etc/x",
        )
        .with_targets(vec!["/etc/x".into()])
        .with_command("echo x > /etc/x");
        let v = d.to_json();
        assert_eq!(v["category"], json!("file_write"));
        assert_eq!(v["source"], json!("static_precheck"));
        assert_eq!(v["targets"], json!(["/etc/x"]));
        assert_eq!(v["command"], json!("echo x > /etc/x"));
        assert_eq!(DenialCategory::Resource.as_str(), "resource");
        assert_eq!(DenialCategory::Browser.as_str(), "browser");

        // 空 targets / 无 command 时字段缺省（不写空数组）
        let bare = Denial::new(DenialCategory::Network, DenialSource::SandboxRuntime, "x").to_json();
        assert!(bare.get("targets").is_none());
        assert!(bare.get("command").is_none());
    }
}
