//! 安全配置辅助：内置危险命令清单查询 + 默认配置导出 + 沙箱工具链注入报告
//! + 策略字段级强制力报告。

use napi_derive::napi;
use serde::Serialize;
use serde_json::Value;
use std::path::PathBuf;

use astorm_sandbox::policy::{
    DangerPolicyCfg, FilePolicyCfg, NetPolicyCfg, ProtectionConfig, ResolvedPolicy, ResourceLimits,
    SandboxInput,
};
use astorm_sandbox::toolchain;

/// 内置危险命令正则清单（前端安全配置页展示，可编辑为用户副本保存）。
#[napi]
pub fn builtin_danger_patterns() -> Vec<String> {
    astorm_core::tool::policy::builtin_danger_patterns()
}

/// 安全配置默认值（camelCase JSON，与后端 `SecurityConfig::default` 同源）。
///
/// 默认值里有非空项（凭据路径读写黑名单，见 `config::credential_paths`），前端各自的
/// 常量兜底会与后端漂移；设置页据此作「未配置时」的初值底座（已保存配置叠加其上、
/// 优先），保存回传的也是同一份清单——避免「设置页用空表占位，保存一次就把默认清单清掉」。
#[napi]
pub fn security_defaults() -> napi::Result<Value> {
    serde_json::to_value(astorm_core::app::config::SecurityConfig::default())
        .map_err(|e| napi::Error::from_reason(format!("序列化安全配置默认值失败: {e}")))
}

/// 单个工具的沙箱注入状态（「本地环境」卡片展开区展示）。
#[napi(object)]
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolchainAllowanceTool {
    /// 环境显示名（与 localenv 探测一致）
    pub name: String,
    /// 工具 bin 名（toolchain_disabled 列表的键）
    pub bin: String,
    /// 是否检测到（可执行文件存在）
    pub available: bool,
    /// 是否已被用户禁用注入
    pub disabled: bool,
    /// PATH 命中的真实所在目录（canonicalize；未检测到为 null）
    pub resolved_dir: Option<String>,
    /// 附加注入目录（rustup 真实工具链等；无则空）
    pub extra_dirs: Vec<String>,
    /// 禁用该工具后其目录是否仍被其他启用工具共享留在白名单
    /// （如 Homebrew 整前缀 / rustup 生态需相关工具全部禁用才移除）。
    pub shared_dir: bool,
}

/// 沙箱自动注入白名单报告（本地环境页 + 安全配置只读展示共用）。
#[napi(object)]
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolchainAllowanceReport {
    /// 全部探测工具的注入状态（含未安装项）
    pub tools: Vec<ToolchainAllowanceTool>,
    /// 当前生效的 exec 放行目录（过滤禁用 + Cellar 前缀合并后）
    pub exec_dirs: Vec<String>,
    /// 当前生效的只读放行 globs
    pub read_globs: Vec<String>,
    /// 系统技能目录（固定只读注入；App 未初始化为 null）
    pub skills_dir: Option<String>,
}

/// 沙箱工具链注入报告：按当前安全配置（含 toolchain_disabled）计算生效注入。
/// 「本地环境」页注入开关与「安全配置」页只读展示共用此数据源。
#[napi]
pub fn toolchain_allowances() -> ToolchainAllowanceReport {
    // 当前安全配置快照（含本地环境页维护的禁用列表；App 未初始化回退默认）
    let disabled: Vec<String> = astorm_core::app::App::try_global()
        .map(|app| app.security_config().toolchain_disabled)
        .unwrap_or_default();

    // 每 probe 的解析目录（含 rustup 附加归属）
    let entries = toolchain::resolved_entries();
    let tools = toolchain::TOOLCHAIN_PROBES
        .iter()
        .map(|p| {
            let resolved_dir = toolchain::which_path(p.bin)
                .and_then(|path| std::fs::canonicalize(&path).ok())
                .and_then(|real| real.parent().map(|d| d.to_string_lossy().into_owned()));
            // 该 bin 的附加目录（rustup toolchains 归属 cargo/rustc）
            let extra_dirs: Vec<String> = entries
                .iter()
                .filter(|e| e.bin == p.bin)
                .filter(|e| {
                    // 附加目录 = 非 PATH 直接解析所得（PATH 命中目录外的注入）
                    Some(e.dir.to_string_lossy().into_owned()) != resolved_dir
                })
                .map(|e| e.dir.to_string_lossy().into_owned())
                .collect();
            let disabled_flag = disabled.iter().any(|d| d == p.bin);
            // 单独禁用该 bin 后的 exec 放行目录（用于判定「共享」：禁用后其目录仍被
            // 其他启用工具保留，点击开关后提示无法立即移除注入）。
            let mut with_bin = disabled.clone();
            if !with_bin.iter().any(|d| d == p.bin) {
                with_bin.push(p.bin.to_string());
            }
            let alone = toolchain::exec_allow_dirs(&with_bin);
            let own_dirs: Vec<PathBuf> = resolved_dir
                .as_ref()
                .map(PathBuf::from)
                .into_iter()
                .chain(extra_dirs.iter().map(PathBuf::from))
                .collect();
            let shared_dir = own_dirs.iter().any(|own| {
                alone.iter()
                    .any(|d| own.starts_with(d) || d.starts_with(own))
            });
            ToolchainAllowanceTool {
                name: p.name.to_string(),
                bin: p.bin.to_string(),
                available: resolved_dir.is_some() || !extra_dirs.is_empty(),
                disabled: disabled_flag,
                resolved_dir,
                extra_dirs,
                shared_dir,
            }
        })
        .collect();

    ToolchainAllowanceReport {
        tools,
        exec_dirs: toolchain::exec_allow_dirs(&disabled)
            .iter()
            .map(|d| d.to_string_lossy().into_owned())
            .collect(),
        read_globs: toolchain::read_allow_globs(&disabled),
        skills_dir: astorm_core::app::App::try_global()
            .map(|app| app.dir().skills_dir.to_string_lossy().into_owned()),
    }
}

/// 单个策略字段的强制力（设置页「安全配置 → 策略强制力」展示）。
#[napi(object)]
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PolicyFieldEnforcement {
    /// 字段路径（`ProtectionConfig` 的 serde 名，如 `file.read_black`、`resource.memory_mb`）
    pub field: String,
    /// enforced | approximate | not_supported
    pub state: String,
    /// 实际生效的强制机制（照平台实现代码的事实）
    pub mechanism: String,
    /// 偏差/前提（approximate / not_supported 必有）
    pub caveat: Option<String>,
}

/// 策略字段级强制力报告：逐字段说明当前平台「到底怎么强制、近似到什么程度」。
///
/// 数据源 = 当前安全配置（`App::security_config`，App 未初始化回退默认），按与 bash 工具
/// 同源的字段映射编译为沙箱策略后取 `ResolvedPolicy::enforcement_report()`。
/// 报告的字段状态是**平台强制层**的事实，与单次执行的降级（passthrough 等）正交；
/// 工作区/会话白名单基线是逐次执行注入的，设置页视角不包含（故 `allow_workspace_exec`
/// 取与 `tool::policy::bash_protection` 相同的默认 true）。
#[napi]
pub fn policy_enforcement() -> napi::Result<Vec<PolicyFieldEnforcement>> {
    let sec = astorm_core::app::App::try_global()
        .map(|app| app.security_config())
        .unwrap_or_default();
    // 与 `astorm_core::tool::policy::bash_protection` 的安全配置 → 沙箱策略映射同口径
    // （该函数为 pub(crate)，此处按字段复刻；新增字段时两处都要改）
    let protection = ProtectionConfig {
        enabled: sec.enabled,
        file: FilePolicyCfg {
            read_black: sec.file.read_black.clone(),
            read_white: sec.file.read_white.clone(),
            write_black: sec.file.write_black.clone(),
            write_white: sec.file.write_white.clone(),
        },
        net: NetPolicyCfg {
            use_whitelist: sec.net.use_whitelist,
            in_black: sec.net.in_black.clone(),
            in_white: sec.net.in_white.clone(),
            out_black: sec.net.out_black.clone(),
            out_white: sec.net.out_white.clone(),
        },
        danger: DangerPolicyCfg {
            black: sec.danger.black.clone(),
            use_builtin: sec.danger.use_builtin,
            builtin_override: sec.danger.builtin_patterns.clone(),
        },
        resource: ResourceLimits {
            memory_mb: sec.resource.memory_mb,
            disk_mb: sec.resource.disk_mb,
            cpu_seconds: sec.resource.cpu_seconds,
            cpu_percent: sec.resource.cpu_percent,
            gpu: sec.resource.gpu,
        },
        toolchain_disabled: sec.toolchain_disabled.clone(),
        allow_workspace_exec: true,
        exec_white_dirs: sec.exec_white.clone(),
    };
    let input = SandboxInput {
        protection,
        ..SandboxInput::new("")
    };
    let resolved = ResolvedPolicy::compile(&input)
        .map_err(|e| napi::Error::from_reason(format!("策略编译失败: {e}")))?;
    Ok(resolved
        .enforcement_report()
        .into_iter()
        .map(|f| PolicyFieldEnforcement {
            field: f.field,
            state: f.state.as_str().to_string(),
            mechanism: f.mechanism,
            caveat: f.caveat,
        })
        .collect())
}
