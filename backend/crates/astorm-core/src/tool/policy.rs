//! 安全策略适配层：把会话模式（plan/execute/auto）× 权限模式（manual/auto）翻译成
//! bash 沙箱策略与"可审批 / 硬拒"分类；并为 file / bash 工具提供 `needs_approval` 结果构造。

use std::path::{Path, PathBuf};

use astorm_common::session::types::{PermissionMode, SessionMode};
use astorm_sandbox::policy::{DangerPolicyCfg, FilePolicyCfg, NetPolicyCfg, ProtectionConfig};
use serde_json::json;
use uuid::Uuid;

/// 会话权限模式（工具读取；缺省 Manual）。
pub(crate) fn permission_mode(session_id: &str) -> PermissionMode {
    crate::tool::session_record(session_id)
        .map(|r| r.permission_mode)
        .unwrap_or_default()
}

/// 会话是否"无需审批弹窗"：会话=auto 或 权限=auto。
///
/// **语义边界**：它只决定**可审批项**是否弹卡（自动同意），**绝不跳过安全硬规则**——
/// 危险命令 / 资源超限 / 技能目录只读 / Plan 只读等硬拒路径不受其影响。
/// 工具判定硬规则时必须先于本函数（见 [`plan_read_only`]），否则 Auto 会成为硬规则的逃生口。
pub(crate) fn auto_guard(session_id: &str) -> bool {
    let mode = crate::tool::session_mode(session_id);
    if mode == SessionMode::Auto {
        return true;
    }
    permission_mode(session_id) == PermissionMode::Auto
}

/// 会话是否只读（Plan 模式）。
///
/// **硬规则**：Plan 只读与权限模式**正交**——`permission_mode = auto` 只免弹审批卡，
/// 不解除写禁令（`docs/backend-contract.md` 「Plan（只读）模式的审批语义」）。
/// 空间外**读**不属于写禁令，Plan 下仍可（手动弹卡 / 自动免卡）。
pub(crate) fn plan_read_only(session_id: &str) -> bool {
    crate::tool::session_mode(session_id) == SessionMode::Plan
}

/// 审批请求构造参数（工具侧填充；由 [`needs_approval`] 生成中断信封并登记台账）。
pub(crate) struct ApprovalReq<'a> {
    /// 拦截分类（拒绝读 / 拒绝写 / 危险命令 / 网络 / 资源）+ 来源与明细。
    pub denial: crate::tool::denial::Denial,
    /// 被拦操作原文（命令 / `file_operation <op> <path>`）——重放闸门 key 与重放依据。
    pub command: &'a str,
    pub session_id: &'a str,
    /// 发起拦截的工具名（重放时按名查找工具）。
    pub tool_name: &'a str,
    /// 被拦工具调用 id（重放时定位历史 tool 消息）。
    pub tool_call_id: &'a str,
}

/// 生成一条审批中断（uuid `request_id`）→ 登记待决议 → 返回**纯 JSON 中断信封**。
///
/// 返回文本即工具结果：引擎据此命中 [`crate::tool::Tool::interrupt_payload`]，
/// 中断本轮循环并向前端发带 `payload` 的 `interrupt` 事件（前端弹审批卡）；
/// 用户决议经 `submitInterrupt` 交回，就地把结果写回这条 tool 消息。
pub(crate) fn needs_approval(req: ApprovalReq<'_>) -> String {
    let request_id = Uuid::new_v4().to_string();
    let denial = &req.denial;
    let message = format!(
        "操作需要用户审批（{}），已中断本轮循环等待用户决定：{}",
        denial.category.as_str(),
        denial.detail
    );
    let request = json!({
        "interrupt": true,
        "interrupt_type": "approval",
        "request_id": request_id,
        // 被拦工具调用 id：前端据此把审批卡与对应工具卡关联（决议时就地更新）
        "tool_call_id": req.tool_call_id,
        "message": message,
        "approval": {
            // 分类（7 值）+ 来源可信度 + 涉及目标；`kind` 为旧字段（历史待决中断仍带），
            // 前端按 `category ?? kind` 兜底
            "category": denial.category.as_str(),
            "source": denial.source,
            "targets": denial.targets,
            "command": req.command,
            "reason": denial.detail,
        },
    });
    register_pending(req.session_id, &req, &request);
    serde_json::to_string(&request).unwrap_or_default()
}

/// 待决议登记（工具登记与冷启动恢复的统一出口）。
///
/// 同时把会话状态置为「等待用户处理」（侧栏状态图标）：决议提交后续跑时会回到运行中。
pub(crate) fn register_pending_interrupt(
    session_id: &str,
    item: crate::tool::interrupt::PendingInterrupt,
) {
    use astorm_common::types::record::SessionStatus;
    let status = match item.interrupt_type {
        astorm_common::session::InterruptType::Approval => SessionStatus::AwaitingApproval,
        astorm_common::session::InterruptType::Form => SessionStatus::AwaitingForm,
        astorm_common::session::InterruptType::MailCompose => SessionStatus::AwaitingMailCompose,
    };
    if let Some(app) = crate::app::App::try_global() {
        app.interrupts.register_pending(session_id, item);
        app.session().set_status_of(session_id, status);
    }
}

/// 审批待决议登记（从本次审批参数与信封构造待决项）。
fn register_pending(session_id: &str, req: &ApprovalReq<'_>, request: &serde_json::Value) {
    let Some(request_id) = request.get("request_id").and_then(|v| v.as_str()) else {
        return;
    };
    register_pending_interrupt(
        session_id,
        crate::tool::interrupt::PendingInterrupt {
            request_id: request_id.to_string(),
            interrupt_type: astorm_common::session::InterruptType::Approval,
            tool_call_id: req.tool_call_id.to_string(),
            tool_name: req.tool_name.to_string(),
            request: request.clone(),
            command: Some(req.command.to_string()),
            requested_at: crate::agent::session::unix_now(),
        },
    );
}

/// 工具结果是否为**中断请求**（严格纯 JSON 且 `interrupt=true` 且带类型与 request_id）；
/// 是则返回该信封（引擎据此打断并透传前端）。
pub fn interrupt_request_from_result(result: &str) -> Option<serde_json::Value> {
    let v: serde_json::Value = serde_json::from_str(result.trim()).ok()?;
    let is_interrupt = v.get("interrupt").and_then(|x| x.as_bool()) == Some(true)
        && v.get("interrupt_type").and_then(|x| x.as_str()).is_some()
        && v.get("request_id").and_then(|x| x.as_str()).is_some();
    is_interrupt.then_some(v)
}

/// 命令是否命中"允许一次性重放"（bash 重跑前调用，命中即消费并返回 true）。
pub(crate) fn consume_allowed_rerun(session_id: &str, command: &str) -> bool {
    crate::app::App::try_global()
        .map(|app| app.interrupts.consume_allow(session_id, command))
        .unwrap_or(false)
}

/// 命令是否被拒绝过（供工具错误文案提示）。
pub(crate) fn is_denied(session_id: &str, command: &str) -> bool {
    crate::app::App::try_global()
        .map(|app| app.interrupts.is_denied(session_id, command))
        .unwrap_or(false)
}

/// 判断路径是否在会话工作区内。
///
/// 先做词法归一化 + 目录边界比较（避免 `/ws/proj-evil` 被 `/ws/proj` 前缀误判），
/// 再对最近的已存在祖先做 canonicalize（阻断 symlink/junction 逃逸）。
/// Windows / macOS 默认文件系统大小写不敏感，比较时统一小写。
pub(crate) fn path_in_workspace(workspace: &Path, path: &Path) -> bool {
    let ws = normalize(workspace);
    let p = normalize(path);
    #[cfg(any(windows, target_os = "macos"))]
    let (ws, p) = (ws.to_lowercase(), p.to_lowercase());
    let lexical = p == ws || p.starts_with(&format!("{ws}/"));
    if !lexical {
        return false;
    }
    // 工作区本身不存在（如尚未创建）时只能信任词法结果。
    let Ok(ws_real) = std::fs::canonicalize(workspace) else {
        return true;
    };
    canonicalize_nearest(path).starts_with(&ws_real)
}

/// 对路径最近的已存在祖先做 canonicalize，再把不存在的尾部拼回去。
/// 用于写入尚不存在的文件时仍能识别 symlink/junction 逃逸。
fn canonicalize_nearest(path: &Path) -> PathBuf {
    let mut cur = path.to_path_buf();
    let mut missing: Vec<std::ffi::OsString> = Vec::new();
    loop {
        if let Ok(real) = std::fs::canonicalize(&cur) {
            let mut out = real;
            for part in missing.iter().rev() {
                out.push(part);
            }
            return out;
        }
        match (cur.file_name(), cur.parent()) {
            (Some(name), Some(parent)) => {
                missing.push(name.to_os_string());
                cur = parent.to_path_buf();
            }
            _ => return path.to_path_buf(),
        }
    }
}

/// 路径归一化（`..`/`.` 解析 + 去尾分隔符；Windows 反斜杠统一斜杠）。
pub(crate) fn normalize(p: &Path) -> String {
    let mut out = std::path::PathBuf::new();
    for c in p.components() {
        match c {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    let s = out.to_string_lossy().replace('\\', "/");
    let trimmed = s.trim_end_matches('/');
    // 保留 POSIX 根 "/" 与 Windows 盘符根 "C:/"（否则前缀比较会失真）
    if trimmed.is_empty() || (trimmed.len() == 2 && trimmed.as_bytes()[1] == b':') {
        s
    } else {
        trimmed.to_string()
    }
}

/// 安全配置快照（前端设置页「安全配置」经 `update_config` 热更；未初始化 App 回退默认）。
pub(crate) fn security_config() -> crate::app::config::SecurityConfig {
    crate::app::App::try_global()
        .map(|app| app.security_config())
        .unwrap_or_default()
}

/// 系统技能目录（`AppDir/skills`，技能 / 技能包存放处）。
/// bash 可读/执行技能脚本但不可修改，故该目录作为**只读白名单**注入防护。
pub(crate) fn system_skills_dir() -> Option<std::path::PathBuf> {
    crate::app::App::try_global().map(|app| app.dir().skills_dir)
}

/// 本会话内存白名单快照（审批卡「当前会话允许」；App 未初始化 / 无记录 → None）。
pub(crate) fn session_allow_snapshot(session_id: &str) -> Option<crate::tool::session_allow::SessionAllow> {
    crate::app::App::try_global().and_then(|app| app.session_allow().snapshot(session_id))
}

/// 路径是否被**本会话**白名单放行（与 `path_allowed_by_security` 同一角色：
/// 命中的空间外路径视为已授权，静态预检不再弹审批卡）。
pub(crate) fn path_allowed_by_session(session_id: &str, path: &Path, write: bool) -> bool {
    crate::app::App::try_global()
        .map(|app| {
            app.session_allow()
                .path_allowed(session_id, &path.to_string_lossy(), write)
        })
        .unwrap_or(false)
}

/// 路径是否位于系统技能目录内（`{skills}/**`）。
pub(crate) fn path_in_skills_dir(path: &Path) -> bool {
    match system_skills_dir() {
        Some(dir) => path_in_workspace(&dir, path),
        None => false,
    }
}

/// 路径是否落在**平台强制层的默认可读范围**内（系统区 + 已登记工具链目录）。
///
/// 判据与内核读白名单同源（`astorm_sandbox::roots`，Seatbelt profile 与静态预检共用
/// 同一份根清单）：内核本就会放行的路径，静态预检不该再弹「空间外读」审批卡——
/// 否则 `which git; /usr/bin/git --version`、`ls /opt/homebrew/bin` 这类命令
/// 每天都要用户点一次「同意」，真正的空间外访问（如 ~/.ssh）反而淹没在噪音里。
///
/// 注意：这是**读**的豁免；写仍然只有会话工作区与显式白名单（内核写白名单未变）。
pub(crate) fn path_kernel_readable(path: &Path) -> bool {
    let disabled = security_config().toolchain_disabled;
    astorm_sandbox::roots::statically_readable(path, &disabled)
}

/// 沙箱**持久 HOME**（按工作区）：`{data_dir}/sandbox/home/<工作区名>-<路径哈希>`。
///
/// 为什么需要：沙箱原先给每条命令一个临时 HOME 并在命令后删除，于是 rustup/cargo
/// 工具链、uv 托管 Python、npm/pip 缓存、~/.gitconfig 全都不跨命令保留——同一会话里
/// 「刚装好的工具链下一条命令就没了」，把缓存锚进工作区成了唯一的活路
/// （会话 2026-10-08 大量轮次耗在这上面）。改为**按工作区稳定**的目录后：
/// - 缓存与用户级配置跨命令、跨会话复用（同项目）；
/// - 换工作区即换目录，项目之间不互相污染；
/// - 它位于应用数据目录内，**绝不指向真实 `$HOME`**（真实凭据仍不可达）。
///
/// `TMPDIR` / `TEMP` 不受影响：仍是逐命令临时目录（用完即删），临时文件不会堆积。
/// 目录由沙箱会话层创建；App 未初始化（单测/CLI）返回 None，退化为临时 HOME。
pub(crate) fn sandbox_home_dir(workspace: &Path) -> Option<std::path::PathBuf> {
    let app = crate::app::App::try_global()?;
    Some(
        app.dir()
            .data_dir
            .join("sandbox")
            .join("home")
            .join(sandbox_home_key(workspace)),
    )
}

/// 持久 HOME 的目录名：`<工作区名>-<路径哈希>`。
///
/// 只依赖工作区绝对路径：同一工作区永远映射到同一个目录（缓存不会被「搬家」），
/// 不同工作区（含同名不同路径）互不撞车。哈希用 FNV-1a——稳定、无依赖、跨版本不变
/// （`DefaultHasher` 的算法不保证跨版本稳定，一旦变化就会让缓存目录换位置）。
fn sandbox_home_key(workspace: &Path) -> String {
    let name: String = workspace
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "workspace".to_string());
    let name: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .take(40)
        .collect();
    let hash = fnv1a64(workspace.to_string_lossy().as_bytes());
    format!("{name}-{hash:016x}")
}

/// FNV-1a 64 位：稳定、无依赖、跨版本不变（`DefaultHasher` 的算法不保证跨版本稳定，
/// 一旦变化就会让缓存目录「搬家」，因此不用它）。
fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// 路径是否命中安全配置的**额外白名单**（glob 匹配；`write=true` 查写白名单，否则读）。
/// 命中的空间外路径视为已授权：file 工具直接放行、bash 静态预检不再审批。
pub(crate) fn path_allowed_by_security(path: &Path, write: bool) -> bool {
    let sec = security_config().file;
    let patterns = if write { &sec.write_white } else { &sec.read_white };
    path_matches_security_patterns(path, patterns)
}

/// 安全白名单 glob 匹配（抽成纯函数便于测试）。
///
/// Windows/macOS 默认文件系统大小写不敏感，白名单也必须按不敏感匹配；
/// Linux 保持大小写敏感（与文件系统语义一致）。
pub(crate) fn path_matches_security_patterns(path: &Path, patterns: &[String]) -> bool {
    if patterns.is_empty() {
        return false;
    }
    let p = path.to_string_lossy();
    let case_insensitive = cfg!(any(windows, target_os = "macos"));
    patterns.iter().any(|pat| {
        globset::GlobBuilder::new(pat)
            .case_insensitive(case_insensitive)
            .literal_separator(true)
            .build()
            .map(|g| g.compile_matcher().is_match(p.as_ref()))
            .unwrap_or(false)
    })
}

/// bash 防护策略（空间内读/写放行 + 网络默认拒绝 + 危险命令内置清单），
/// 叠加前端「安全配置」（额外黑白名单 / 网络名单替换 / 危险正则 / 资源限制）
/// 与**本会话内存白名单**（审批卡「当前会话允许」，仅本进程有效）。
///
/// 会话白名单并入的意义：命中它的目录/主机在沙箱层也被放行，命令**仍在沙箱内**执行
/// （其余目录与主机照旧受保护），无需无防护重跑。
///
/// `include_session_write`：是否并入会话白名单的**写**目录。Plan 是只读模式，
/// 传 `false`——否则「当前会话允许」会成为绕过只读契约（含静态分析查不到的写）的旁路。
pub(crate) fn bash_protection(
    workspace: &Path,
    session_id: &str,
    include_session_write: bool,
) -> ProtectionConfig {
    let ws = workspace.to_string_lossy().into_owned();
    let sec = security_config();
    // 会话空间基线始终隐式注入；用户额外白名单叠加（黑名单优先，由沙箱决策保证）
    let mut read_white = vec![format!("{ws}/**")];
    read_white.extend(sec.file.read_white.iter().cloned());
    // 系统技能目录只读放行（可读/执行技能脚本，禁止修改）：glob = {skills}/**，仅进读白名单
    if let Some(sk) = system_skills_dir() {
        read_white.push(format!("{}/**", sk.to_string_lossy()));
    }
    let mut write_white = vec![format!("{ws}/**")];
    write_white.extend(sec.file.write_white.iter().cloned());
    // 会话级内存白名单（目录 → glob；主机 → domain:/ip: 规则）
    let session = session_allow_snapshot(session_id);
    let mut net_out_white = sec.net.out_white.clone();
    let mut net_in_white = sec.net.in_white.clone();
    if let Some(s) = session {
        read_white.extend(s.read_dirs.iter().map(|d| format!("{d}/**")));
        if include_session_write {
            write_white.extend(s.write_dirs.iter().map(|d| format!("{d}/**")));
        }
        let rules: Vec<String> = s
            .net_hosts
            .iter()
            .filter_map(|h| crate::tool::session_allow::net_rule_literal(h))
            .collect();
        net_out_white.extend(rules.iter().cloned());
        net_in_white.extend(rules);
    }
    ProtectionConfig {
        enabled: sec.enabled,
        file: FilePolicyCfg {
            read_black: sec.file.read_black.clone(),
            // 空间内读/遍历放行（pwd / ls / cat / 读源码），否则工作区在 Seatbelt 下不可达
            read_white,
            write_black: sec.file.write_black.clone(),
            // 只放行会话空间内写 + 用户额外白名单；其余空间外写默认拒绝（命中默认拒绝 → 可审批）
            write_white,
        },
        net: NetPolicyCfg {
            use_whitelist: sec.net.use_whitelist,
            in_black: sec.net.in_black.clone(),
            in_white: net_in_white,
            out_black: sec.net.out_black.clone(),
            out_white: net_out_white,
        },
        danger: DangerPolicyCfg {
            use_builtin: sec.danger.use_builtin,
            builtin_override: sec.danger.builtin_patterns.clone(),
            black: sec.danger.black.clone(),
        },
        resource: astorm_sandbox::policy::ResourceLimits {
            memory_mb: sec.resource.memory_mb,
            disk_mb: sec.resource.disk_mb,
            cpu_seconds: sec.resource.cpu_seconds,
            cpu_percent: sec.resource.cpu_percent,
            gpu: sec.resource.gpu,
        },
        // 「本地环境」页禁用注入的工具 bin：沙箱会话层据此过滤 exec/读放行
        toolchain_disabled: sec.toolchain_disabled.clone(),
        // 工作区内可执行文件放行（.venv/bin、node_modules/.bin、编译产物、自写脚本）：
        // 默认开。解释器本就放行（`node -e` 已是任意代码执行），禁止工作区 exec
        // 不构成安全边界，只挡住正常开发流程（会话 2026-10-08 实测）。
        // 预留：将来给设置页一个开关时改为读 sec，无需再动沙箱层。
        allow_workspace_exec: true,
        // 「安全配置 → 额外可执行目录」：非常规安装位置的工具链/编译器（加法白名单）
        exec_white_dirs: sec.exec_white.clone(),
    }
}

/// 审计写入快捷函数（App 未初始化时静默跳过）。
/// `command` 记录执行前输入的原始命令，供审计页精确还原 bash 内容；可为空。
/// `category` 为安全拦截分类，`source` 为分类来源 / 可信度
/// （普通执行 / 审批同意放行等非拦截场景均为 None）。
#[allow(clippy::too_many_arguments)]
pub(crate) fn audit(
    session_id: &str,
    kind: crate::tool::audit::AuditKind,
    detail: &str,
    approval: crate::tool::audit::AuditApproval,
    command: Option<&str>,
    category: Option<crate::tool::denial::DenialCategory>,
    source: Option<crate::tool::denial::DenialSource>,
) {
    if let Some(app) = crate::app::App::try_global() {
        app.audit
            .record(session_id, kind, detail, approval, command, category, source);
    }
}

/// 由拒绝分类推审计类型（历史 `kind` 字符串兼容在调用侧处理）。
pub(crate) fn audit_kind_of(category: crate::tool::denial::DenialCategory) -> crate::tool::audit::AuditKind {
    use crate::tool::audit::AuditKind;
    use crate::tool::denial::DenialCategory as C;
    match category {
        C::FileRead => AuditKind::FileRead,
        C::FileWrite => AuditKind::FileWrite,
        C::Network => AuditKind::Network,
        C::Resource => AuditKind::Resource,
        // 危险命令 / 目录切换等策略硬拒：沿用命令类
        C::DangerCommand | C::CwdChange => AuditKind::Bash,
        // 浏览器自动化：独立审计类型（写操作审批 / Plan 硬拒）
        C::Browser => AuditKind::Browser,
        // 邮件操作：独立审计类型（发信确认 / 移动 / Plan 硬拒）
        C::Mail => AuditKind::Mail,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn path_in_workspace_detects_inside_outside() {
        let ws = Path::new("/ws/proj");
        assert!(path_in_workspace(ws, Path::new("/ws/proj/src/a.rs")));
        assert!(path_in_workspace(ws, Path::new("/ws/proj/../proj/b.rs")));
        assert!(!path_in_workspace(ws, Path::new("/ws/other/x.rs")));
        assert!(!path_in_workspace(ws, Path::new("/etc/passwd")));
    }

    #[cfg(any(windows, target_os = "macos"))]
    #[test]
    fn path_in_workspace_case_insensitive_on_windows_macos() {
        let (ws, p) = if cfg!(windows) {
            (Path::new("C:/WS/Proj"), Path::new("c:/ws/proj/src/a.rs"))
        } else {
            (Path::new("/WS/Proj"), Path::new("/ws/proj/src/a.rs"))
        };
        assert!(path_in_workspace(ws, p), "大小写不同仍应视为工作区内");
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn path_in_workspace_case_sensitive_on_linux() {
        assert!(
            !path_in_workspace(Path::new("/WS/Proj"), Path::new("/ws/proj/src/a.rs")),
            "Linux 文件系统区分大小写"
        );
    }

    #[cfg(windows)]
    #[test]
    fn security_whitelist_case_insensitive_on_windows() {
        let patterns = vec!["C:/Users/Me/Docs/**".to_string()];
        assert!(path_matches_security_patterns(
            Path::new("c:/users/me/docs/a.txt"),
            &patterns
        ));
        assert!(!path_matches_security_patterns(
            Path::new("c:/users/me/other/a.txt"),
            &patterns
        ));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn security_whitelist_case_insensitive_on_macos() {
        let patterns = vec!["/Users/Me/Docs/**".to_string()];
        assert!(path_matches_security_patterns(
            Path::new("/users/me/docs/a.txt"),
            &patterns
        ));
        assert!(!path_matches_security_patterns(
            Path::new("/users/me/other/a.txt"),
            &patterns
        ));
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn security_whitelist_case_sensitive_on_linux() {
        let patterns = vec!["/tmp/Docs/**".to_string()];
        assert!(!path_matches_security_patterns(
            Path::new("/tmp/docs/a.txt"),
            &patterns
        ));
        assert!(path_matches_security_patterns(
            Path::new("/tmp/Docs/a.txt"),
            &patterns
        ));
    }

    /// 「安全配置 → 额外可执行目录」必须原样透传到沙箱策略（exec 放行的用户入口）。
    #[test]
    fn 额外可执行目录透传到沙箱策略() {
        let p = bash_protection(Path::new("/tmp/ws"), "sess-x", true);
        assert_eq!(p.exec_white_dirs, security_config().exec_white);
        assert!(p.allow_workspace_exec, "工作区内 exec 默认放行");
    }

    /// 持久 HOME 的键必须**稳定**（缓存不能被「搬家」）且按工作区**互不相同**。
    #[test]
    fn 沙箱_home_键稳定且按工作区区分() {
        let a = sandbox_home_key(Path::new("/Users/x/proj-one"));
        let b = sandbox_home_key(Path::new("/Users/x/proj-one"));
        assert_eq!(a, b, "同一工作区必须映射到同一目录名");
        assert_ne!(
            a,
            sandbox_home_key(Path::new("/Users/x/proj-two")),
            "不同工作区不得撞名"
        );
        // 同名不同路径（两个 proj-one）也必须区分开
        assert_ne!(
            sandbox_home_key(Path::new("/Users/x/proj-one")),
            sandbox_home_key(Path::new("/Users/y/proj-one"))
        );
        assert!(a.starts_with("proj-one-"), "目录名应带工作区名便于辨认: {a}");
        assert!(
            a.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
            "目录名不得含需要转义的字符: {a}"
        );
    }

    #[test]
    fn needs_approval_registers_without_app_and_returns_json() {
        // App 未初始化时 needs_approval 静默（不 panic），仍返回合法中断信封。
        let text = needs_approval(ApprovalReq {
            denial: crate::tool::denial::Denial::new(
                crate::tool::denial::DenialCategory::FileWrite,
                crate::tool::denial::DenialSource::StaticPrecheck,
                "test",
            )
            .with_targets(vec!["/etc/x".into()]),
            command: "touch /etc/x",
            session_id: "sess-x",
            tool_name: "bash",
            tool_call_id: "call_1",
        });
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["interrupt"], true);
        assert_eq!(v["interrupt_type"], "approval");
        assert_eq!(v["tool_call_id"], "call_1");
        assert_eq!(v["approval"]["category"], "file_write");
        assert_eq!(v["approval"]["source"], "static_precheck");
        assert_eq!(v["approval"]["targets"], serde_json::json!(["/etc/x"]));
        assert_eq!(v["approval"]["command"], "touch /etc/x");
        assert_eq!(v["approval"]["reason"], "test");
        // 消息文案用分类稳定字符串
        assert!(v["message"].as_str().unwrap().contains("file_write"));
        assert!(v["request_id"].as_str().unwrap().len() >= 32);
        // 引擎/持久化侧的识别与工具结果一致
        assert_eq!(interrupt_request_from_result(&text), Some(v));
    }

    #[test]
    fn interrupt_request_from_result_requires_strict_json() {
        // 非纯 JSON（含引导文本）/ 缺类型或 request_id 均不识别
        assert!(interrupt_request_from_result("见下：\n{\"interrupt\":true}").is_none());
        assert!(interrupt_request_from_result("{\"interrupt\":true,\"interrupt_type\":\"approval\"}").is_none());
        assert!(interrupt_request_from_result("{\"interrupt\":false}").is_none());
        assert!(interrupt_request_from_result("普通结果文本").is_none());
    }

    #[test]
    fn bash_protection_enables_and_whitelists_workspace() {
        let p = bash_protection(Path::new("/ws"), "test-session", true);
        assert!(p.enabled);
        assert!(p.file.write_white.contains(&"/ws/**".to_string()));
        assert!(p.net.use_whitelist);
        assert!(p.danger.use_builtin);
    }
}

/// 内置危险命令正则清单（前端「安全配置」页展示并允许编辑为用户副本保存）。
pub fn builtin_danger_patterns() -> Vec<String> {
    astorm_sandbox::policy::BUILTIN_DANGER_PATTERNS
        .iter()
        .map(|s| s.to_string())
        .collect()
}
