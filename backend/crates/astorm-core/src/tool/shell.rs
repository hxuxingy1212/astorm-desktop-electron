//! `bash` 工具：会话工作区内执行 shell / powershell 命令（经 astorm-sandbox 防护执行）。
//!
//! 安全策略（会话模式 × 权限模式，见 `tool/policy.rs`）：
//! - **Plan（只读）是硬规则，与权限模式正交**：`permission = auto` 只免审批卡，不放宽写禁令。
//!   命令静态分析出**任意写目标**（含会话空间内）→ 硬拒（提示切换 Execute / Auto）。
//!   闸门先于 auto 判定——否则 Auto 会按空间外写命中做无防护二跑，直接写出工作区，
//!   成为只读契约的逃生口。只有空间外**读**可审批（读不属于写禁令）。
//! - **Auto 会话 / 自动审批**：两遍执行，但**只有可审批类信号**才关防护重跑
//!   （静态可判定的空间外写 / 结构化网络违规），并写审计(approval=auto)；
//!   硬拦截（危险命令 / 资源 / 结构化文件分类）与「无分类的内核 EPERM」都不重跑。
//! - **Execute + 手动审批**：
//!   - 危险命令 / 资源超限命中 → 拦截，写审计，直接返回错误信息（无审批）。
//!   - 网络（黑名单默认拒绝）→ 可审批：登记待决议、返回**中断信封**（`approval` 段）纯 JSON，
//!     引擎据此中断本轮循环并发带 `approval` 载荷的 interrupt 事件（前端弹审批卡）。
//!   - 命令静态分析出对**会话空间外路径的写** → 可审批（同上），不执行；用户同意后按目标扩白名单
//!     在沙箱内重跑（P2）；仍被内核挡住才兜底整条无防护重跑一次。
//!   - 沙箱给出**结构化文件分类**（预检文件黑名单 / 运行时文件事件）→ 按分类分流：读可审批、
//!     Plan 下写硬拒（提示切换模式）。
//!
//! 卡片来源（只有两个合法来源，**不再事后猜方向**）：
//! 1. 静态预检的确定分类（分析得出空间外读/写）；
//! 2. 沙箱的结构化分类（`Event::Denied/Evicted` 或 `report.denials` 的 `Category`）。
//!
//! 越出沙箱白名单的文件访问由**内核**（Seatbelt / AppContainer）拦下，只把 EPERM 写进进程输出，
//! **不产生任何结构化分类** —— 此时按普通命令失败交回 LLM（不弹卡、不硬拒、不重跑）。
//!
//! 已知残余：bash 的 Plan 写禁令是**静态**判定——解释器内联路径（如 `python -c` 拼出的路径）
//! 分析不出，而沙箱对会话空间内写放行，故这类写仍可能落在工作区内成功。收紧沙箱写白名单会使
//! 工具缓存 / 探测型命令大面积失败，属独立硬化议题。
//!
//! 超时（默认 120s，long_running 上限 7200s）、会话取消 → 终止进程树；输出截断 [`OUTPUT_MAX_CHARS`]。

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use astorm_common::session::types::SessionMode;
use astorm_sandbox::analyzer::analyze_command;
use astorm_sandbox::policy::{AccessKind, ExecMode, OutputEncoding, ProtectionConfig, SandboxInput, ShellSpec};
use astorm_sandbox::runner::ExecReport;
use astorm_sandbox::sandbox::{Category, Event};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;

use crate::tool::audit::AuditApproval;
use crate::tool::denial::{
    self as denial, Denial, DenialCategory, DenialSource,
};
use crate::tool::policy;
use crate::tool::policy::ApprovalReq;
use crate::tool::{expand_session_vars, expand_session_vars_shell, resolve_workspace, session_mode};

/// 前台命令默认超时（秒）。
const DEFAULT_TIMEOUT_SECS: u64 = 120;
/// `long_running` 默认超时（秒）。
const LONG_TIMEOUT_SECS: u64 = 1800;
/// 输出截断字符数（stdout / stderr 各自预算）。
const OUTPUT_MAX_CHARS: usize = 4096;
/// 截断时保留的**尾部**字符数：错误与 `[沙箱提示]` 都在末尾，只留头会把它们切掉。
const OUTPUT_TAIL_CHARS: usize = 1024;
/// 运行期事实（evicted / denied / notes / degraded）单条截断长度。
const FACT_MAX_CHARS: usize = 240;
/// 运行期事实每类最多保留条数。
const FACT_MAX_ITEMS: usize = 4;

/// `bash` 工具。
pub struct ShellTool;

/// 工具参数。
#[derive(Debug, Deserialize)]
struct ShellParams {
    command: String,
    /// 工作目录（相对工作区，缺省为会话工作区）
    #[serde(default)]
    cwd: Option<String>,
    /// 覆盖超时（秒）；`long_running` 时范围 60..=7200，否则 1..=1800
    #[serde(default)]
    timeout: Option<u64>,
    /// 长任务模式（默认 1800s 超时）
    #[serde(default)]
    long_running: bool,
}

#[async_trait]
impl crate::tool::Tool for ShellTool {
    fn name(&self) -> &str {
        "bash"
    }

    fn description(&self) -> &str {
        "在会话工作区执行 shell 命令（Windows 默认 cmd.exe，经沙箱执行）。命令输出会被截断（每条流 4096 字符，保留头尾）。支持 timeout 覆盖与 long_running 长任务。可用 $SESSION_WORKSPACE / $SESSION_ID 变量。命令不得切出会话工作区：`cd <工作区内路径>`（含相对子目录，如 `cd build && make`）允许；切出工作区、`cd ~`/`cd -`/`cd $VAR`/通配、以及 popd/chdir 会被拒——需要子目录时推荐用 cwd 参数。\
\n\n沙箱环境事实（照此规划命令，不要靠反复试探去发现边界）：\
① $HOME 是**按工作区持久**的沙箱目录（跨命令、跨会话保留，cargo/rustup/uv/npm/pip 缓存与用户级配置都在这里，装一次就够）；$TMPDIR 是**逐命令临时**目录（命令结束即删，别把要留存的东西只放那里）。它**不是**你的真实 HOME：真实用户配置（~/.ssh、~/.gitconfig 等）不可读。\
② 可读：系统区（/usr、/System、/bin、/sbin、/dev、/private/etc、/private/tmp）＋会话工作区＋已登记工具链目录（/opt/homebrew、/usr/local/bin、~/.cargo、~/.rustup、~/.local/bin 等）；其余如 /Applications、~/Library、~/.ssh 会被拒（Xcode/CLT 开发目录本身是放行的，见 ④）。\
③ 可写：只有会话工作区（外加 /dev/null 类零副作用设备与上面的沙箱 HOME）。\
④ 可执行：系统目录＋已登记工具链目录＋**工作区内的可执行文件**（.venv/bin、node_modules/.bin、编译产物、自写脚本都能直接跑）。非常规安装位置（手动解压的工具链、独立 SDK）需用户在设置 → 安全配置 → 「额外可执行目录」里放行，否则 exec 会被拒。原生编译开箱可用：沙箱已注入 DEVELOPER_DIR/SDKROOT（macOS 上固定指向 Command Line Tools，Xcode 的 xcodebuild 依赖 SIP 受限框架、沙箱内不可用），`cc`/`clang`/`xcrun`/`rustc` 可直接用。\
⑤ 网络：**回环（127.0.0.1 / localhost）始终放行**——本地服务起得来、连得上、healthz 自测可用；出站到外网可能按域名白名单拦截（设置页可放行或关闭白名单）。命令被杀（returncode 137/9）或被拦时，结果里的 sandbox 字段给出原因（protected/evicted/denied/notes/degraded），先看它再改命令。\
\n\n被安全策略拦截且属可审批项（访问会话空间外路径、需联网）时，本轮循环会中断并由用户在前端审批：用户同意后系统自动重放同一命令（**该次重放无沙箱防护**，结果里 sandbox.protected=false 会标出），无需再次调用本工具。Plan（只读）模式下写文件不会提供审批：会直接拒绝并要求切换到 Execute 或 Auto 模式，请勿重复尝试同样的写命令。输出中出现「[沙箱提示]」表示该失败带内核拒绝特征（可能确系沙箱拦截，也可能是被拦的子进程把内核拒绝原样带出），提示正文按本次实际策略写清网络有没有被限。不要对同一路径反复重试：改用工作区内路径、放行目录下的工具，或让用户审批。"
    }

    /// 审批拦截结果（纯 JSON）→ 中断载荷：引擎据此发带 `approval` 的 interrupt 事件。
    fn interrupt_payload(&self, result: &str) -> Option<serde_json::Value> {
        policy::interrupt_request_from_result(result)
    }

    /// 硬拒绝（危险命令 / 资源限制 / Plan 拒写 / 技能目录只读 / 禁 cd）：
    /// 错误原文即拒绝信封 → 转分类元数据（tool_result 的 `denial` + 消息 `metadata.denial`）。
    /// 同时把结果 JSON 里的 `sandbox` 事实（无防护 / 驱逐 / 超时 / 输出截断）并进元数据，
    /// 供前端工具卡标注「无防护执行 / 被驱逐 / 输出截断」；结果不是预期 JSON
    /// （普通失败文本）时静默跳过事实，既有 `denial` 形状不受影响。
    fn result_meta(&self, ok: bool, result: &str) -> Option<serde_json::Value> {
        let sandbox = sandbox_facts_from_result(result);
        let mut meta = if ok { None } else { denial::denial_meta_from_error(result) };
        if let Some(facts) = sandbox {
            let obj = meta.get_or_insert_with(|| json!({}));
            if let Some(map) = obj.as_object_mut() {
                map.insert("sandbox".to_string(), facts);
            }
        }
        meta
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "description": { "type": "string", "description": "一句话说明本次调用的目的（展示给用户，简短）" },
                "command": { "type": "string", "description": "要执行的命令" },
                "cwd": { "type": "string", "description": "工作目录（相对工作区，缺省为工作区）。禁止在命令里用 cd 切换目录，需子目录时填此参数" },
                "timeout": { "type": "integer", "description": "覆盖超时（秒）" },
                "long_running": { "type": "boolean", "description": "长任务模式，超时上限提高到 7200s" }
            },
            "required": ["command", "description"]
        })
    }

    async fn execute(
        &self,
        params: serde_json::Value,
        ctx: &react_loop::ToolContext,
    ) -> Result<String, String> {
        let p: ShellParams =
            serde_json::from_value(params).map_err(|e| format!("bash 参数解析失败: {e}"))?;
        if p.command.trim().is_empty() {
            return Err("bash 缺少 command".into());
        }

        let workspace = resolve_workspace(&ctx.session_id);
        let command = expand_session_vars_shell(&p.command, &ctx.session_id, &workspace);
        let cwd = match &p.cwd {
            Some(c) => {
                let c = expand_session_vars(c, &ctx.session_id, &workspace);
                let c = Path::new(&c);
                if c.is_absolute() {
                    c.to_path_buf()
                } else {
                    workspace.join(c)
                }
            }
            None => workspace.clone(),
        };
        std::fs::create_dir_all(&cwd).map_err(|e| format!("创建目录失败 {}: {e}", cwd.display()))?;

        // 硬规则（任何模式、无审批）：**不得切出会话工作区**。
        // P2 放宽：`cd <工作区内路径>`（含相对子目录）允许——构建脚本的日常写法
        // （`cd build && make`）；切出工作区、或目标无法可靠解析（变量/~/通配/`-`/无目标）、
        // 以及 popd/chdir 仍按硬规则拒绝，并提示改用 cwd 参数。
        if let Some(what) = cwd_change_blocked(&command, &cwd, &workspace) {
            return Err(hard_deny(
                ctx,
                &command,
                &Denial::new(
                    DenialCategory::CwdChange,
                    DenialSource::StaticPrecheck,
                    format!(
                        "命令包含「{what}」切换目录操作，但目标不在会话工作区内（或无法可靠解析，如变量 / ~ / 通配），已被禁止：命令不得切出会话工作区。工作区**内**的 cd 写法（如 `cd sub && make`）是允许的；需要子目录时仍推荐用本工具的 cwd 参数（如 cwd: \"backend\"）。"
                    ),
                ),
            ));
        }

        // 硬规则（任何模式、无审批）：系统技能目录只读，禁止写入。
        // 必须先于此处的审批/自动二跑判定——否则 Auto/批准后的重跑（即便已改为扩白名单）会真的写进 skill 目录。
        if let Some(sk_target) = static_skill_write(&command) {
            let denial = Denial::new(
                DenialCategory::FileWrite,
                DenialSource::StaticPrecheck,
                format!("系统技能目录为只读资源，禁止修改：{sk_target}"),
            )
            .with_targets(vec![sk_target]);
            return Err(hard_deny(ctx, &command, &denial));
        }

        let timeout_secs = if p.long_running {
            p.timeout.unwrap_or(LONG_TIMEOUT_SECS).clamp(60, 7200)
        } else {
            p.timeout.unwrap_or(DEFAULT_TIMEOUT_SECS).clamp(1, 1800)
        };

        let mode = session_mode(&ctx.session_id);
        let auto = policy::auto_guard(&ctx.session_id);

        // 已被拒绝过（拒绝闸门 60s 窗口内）：直接说明，避免同命令反复撞审批。
        // （不是新的安全拒绝，而是「同一被拒命令不重复执行」的引导，故不带分类元数据）
        if policy::is_denied(&ctx.session_id, &command) {
            return Err(format!(
                "命令已被用户拒绝执行：{command}\n请改换思路，不要重复执行同一被拒命令。"
            ));
        }

        // Plan（只读）**静态写闸门**：先于台账与 auto——只读是硬规则，`permission = auto`
        // 只免弹审批卡，不解除写禁令。
        // 先于台账：Plan 的写从不产生审批，故不应消费任何一次性额度（否则 Execute 期遗留的
        // 额度在切到 Plan 后会被消费，放行一次写）。
        // 先于 auto 分支：否则 Auto 会按下方 `static_hit` 命中做**无防护二跑**，直接写出
        // 会话空间（Auto 成为硬规则逃生口）。
        let plan_writes = (mode == SessionMode::Plan).then(|| plan_any_write(&command)).flatten();
        if let Some(writes) = plan_writes {
            return Err(hard_deny(ctx, &command, &plan_static_write_denial(&command, &writes)));
        }

        // 命中"允许一次性重跑"：按目标扩白名单在沙箱内重跑（仍被挡住才兜底无防护，见下方批准分支）
        let approved_rerun = policy::consume_allowed_rerun(&ctx.session_id, &command);

        // 本次执行的内核网络策略：白名单开启且名单为空 → profile 里 `(deny network-outbound)`。
        // 与 seatbelt_builder 用同一份已解析策略，判据不会漂移。用途：内核整片拒绝出站时
        // 改用静态联网意图补位（见 tool::net_intent）；[沙箱提示] 的文案也据此条件生成，
        // **不能无条件断言网络被禁**（用户关掉白名单后内核已放行，再这么说会把模型带偏）。
        let kernel_denies_outbound = {
            let prot = policy::bash_protection(&workspace, &ctx.session_id, mode != SessionMode::Plan);
            prot.enabled && prot.net.use_whitelist && prot.net.out_white.is_empty()
        };
        let net_intent = kernel_denies_outbound
            .then(|| crate::tool::net_intent::network_intent(&command))
            .flatten();

        // 危险命令 / 资源：硬拦截路径在受保护执行内处理；
        // 网络与空间外读写属可审批项 → 返回审批请求（引擎中断，用户在前端审批后自动重放）。
        // 手动审批模式下，执行前先做命令静态分析：空间外读 → 空间外写 → 联网意图 →
        // 命中即不执行（**Plan 的写在更上方已硬拒**，这里只剩非 Plan 的可审批项）。
        if !approved_rerun && !auto {
            if let Some(ask) = precheck_read_askable(ctx, &workspace, &command) {
                return Ok(ask);
            }
            // Plan 写已在上方闸门硬拒，此处只剩非 Plan 的空间外写 → 返回审批信封
            if let Some(ask) = precheck_write_askable(ctx, &workspace, &command) {
                return Ok(ask);
            }
        }

        // 组装沙箱输入
        // 持久 HOME（按工作区）：缓存/用户级配置跨命令复用；未初始化 App 时为 None（退化旧行为）。
        // 三种跑法都用它，避免「同一条命令两遍看到不同 HOME」。
        let session_home = policy::sandbox_home_dir(&workspace);
        let make_input = |protection: ProtectionConfig, cancel: Arc<AtomicBool>| SandboxInput {
            command: command.clone(),
            shell: ShellSpec {
                program: None,
                direct: false,
                raw_command_arg: None,
            },
            timeout: Duration::from_secs(timeout_secs),
            mode: ExecMode::Sync,
            protection,
            strict: false,
            output_encoding: OutputEncoding::Auto,
            cwd: Some(cwd.clone()),
            workspace: Some(workspace.clone()),
            session_home: session_home.clone(),
            cancel: Some(cancel),
        };

        // 会话取消信号
        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_signal = cancel.clone();
        let ctx_cancel = ctx.cancel.clone();
        let cancel_task = tokio::spawn(async move {
            ctx_cancel.cancelled().await;
            cancel_signal.store(true, Ordering::SeqCst);
        });

        // 基础策略（受防护）。Plan 只读：不并入会话白名单的写目录（只读契约不受会话授权放宽）。
        // 每次执行现算：扩白名单重跑要基于它叠加，而不是改全局状态。
        let base_protection =
            || policy::bash_protection(&workspace, &ctx.session_id, mode != SessionMode::Plan);

        // 在阻塞线程跑一次沙箱，返回 report。
        let spawn_run = |input: SandboxInput| async move {
            tokio::task::spawn_blocking(move || astorm_sandbox::runner::run_sync(input))
                .await
                .map_err(|e| format!("沙箱执行任务失败: {e}"))
        };
        // 受防护执行；`ext` 非空时把本次被拦目标并入白名单（**仍在沙箱内**）。
        let run_protected = |ext: Option<RerunExt>, cancel: Arc<AtomicBool>| {
            let mut prot = base_protection();
            if let Some(e) = ext.as_ref() {
                if !e.is_empty() {
                    prot = extend_protection(prot, e);
                }
            }
            spawn_run(make_input(prot, cancel))
        };
        // 无防护执行：**只留给"用户明确批准却仍被挡住"的兜底**，Auto 不走这条。
        let run_unprotected = |cancel: Arc<AtomicBool>| {
            spawn_run(make_input(
                ProtectionConfig {
                    enabled: false,
                    ..ProtectionConfig::default()
                },
                cancel,
            ))
        };

        // 联网意图（仅在内核整片拒绝出站时才有意义）→ 手动审批卡。
        // 放在这里而不是上面的预检组：会话白名单已覆盖时要**扩白名单重跑**，需要 run_protected/cancel。
        // Auto 不走这里（它按下面的静态意图直接放行重跑）；Plan 不拦网络（只读契约只禁写）。
        if !approved_rerun && !auto {
            if let Some(hit) = net_intent.as_ref() {
                let targets = crate::tool::net_intent::network_targets(&command);
                if session_allows_network(&ctx.session_id, &command, &targets) {
                    // 本会话已授权过该目标：**按目标扩白名单、仍在沙箱内重跑**（P2）
                    policy::audit(
                        &ctx.session_id,
                        crate::tool::audit::AuditKind::Bash,
                        &format!("会话白名单放行（联网意图：{hit}）后按目标扩白名单在沙箱内重跑：{command}"),
                        AuditApproval::Agree,
                        Some(command.as_str()),
                        Some(DenialCategory::Network),
                        Some(DenialSource::StaticPrecheck),
                    );
                    let ext = if targets.is_empty() {
                        RerunExt::net_unrestricted(Vec::new(), Vec::new())
                    } else {
                        RerunExt::from_targets(Vec::new(), Vec::new(), targets)
                    };
                    let report = run_protected(Some(ext), cancel.clone()).await?;
                    cancel_task.abort();
                    return Ok(format_json_result(&command, report, &ctx.session_id, &cwd));
                }
                let ask = policy::needs_approval(ApprovalReq {
                    denial: Denial::new(
                        DenialCategory::Network,
                        DenialSource::StaticPrecheck,
                        format!(
                            "命令需要联网（{hit}），而当前策略默认拒绝所有出站连接；\
                             同意后按目标扩白名单在沙箱内重跑（提不出具体主机时本次不限制出站，\
                             文件/资源/危险命令仍受控）；若仍被内核挡住，才会整条无防护重跑一次"
                        ),
                    )
                    .with_targets(targets)
                    .with_command(&command),
                    command: &command,
                    session_id: &ctx.session_id,
                    tool_name: "bash",
                    tool_call_id: &ctx.tool_call_id,
                });
                cancel_task.abort();
                return Ok(ask);
            }
        }

        let report = if approved_rerun {
            // 用户同意 → 扩白名单重跑（审计同意；非拦截，无分类）
            policy::audit(
                &ctx.session_id,
                crate::tool::audit::AuditKind::Bash,
                &format!("批准执行（按目标扩白名单，仍在沙箱内）：{command}"),
                AuditApproval::Agree,
                Some(command.as_str()),
                None,
                None,
            );
            // 用户明确批准 → 先按目标扩白名单在沙箱内跑；仍被挡住才兜底整条无防护
            // （人工已同意，且结果里 sandbox.protected=false 会标出来）。
            let (reads, writes) = outside_targets(&ctx.session_id, &workspace, &command);
            let net = crate::tool::net_intent::network_targets(&command);
            let ext = RerunExt::from_targets(reads, writes, net);
            let first = run_protected(Some(ext), cancel.clone()).await?;
            if kernel_blocked_again(&first) {
                policy::audit(
                    &ctx.session_id,
                    crate::tool::audit::AuditKind::Bash,
                    &format!("扩白名单后仍被内核拒绝 → 兜底无防护重跑：{command}"),
                    AuditApproval::Agree,
                    Some(command.as_str()),
                    None,
                    None,
                );
                run_unprotected(cancel.clone()).await?
            } else {
                first
            }
        } else if auto {
            // 自动两遍：先防护跑，只有**可审批类**信号才关防护重跑（Auto 的自动审批语义）：
            //   ① 静态可判定的空间外写（文件类内核拒绝不产生事件，靠静态分析补位）
            //   ② 结构化网络违规
            // 硬拦截（危险命令 / 资源 / 结构化文件分类）**永不重跑** —— 否则 Auto 会成为硬规则
            // 的逃生口；「无分类的内核 EPERM」没有弹卡/放行的依据，也不重跑（按普通失败返回）。
            // 普通非零退出但无拦截（如 grep 无匹配）不重跑，避免副作用两遍。
            // Plan 下无静态写目标（有则已在上方闸门硬拒）；guard 一次语义不放宽只读契约
            let static_hit = if mode == SessionMode::Plan {
                None
            } else {
                static_outside_write(Some(&ctx.session_id), &workspace, &command)
            };
            let mut first = run_protected(None, cancel.clone()).await?;
            // 内核拒绝不可观测（Seatbelt 只把 EPERM 写进输出）→ 受保护失败时补一句诊断，
            // 免得模型把「沙箱拦了」当成「命令写错了」反复试错（不改路由：不弹卡、不重跑）
            annotate_kernel_denial(&mut first, kernel_denies_outbound);
            // 硬拦截与手动审批同口径：危险命令 / 资源 → 直接报错（不二跑、不静默）
            if let Some(denial) = hard_block_denial(&first) {
                return Err(hard_deny(ctx, &command, &denial));
            }
            // 沙箱结构化文件分类（文件黑名单等）：属硬规则，不二跑；按分类报错（不静默）
            if let Some(denial) = classified_file_denial(&first) {
                return Err(hard_deny(ctx, &command, &denial));
            }
            if static_hit.is_none() && net_intent.is_none() && !has_network_violation(&first) {
                first
            } else {
                // 来源按二跑依据：静态空间外写 / 静态联网意图 → 静态预检；网络驱逐事件 → 沙箱运行期
                let is_static_write = static_hit.is_some();
                let (detail, kind, category, source) = if let Some(outside) = &static_hit {
                    (
                        format!("自动放行（静态检测空间外写：{outside}）后按目标扩白名单在沙箱内重跑：{command}"),
                        crate::tool::audit::AuditKind::FileWrite,
                        Some(DenialCategory::FileWrite),
                        Some(DenialSource::StaticPrecheck),
                    )
                } else if let Some(hit) = &net_intent {
                    (
                        format!("自动放行（静态检测联网意图：{hit}）后按目标扩白名单在沙箱内重跑：{command}"),
                        crate::tool::audit::AuditKind::Bash,
                        Some(DenialCategory::Network),
                        Some(DenialSource::StaticPrecheck),
                    )
                } else {
                    let net_detail = first_network_detail(&first)
                        .unwrap_or_else(|| "命令尝试访问网络，被默认拒绝策略拦截".to_string());
                    (
                        format!("自动放行（网络：{net_detail}）后按目标扩白名单在沙箱内重跑：{command}"),
                        crate::tool::audit::AuditKind::Bash,
                        Some(DenialCategory::Network),
                        Some(DenialSource::SandboxRuntime),
                    )
                };
                policy::audit(
                    &ctx.session_id,
                    kind,
                    &detail,
                    AuditApproval::Auto,
                    Some(command.as_str()),
                    category,
                    source,
                );
                // 扩白名单依据按触发来源取：静态空间外写 → 读写目标；联网 → 命令里的网络目标
                // （提不出具体主机 → 只关联网白名单开关，文件/资源/危险命令仍受控）。
                let (reads, writes) = outside_targets(&ctx.session_id, &workspace, &command);
                let net = if is_static_write {
                    Vec::new()
                } else {
                    crate::tool::net_intent::network_targets(&command)
                };
                let ext = if is_static_write {
                    RerunExt::from_targets(reads, writes, net)
                } else if net.is_empty() {
                    RerunExt::net_unrestricted(reads, writes)
                } else {
                    RerunExt::from_targets(reads, writes, net)
                };
                run_protected(Some(ext), cancel.clone()).await?
            }
        } else {
            // 手动审批：受保护跑；根据违规类型分流
            let mut first = run_protected(None, cancel.clone()).await?;
            annotate_kernel_denial(&mut first, kernel_denies_outbound);
            if first_guard_ok(&first) {
                first
            } else {
                // 危险命令 / 资源 → 审计 + 直接报错（不审批）
                if let Some(denial) = hard_block_denial(&first) {
                    return Err(hard_deny(ctx, &command, &denial));
                }
                // 网络（可审批）→ 会话白名单已覆盖则按目标扩白名单在沙箱内重跑（审计留痕）；
                // 否则返回审批请求（纯 JSON，引擎中断等用户决定）
                if has_network_violation(&first) {
                    let net_detail = first_network_detail(&first)
                        .unwrap_or_else(|| "命令尝试访问网络，被默认拒绝策略拦截".to_string());
                    let targets = first_network_targets(&first);
                    if !session_allows_network(&ctx.session_id, &command, &targets) {
                        let ask = policy::needs_approval(ApprovalReq {
                            denial: Denial::new(DenialCategory::Network, DenialSource::SandboxRuntime, net_detail)
                                .with_targets(targets),
                            command: &command,
                            session_id: &ctx.session_id,
                            tool_name: "bash",
                            tool_call_id: &ctx.tool_call_id,
                        });
                        cancel_task.abort();
                        return Ok(ask);
                    }
                    policy::audit(
                        &ctx.session_id,
                        crate::tool::audit::AuditKind::Bash,
                        &format!("会话白名单放行（网络：{net_detail}）后按目标扩白名单在沙箱内重跑：{command}"),
                        AuditApproval::Agree,
                        Some(command.as_str()),
                        Some(DenialCategory::Network),
                        Some(DenialSource::SandboxRuntime),
                    );
                    let targets = crate::tool::net_intent::network_targets(&command);
                    let ext = if targets.is_empty() {
                        RerunExt::net_unrestricted(Vec::new(), Vec::new())
                    } else {
                        RerunExt::from_targets(Vec::new(), Vec::new(), targets)
                    };
                    let report = run_protected(Some(ext), cancel.clone()).await?;
                    cancel_task.abort();
                    return Ok(format_json_result(&command, report, &ctx.session_id, &cwd));
                }
                // 沙箱给出的**结构化文件分类**（预检文件黑名单 / 运行时文件事件）→ 按分类分流。
                // 内核文件拒绝（越出白名单）不产生分类，故不在此列：留到最后按普通失败返回。
                if let Some(denial) = classified_file_denial(&first) {
                    // Plan 是**只读模式**：写一律硬拒，不接受临时审批。
                    // 理由：Plan 下写只有「切换模式」才能授权，且审批同意后的重跑本就绕开静态写闸门——
                    // 用一次性审批绕过只读契约自相矛盾。读仍走审批（Plan 允许读空间外）。
                    if mode == SessionMode::Plan && denial.category == DenialCategory::FileWrite {
                        return Err(hard_deny(ctx, &command, &plan_write_denial(&denial, &command)));
                    }
                    policy::audit(
                        &ctx.session_id,
                        policy::audit_kind_of(denial.category),
                        &denial.detail,
                        AuditApproval::None,
                        Some(command.as_str()),
                        Some(denial.category),
                        Some(denial.source),
                    );
                    let ask = policy::needs_approval(ApprovalReq {
                        denial,
                        command: &command,
                        session_id: &ctx.session_id,
                        tool_name: "bash",
                        tool_call_id: &ctx.tool_call_id,
                    });
                    cancel_task.abort();
                    return Ok(ask);
                }
                first
            }
        };

        cancel_task.abort();
        Ok(format_json_result(&command, report, &ctx.session_id, &cwd))
    }
}

/// 预检：命令是否包含对会话空间外路径的读（静态分析可精确指认 → 可审批）。
/// Plan / Execute 模式空间外读均需审批（Auto 由两遍沙箱执行兜底，不预检）。
/// 相对路径不拦（cwd 在工作区内解析，与写预检同粒度）；无文件引用的命令自然放行；
/// 安全配置读白名单（pathAllowedBySecurity）命中的空间外路径视为已授权，放行。
/// **内核默认可读范围**（系统区 + 已登记工具链目录，见 [`policy::path_kernel_readable`]）
/// 同样放行：那些路径 Seatbelt / AppContainer 本就会放行，弹卡只会制造噪音。
fn precheck_read_askable(
    ctx: &react_loop::ToolContext,
    workspace: &Path,
    command: &str,
) -> Option<String> {
    let analysis = analyze_command(command);
    let mut outside: Vec<String> = Vec::new();
    for f in &analysis.file_refs {
        if f.kind != AccessKind::Read {
            continue;
        }
        let p = Path::new(&f.path);
        if p.is_absolute()
            && !policy::path_in_workspace(workspace, p)
            && !policy::path_allowed_by_security(p, false)
            && !policy::path_allowed_by_session(&ctx.session_id, p, false)
            && !policy::path_in_skills_dir(p)
            && !policy::path_kernel_readable(p)
        {
            outside.push(f.path.clone());
        }
    }
    if outside.is_empty() {
        return None;
    }
    let denial = Denial::new(
        DenialCategory::FileRead,
        DenialSource::StaticPrecheck,
        format!("命令试图读取会话空间外路径：{}", outside.join("，")),
    )
    .with_targets(outside)
    .with_command(command);
    policy::audit(
        &ctx.session_id,
        policy::audit_kind_of(denial.category),
        &denial.detail,
        AuditApproval::None,
        Some(command),
        Some(denial.category),
        Some(denial.source),
    );
    Some(policy::needs_approval(ApprovalReq {
        denial,
        command,
        session_id: &ctx.session_id,
        tool_name: "bash",
        tool_call_id: &ctx.tool_call_id,
    }))
}

/// 静态分析：命令中指向会话空间外的写目标（绝对路径，含重定向 / 写类命令）。
/// 安全配置写白名单（pathAllowedBySecurity）命中的空间外路径视为已授权，不计入。
/// `session_id` 给出时**会话内存白名单**（审批卡「当前会话允许」）同样视为已授权；
/// Plan 模式传 `None`——只读契约是硬规则，会话白名单不得放宽。
fn static_outside_write(
    session_id: Option<&str>,
    workspace: &Path,
    command: &str,
) -> Option<String> {
    let analysis = analyze_command(command);
    let mut outside: Vec<String> = Vec::new();
    for f in &analysis.file_refs {
        if f.kind != AccessKind::Write {
            continue;
        }
        let p = Path::new(&f.path);
        let session_ok = session_id
            .map(|sid| policy::path_allowed_by_session(sid, p, true))
            .unwrap_or(false);
        if p.is_absolute()
            && !policy::path_in_workspace(workspace, p)
            && !policy::path_allowed_by_security(p, true)
            && !session_ok
        {
            outside.push(f.path.clone());
        }
    }
    if outside.is_empty() {
        None
    } else {
        Some(outside.join("，"))
    }
}

/// 静态分析：命令中的**所有**写目标（绝对 + 相对；相对路径按 `cwd`=工作区解析）。
///
/// Plan 是只读模式：任意写目标（**含会话空间内**）都违背只读契约 → 硬拒。
/// 零副作用设备（`/dev/null` / `NUL` 等）已由分析器豁免写引用，`2>/dev/null` 惯用法不受影响。
fn plan_any_write(command: &str) -> Option<String> {
    let analysis = analyze_command(command);
    let writes: Vec<String> = analysis
        .file_refs
        .iter()
        .filter(|f| f.kind == AccessKind::Write)
        .map(|f| f.path.clone())
        .collect();
    if writes.is_empty() {
        None
    } else {
        Some(writes.join("，"))
    }
}

/// 静态分析：命令中指向系统技能目录的写目标（绝对路径）。
/// skill 目录只读，命中即硬拒（所有模式），不允许审批/自动二跑放行。
fn static_skill_write(command: &str) -> Option<String> {
    let analysis = analyze_command(command);
    let mut hits: Vec<String> = Vec::new();
    for f in &analysis.file_refs {
        if f.kind != AccessKind::Write {
            continue;
        }
        let p = Path::new(&f.path);
        if p.is_absolute() && policy::path_in_skills_dir(p) {
            hits.push(f.path.clone());
        }
    }
    if hits.is_empty() {
        None
    } else {
        Some(hits.join("，"))
    }
}

/// 预检：命令是否包含对会话空间外路径的写（静态分析可精确指认 → 可审批）。
///
/// 仅**非 Plan** 可达：Plan 的写（含会话空间内）已由 [`plan_any_write`] 闸门硬拒，
/// 故此处不再分 Plan/Execute，直接按可审批项弹卡。
fn precheck_write_askable(
    ctx: &react_loop::ToolContext,
    workspace: &Path,
    command: &str,
) -> Option<String> {
    // 会话内存白名单（审批卡「当前会话允许」）命中的空间外写视为已授权
    let outside = static_outside_write(Some(ctx.session_id.as_str()), workspace, command)?;
    let denial = Denial::new(
        DenialCategory::FileWrite,
        DenialSource::StaticPrecheck,
        format!("命令试图写入会话空间外路径：{outside}"),
    )
    .with_targets(outside.split('，').map(|s| s.to_string()).collect())
    .with_command(command);
    policy::audit(
        &ctx.session_id,
        policy::audit_kind_of(denial.category),
        &denial.detail,
        AuditApproval::None,
        Some(command),
        Some(denial.category),
        Some(denial.source),
    );
    Some(policy::needs_approval(ApprovalReq {
        denial,
        command,
        session_id: &ctx.session_id,
        tool_name: "bash",
        tool_call_id: &ctx.tool_call_id,
    }))
}

/// 首次受保护执行是否"无违规"（正常完成）。
fn first_guard_ok(r: &ExecReport) -> bool {
    r.ok && r.events.iter().all(|e| !matches!(
        e,
        astorm_sandbox::sandbox::Event::Denied { .. }
            | astorm_sandbox::sandbox::Event::Evicted { .. }
    ))
}

/// 内核拒绝在输出里的特征文本（macOS Seatbelt / Windows AppContainer / 通用 EACCES）。
const OS_DENY_PATTERNS: &[&str] = &[
    "operation not permitted",
    "permission denied",
    "access is denied",
    "拒绝访问",
];
/// 诊断标记：幂等（不重复追加）+ 便于测试断言。
const KERNEL_DENIAL_MARK: &str = "[沙箱提示]";

/// 受保护执行失败、且输出带内核拒绝特征时，补一句**诊断**（**不改路由**：不弹卡、不重跑）。
///
/// 为什么需要：Seatbelt / AppContainer 的运行期拒绝不产生任何结构化事件（实测结论：
/// profile 里的 `(with message …)` 也不会出现在子进程 stderr），Auto 没有信号可重跑、
/// Manual 没有依据弹卡，按既定设计「按普通失败返回」。但失败原文对模型只是一条裸
/// `Operation not permitted`，它会误判成「命令写错了 / 环境不支持」，再花几轮试别的路径——
/// 会话 2026_09_14_00_22_47 因此烧掉大量轮次（/dev/null 重定向、~/Library 写入、
/// /Applications 下的可执行文件、联网下载全属这一类）。
/// 这里只把原因与可行动作说清楚，不动分类与放行判定。
///
/// 两个必须守住的边界（都可从报告本身判定，不靠调用方自觉）：
/// 1. **沙箱没在内核层生效就别贴**（总开关关闭 / 平台降级为驱逐监视）——那时的 EPERM 是
///    操作系统真实报的错，贴「沙箱提示」会把排查方向彻底带偏；
/// 2. **网络一句必须按实际策略写**——用户关掉联网白名单后内核是 `(allow network-outbound)`，
///    此时再断言「网络默认拒绝所有出站」会让模型直接放弃联网方案（实测被原文照抄）。
///
/// 判据只有「输出里出现内核拒绝特征文本」+ 幂等标记，**不看退出码**：
/// `cmd 2>/dev/null; echo done` 这类链式命令整体退出码是 0，但中间那步确实被内核拒了。
fn annotate_kernel_denial(report: &mut ExecReport, kernel_denies_outbound: bool) {
    if report.output.contains(KERNEL_DENIAL_MARK) {
        return;
    }
    if !report
        .capabilities
        .as_ref()
        .is_some_and(|c| c.file_runtime_enforced)
    {
        return;
    }
    let lower = report.output.to_ascii_lowercase();
    if !OS_DENY_PATTERNS.iter().any(|p| lower.contains(p)) {
        return;
    }
    // 网络：只在本次运行**确实**拒绝所有出站时这么写；否则明说没在沙箱层拦，
    // 免得模型把「连不上」归因到沙箱而改道（会话 2026_09_14_00_22_47：curl 的真实
    // 死因是读不到 /private/etc/ssl/openssl.cnf，TLS 初始化就崩了，与出站策略无关）。
    let net = if kernel_denies_outbound {
        "④ 网络：本次运行的沙箱策略拒绝所有出站连接（联网白名单已开启但名单为空）——联网命令会失败；"
    } else {
        "④ 网络：本次运行未在内核层限制出站（若仍连不上，那是目标不可达 / 代理 / DNS 的问题，不是沙箱拦的）；"
    };
    report.output.push_str(&format!(
        "\n\n{KERNEL_DENIAL_MARK} 该失败带内核拒绝特征（Operation not permitted / Permission denied）。\
         若确系沙箱拦截，被拒的原因通常是下面之一（失败原文里一般带着具体路径，先照它定位）——\
         ① 读：沙箱只放行系统区（/usr、/System、/bin、/sbin、/dev、/private/etc 等）与会话工作区；\
         工作区之外的读（如 /Applications、~/.ssh、~/Library）会被拒——\
         `curl` 读不到 TLS 信任配置也会以这个错误失败；\
         ② 写：只放行会话工作区（外加 /dev/null 等零副作用设备与沙箱 HOME），工作区之外一律拒绝；\
         ③ 执行：只放行系统目录、已登记的工具链目录与**会话工作区内的可执行文件**；\
         若报错来自 `xcodebuild`/`cc` 的许可或框架加载，注意沙箱内 Xcode 的 xcodebuild 不可用\
         （依赖 SIP 受限框架），可用的是 Command Line Tools：`cc`/`clang`/`xcrun` 与 `rustc` 应能直接编译；\
         ④ 环境：$HOME 是按工作区持久的沙箱目录（缓存跨命令保留），$TMPDIR 逐命令临时；\
         真实用户配置（~/.ssh、~/.gitconfig）不可读，把它需要的东西放进会话工作区；\
         {net}\
         可行动作：把要用的东西放进会话工作区；改用已放行的工具；非常规位置的编译器/工具链让用户\
         在设置 → 安全配置 → 「额外可执行目录」里放行；工作区外的读写交给用户审批\
         （手动授权会弹审批卡，自动授权按目标扩白名单在沙箱内重跑）。\
         不要对同一路径反复重试。"
    ));
}

/// 硬拒绝：记审计 + 返回**结构化错误信封**（引擎经 `result_meta` 转成分类元数据）。
fn hard_deny(ctx: &react_loop::ToolContext, command: &str, denial: &Denial) -> String {
    policy::audit(
        &ctx.session_id,
        policy::audit_kind_of(denial.category),
        &denial.detail,
        AuditApproval::None,
        Some(command),
        Some(denial.category),
        Some(denial.source),
    );
    denial::denial_error(denial)
}

/// 危险命令 / 资源类硬拦截：返回分类化拒绝（无则 None，表示可能网络/其他）。
///
/// 类别直接取自沙箱事件（`Denied.category` / `Evicted.category`），不再靠中文子串匹配。
fn hard_block_denial(r: &ExecReport) -> Option<Denial> {
    use astorm_sandbox::sandbox::Event;
    for ev in &r.events {
        match ev {
            Event::Denied { category, detail } => match category {
                Category::DangerCommand => {
                    return Some(Denial::new(
                        DenialCategory::DangerCommand,
                        DenialSource::SandboxRuntime,
                        format!("危险命令被拦截：{detail}"),
                    ))
                }
                Category::Resource => {
                    return Some(Denial::new(
                        DenialCategory::Resource,
                        DenialSource::SandboxRuntime,
                        format!("资源限制拦截：{detail}"),
                    ))
                }
                _ => {}
            },
            Event::Evicted { category: Category::Resource, reason } => {
                return Some(Denial::new(
                    DenialCategory::Resource,
                    DenialSource::SandboxRuntime,
                    format!("资源限制拦截：{reason}"),
                ))
            }
            _ => {}
        }
    }
    // 兜底：预检拦截项（旧后端不带分类时，按明细文本判定危险命令）
    for d in &r.denials {
        if d.category == Category::DangerCommand {
            return Some(Denial::new(
                DenialCategory::DangerCommand,
                DenialSource::SandboxPreflight,
                format!("危险命令被拦截：{}", d.detail),
            ));
        }
        if d.category == Category::Resource {
            return Some(Denial::new(
                DenialCategory::Resource,
                DenialSource::SandboxPreflight,
                format!("资源限制拦截：{}", d.detail),
            ));
        }
    }
    None
}

/// 沙箱给出的**结构化文件分类**拒绝（预检 `report.denials` 优先 → 运行时事件）。
///
/// 这是文件类卡片/硬拒的**唯一**运行期来源：沙箱的 `Category::FileRead/FileWrite`
/// 只在它自己能判定的场合产生（如预检文件黑名单、运行时文件事件）。越出沙箱白名单的
/// 文件访问由内核拦下，**不产生任何分类**——那种情况必须按普通命令失败处理，不能靠
/// 事后静态分析猜方向造卡片。
fn classified_file_denial(r: &ExecReport) -> Option<Denial> {
    use astorm_sandbox::sandbox::Event;
    // 预检项（带 category + 原因文本）优先：文件黑名单命中走这里
    if let Some(d) = r
        .denials
        .iter()
        .find(|d| matches!(d.category, Category::FileRead | Category::FileWrite))
    {
        return Some(Denial::new(
            DenialCategory::from(d.category),
            DenialSource::SandboxPreflight,
            format!("沙箱分类拦截：{}", d.detail),
        ));
    }
    // 运行时事件（预检之外的分类化文件拒绝）
    for ev in &r.events {
        if let Event::Denied { category, detail } = ev {
            if matches!(category, Category::FileRead | Category::FileWrite) {
                return Some(Denial::new(
                    DenialCategory::from(*category),
                    DenialSource::SandboxRuntime,
                    format!("沙箱分类拦截：{detail}"),
                ));
            }
        }
    }
    None
}

/// Plan（只读）**静态**写闸门的拒绝描述：命令含任意写目标即硬拒（含会话空间内）。
///
/// 与权限模式**正交**：`permission = auto` 只免审批卡，不放宽只读契约。本闸门先于 auto
/// 分支判定——否则 Auto 会按 `static_outside_write` 命中做**无防护二跑**，直接把文件写到
/// 会话空间外，成为 Plan 只读的逃生口。
fn plan_static_write_denial(command: &str, writes: &str) -> Denial {
    Denial::new(
        DenialCategory::FileWrite,
        DenialSource::StaticPrecheck,
        format!(
            "Plan 模式为只读模式，禁止写文件；命令含写目标：{writes}。Plan 只读与权限模式无关（自动审批也不会放宽）——请切换到 Execute 或 Auto 模式后重试。"
        ),
    )
    .with_targets(writes.split('，').map(str::to_string).collect())
    .with_command(command)
}

/// Plan（只读）对沙箱**分类为写**的拒绝：改写为「切换模式」引导（不走审批）。
fn plan_write_denial(denial: &Denial, command: &str) -> Denial {
    Denial::new(
        DenialCategory::FileWrite,
        denial.source,
        format!(
            "Plan 模式为只读模式，禁止写文件；该命令的文件写被沙箱拦截：{}。临时审批不能绕过——请切换到 Execute 或 Auto 模式后重试。",
            denial.detail
        ),
    )
    .with_targets(denial.targets.clone())
    .with_command(command)
}

/// 是否存在网络类违规（驱逐 / 网络预检）。
fn has_network_violation(r: &ExecReport) -> bool {
    use astorm_sandbox::sandbox::Event;
    r.events.iter().any(|ev| match ev {
        Event::Evicted { category, .. } => category.is_network(),
        Event::Denied { category, .. } => category.is_network(),
        _ => false,
    }) || r
        .denials
        .iter()
        .any(|d| d.category.is_network())
}

/// 网络类违规明细。
fn first_network_detail(r: &ExecReport) -> Option<String> {
    use astorm_sandbox::sandbox::Event;
    for ev in &r.events {
        match ev {
            Event::Evicted { category, reason } if category.is_network() => return Some(reason.clone()),
            Event::Denied { category, detail } if category.is_network() => return Some(detail.clone()),
            _ => {}
        }
    }
    r.denials
        .iter()
        .find(|d| d.category.is_network())
        .map(|d| d.detail.clone())
}

/// 网络类违规涉及的地址（从明细文本中提取，仅用于审批卡 target 展示）。
fn first_network_targets(r: &ExecReport) -> Vec<String> {
    first_network_detail(r)
        .map(|d| {
            d.split_whitespace()
                .filter(|t| t.contains('.') || t.contains(':'))
                .take(3)
                .map(|t| t.to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// 会话白名单是否已覆盖本次网络违规：命中主机（targets 里能识别出的）或命中整条命令。
fn session_allows_network(session_id: &str, command: &str, targets: &[String]) -> bool {
    let Some(app) = crate::app::App::try_global() else {
        return false;
    };
    let registry = app.session_allow();
    if targets.iter().any(|t| {
        crate::tool::session_allow::host_token(t)
            .map(|h| registry.host_allowed(session_id, &h))
            .unwrap_or(false)
    }) {
        return true;
    }
    registry.command_allowed(session_id, command)
}

/// 放行后的重跑方式（P2）：优先**按目标扩白名单、仍在沙箱内跑**。
///
/// 语义变化：此前"放行"= `protection.enabled=false` 整条命令无防护重跑——一次批准
/// （或 Auto 模式下的一次静默命中）就把文件、网络、资源、危险命令四层一起关掉。
/// 现在改为把**本次被拦的目标**并入当次策略白名单，其余约束照旧生效；
/// 只有"用户明确批准"那一路保留无防护兜底（见 `kernel_blocked_again` 处的说明）。
#[derive(Default, Clone)]
struct RerunExt {
    /// 追加的读/写白名单（`{目标}/**`，沙箱 path_rules 按 subpath 放行）
    read: Vec<String>,
    write: Vec<String>,
    /// 追加的网络主机规则（`domain:` / `ip:`，与安全配置同一套语法）
    net_hosts: Vec<String>,
    /// 提不出具体主机（裸 `git push` 这类）：本次**只关掉联网白名单开关**，
    /// 内核不再按方向拒绝出站、主机级只剩黑名单；文件/资源/危险命令照旧受控
    /// —— 这不等于关沙箱（此前那种做法连文件沙箱一起关掉了）。
    net_all: bool,
}

impl RerunExt {
    /// 由目标列表构造。
    ///
    /// `net` **非空**但一条都解析不出主机时退化为 `net_all`（本次不限制出站）：
    /// 用户批准了"要联网"这件事，却给不出具体主机（裸 `git push`），只能放开方向级限制。
    /// `net` 为空 = 本次不碰网络策略。
    fn from_targets(read: Vec<String>, write: Vec<String>, net: Vec<String>) -> Self {
        let requested = !net.is_empty();
        let mut ext = RerunExt {
            read,
            write,
            net_hosts: Vec::new(),
            net_all: false,
        };
        for t in net {
            if let Some(rule) = crate::tool::session_allow::net_rule_literal(&t) {
                if !ext.net_hosts.contains(&rule) {
                    ext.net_hosts.push(rule);
                }
            }
        }
        if requested && ext.net_hosts.is_empty() {
            ext.net_all = true;
        }
        ext
    }

    /// 网络意图存在但提不出主机（如裸 `git push`）：显式要求本次不限制出站。
    fn net_unrestricted(read: Vec<String>, write: Vec<String>) -> Self {
        RerunExt {
            read,
            write,
            net_hosts: Vec::new(),
            net_all: true,
        }
    }

    fn is_empty(&self) -> bool {
        self.read.is_empty() && self.write.is_empty() && self.net_hosts.is_empty() && !self.net_all
    }
}

/// 把本次被拦的目标并入基础策略（**仍在沙箱内**执行）。
fn extend_protection(mut prot: ProtectionConfig, ext: &RerunExt) -> ProtectionConfig {
    for p in &ext.read {
        let g = format!("{p}/**");
        if !prot.file.read_white.contains(&g) {
            prot.file.read_white.push(g);
        }
    }
    for p in &ext.write {
        let g = format!("{p}/**");
        if !prot.file.write_white.contains(&g) {
            prot.file.write_white.push(g);
        }
    }
    if ext.net_all {
        prot.net.use_whitelist = false;
    } else {
        for r in &ext.net_hosts {
            if !prot.net.out_white.contains(r) {
                prot.net.out_white.push(r.clone());
            }
            if !prot.net.in_white.contains(r) {
                prot.net.in_white.push(r.clone());
            }
        }
    }
    prot
}

/// 静态分析出的"会话空间外读 / 写"目标（与预检卡同源；供扩白名单重跑使用）。
fn outside_targets(
    session_id: &str,
    workspace: &Path,
    command: &str,
) -> (Vec<String>, Vec<String>) {
    let analysis = analyze_command(command);
    let mut reads: Vec<String> = Vec::new();
    let mut writes: Vec<String> = Vec::new();
    for f in &analysis.file_refs {
        let p = Path::new(&f.path);
        if !p.is_absolute() {
            continue;
        }
        match f.kind {
            AccessKind::Read => {
                if !policy::path_in_workspace(workspace, p)
                    && !policy::path_allowed_by_security(p, false)
                    && !policy::path_allowed_by_session(session_id, p, false)
                    && !policy::path_in_skills_dir(p)
                    && !policy::path_kernel_readable(p)
                {
                    reads.push(f.path.clone());
                }
            }
            AccessKind::Write => {
                let session_ok = policy::path_allowed_by_session(session_id, p, true);
                if !policy::path_in_workspace(workspace, p)
                    && !policy::path_allowed_by_security(p, true)
                    && !session_ok
                {
                    writes.push(f.path.clone());
                }
            }
        }
    }
    (reads, writes)
}

/// 扩白名单重跑是否**又被挡住**（内核 EPERM 特征 / 网络驱逐 / 预检再次拦截）：
/// 是则说明目标没覆盖到命令真正要访问的东西 —— 用户批准路径下才允许兜底无防护重跑。
fn kernel_blocked_again(r: &ExecReport) -> bool {
    if r.evicted || !r.prevented.is_empty() {
        return true;
    }
    if !r
        .capabilities
        .as_ref()
        .is_some_and(|c| c.file_runtime_enforced)
    {
        return false;
    }
    let lower = r.output.to_ascii_lowercase();
    OS_DENY_PATTERNS.iter().any(|p| lower.contains(p))
}

/// 汇总为既有 JSON 形状 + **沙箱运行期事实**。
///
/// 事实（是否真受防护 / 驱逐原因 / 拦截明细 / 强制层降级说明 / 输出截断）此前只进
/// `report.events`，模型侧完全看不到：命令被内存或网络驱逐时它只拿到 `returncode=137`，
/// 于是把「沙箱拦了」误判成「命令写错了」，再花几轮试别的写法
/// （会话 2026-10-08 的 `node_modules` 疑案就叠了这层误导）。
///
/// `session_id` / `cwd` 仅用于**轻量执行审计**（每次执行一条 `bash_exec` 记录）：
/// 审批 / 拦截记录只在「有决策」时写，普通成功命令此前在审计页完全无痕。
fn format_json_result(command: &str, report: ExecReport, session_id: &str, cwd: &Path) -> String {
    let cancelled = report.killed && report.exit_code.is_none() && !report.evicted;
    let timed_out = report.killed && !cancelled;
    let (stdout, stderr, truncated) = split_output(&report.output, &report.stderr);

    let mut denied: Vec<String> = Vec::new();
    let mut evicted: Vec<String> = Vec::new();
    let mut notes: Vec<String> = Vec::new();
    let mut output_capped = false;
    for ev in &report.events {
        match ev {
            Event::Denied { detail, .. } => denied.push(clip_fact(detail)),
            Event::Evicted { reason, .. } => evicted.push(clip_fact(reason)),
            Event::Note(note) => notes.push(clip_fact(note)),
            Event::OutputTruncated => output_capped = true,
            _ => {}
        }
    }
    denied.truncate(FACT_MAX_ITEMS);
    evicted.truncate(FACT_MAX_ITEMS);
    notes.truncate(FACT_MAX_ITEMS);
    let degraded: Vec<String> = report
        .capabilities
        .as_ref()
        .map(|c| {
            c.degraded_reasons
                .iter()
                .take(FACT_MAX_ITEMS)
                .map(|s| clip_fact(s))
                .collect()
        })
        .unwrap_or_default();
    // 是否**真的**跑在内核沙箱里：总开关关闭、兜底无防护重跑、强制层降级
    // 都会是 passthrough。模型据此知道这条结果不能按沙箱语义解读
    // （例如无防护重跑继承的是真实 HOME，而不是逐命令临时 HOME）。
    let protected = report.protection_on && report.backend != "passthrough";
    // 策略字段级强制力自曝：只带**非 Enforced** 项（Approximate / NotSupported 才是
    // 模型规划命令时需要知道的边界：采样/轮询/未实现），Enforced 项不进结果以免膨胀。
    // 防护关闭时 report.enforcement 为空（无策略在生效）→ 空数组。
    let enforcement: Vec<String> = report
        .enforcement
        .iter()
        .filter(|f| !f.state.is_enforced())
        .map(|f| clip_fact(&f.line()))
        .collect();
    // 轻量执行审计：每次 bash 执行一条（type=bash_exec），与「有决策才记」的审批 / 拦截
    // 记录互补，供审计页按会话回放普通命令「跑没跑在沙箱里、超时没有、被驱逐没有」。
    // 每次 execute 只有最终报告走到这里（扩白名单 / 无防护二跑只取最后一次报告），
    // 故不会为同一次工具调用重复记账；detail 保持单行，便于 grep。
    let mut detail = format!("执行完成：exit={}", report.exit_code.unwrap_or(-1));
    if protected {
        detail.push_str(" protected=true");
    } else {
        // 无防护（总开关关闭 / 强制层降级 / 兜底重跑）：用中文事实标出，而不是 protected=false
        detail.push_str(" 无防护");
    }
    if timed_out {
        detail.push_str(" timed_out=true");
    }
    if let Some(reason) = evicted.first() {
        detail.push_str(&format!(" evicted={reason}"));
        if evicted.len() > 1 {
            detail.push_str(&format!("（另 {} 项）", evicted.len() - 1));
        }
    }
    detail.push_str(&format!(" elapsed={}ms cwd={}", report.elapsed_ms, cwd.display()));
    policy::audit(
        session_id,
        crate::tool::audit::AuditKind::BashExec,
        &detail,
        AuditApproval::None,
        Some(command),
        None,
        None,
    );
    json!({
        "command": command,
        "returncode": report.exit_code.unwrap_or(-1),
        "stdout": stdout,
        "stderr": stderr,
        "cancelled": cancelled,
        "timed_out": timed_out,
        "sandbox": {
            "protected": protected,
            "backend": report.backend,
            "evicted": evicted,
            "denied": denied,
            "notes": notes,
            "degraded": degraded,
            "output_truncated": output_capped || truncated,
            "enforcement": enforcement,
        },
    })
    .to_string()
}

/// 从工具结果 JSON 提取前端工具卡要用的**沙箱事实**
/// （`{protected, evicted, timed_out, output_truncated}`）。
///
/// 严格纯 JSON：结果不是 JSON、没有 `sandbox` 对象（拒绝信封 / 普通失败文本）→ None，
/// 调用侧不应受影响。只挑这四个字段：元数据会随消息落库，保持紧凑；
/// `timed_out` 在结果 JSON 里位于顶层（sandbox 外），并入此处以统一前端结构。
fn sandbox_facts_from_result(result: &str) -> Option<serde_json::Value> {
    let v: serde_json::Value = serde_json::from_str(result.trim()).ok()?;
    let sb = v.get("sandbox")?.as_object()?;
    let mut out = serde_json::Map::new();
    if let Some(p) = sb.get("protected") {
        out.insert("protected".to_string(), p.clone());
    }
    if let Some(e) = sb.get("evicted") {
        out.insert("evicted".to_string(), e.clone());
    }
    out.insert(
        "timed_out".to_string(),
        v.get("timed_out").cloned().unwrap_or(serde_json::Value::Bool(false)),
    );
    if let Some(t) = sb.get("output_truncated") {
        out.insert("output_truncated".to_string(), t.clone());
    }
    Some(serde_json::Value::Object(out))
}

/// 运行期事实文本裁剪：单条过长（降级说明常是整段中文）会撑大工具结果。
fn clip_fact(text: &str) -> String {
    if text.chars().count() <= FACT_MAX_CHARS {
        text.to_string()
    } else {
        let head: String = text.chars().take(FACT_MAX_CHARS).collect();
        format!("{head}…")
    }
}

/// 单条输出流的裁剪：**保留头 + 尾**，中间省略。
///
/// 只留开头会把错误整段切掉——错误通常在末尾，`[沙箱提示]` 也追加在末尾；
/// 模型看到「命令没有输出」比看到截断标记更有害。
fn clip_stream(text: &str) -> (String, bool) {
    let trimmed = text.trim_end();
    let total = trimmed.chars().count();
    if total <= OUTPUT_MAX_CHARS {
        return (trimmed.to_string(), false);
    }
    let head: String = trimmed.chars().take(OUTPUT_MAX_CHARS - OUTPUT_TAIL_CHARS).collect();
    let tail: String = trimmed.chars().skip(total - OUTPUT_TAIL_CHARS).collect();
    (
        format!("{head}\n…（中间省略 {} 字符）…\n{tail}", total - OUTPUT_MAX_CHARS),
        true,
    )
}

/// stdout / stderr 各自裁剪（沙箱自 2026-10-08 起把两路分开上报，此前 stderr 恒为空）。
fn split_output(output: &str, stderr: &str) -> (String, String, bool) {
    let (out, capped_out) = clip_stream(output);
    let (err, capped_err) = clip_stream(stderr);
    (out, err, capped_out || capped_err)
}

/// 命令位 cwd 切换的**放行判定**（P2 放宽）。
///
/// 规则：`cd` / `pushd` 的目标**落在会话工作区内**（相对路径按当前 cwd 解析）→ 放行，
/// 常见构建脚本写法（`cd build && make`、`(cd sub && npm test)`）不再被整条拒绝；
/// 以下情况仍拒绝并提示改用 `cwd` 参数：
/// - 目标在工作区外（`cd /tmp`、`cd ../..` 越界）；
/// - 目标无法可靠解析（变量 `$X`、`~`、上一个目录 `-`、通配 `*`、命令替换）；
/// - `popd`（依赖目录栈，静态推不出落点）与 `chdir`。
///
/// 为什么可以放宽：`cd` 到工作区内不改变「命令始终在会话空间内执行」这条不变量；
/// 写保护由内核白名单兜底（工作区外的写会被 Seatbelt 拒），因此静态分析对相对路径的
/// 解析偏差不会造成越界写。
fn cwd_change_blocked(command: &str, cwd: &Path, workspace: &Path) -> Option<String> {
    let (kw, after) = find_cwd_change_at(command)?;
    if kw == "popd" || kw == "chdir" {
        return Some(kw.to_string());
    }
    let Some(target) = cwd_target_after(command, after) else {
        return Some(format!("{kw}（未给出目标）"));
    };
    if target == "-"
        || target.starts_with('$')
        || target.starts_with('~')
        || target.contains('*')
        || target.contains('`')
    {
        return Some(format!("{kw} {target}"));
    }
    let p = Path::new(&target);
    let resolved = if p.is_absolute() {
        p.to_path_buf()
    } else {
        cwd.join(p)
    };
    if policy::path_in_workspace(workspace, &resolved) {
        None
    } else {
        Some(format!("{kw} {target}"))
    }
}

/// 取 cwd 关键字之后的**目标 token**（跳过 `-L`/`-P`/`-e`/`--` 之类的开关）。
/// 返回 `None` = 没有目标（`cd` 单独出现 → 到 $HOME）。
fn cwd_target_after(command: &str, from: usize) -> Option<String> {
    let rest = command.get(from..)?;
    for raw in rest.split_whitespace() {
        let t = raw.trim_matches(|c: char| {
            matches!(c, '\'' | '"' | ';' | '&' | '|' | ')' | '(' | '`')
        });
        if t.is_empty() || t == "--" || (t.starts_with('-') && t.len() > 1) {
            continue;
        }
        return Some(t.to_string());
    }
    None
}

/// 识别命令中的 cwd 切换关键字（cd / pushd / popd / chdir）。
///
/// 仅在**命令位**判定（语句起点，或 `&&`/`||`/`;`/`|`/`(`/换行 之后且剥掉前导空白），
/// 且为完整词（后随空白 / 分隔符 / 行尾），从而避免把 `echo cd`、引号内的 `"cd x"`、
/// 变量内容等误判。返回命中的关键字，未命中返回 None。
/// 同上，但额外返回**关键字结束处的字节偏移**（供 [`cwd_change_blocked`] 取目标 token）。
fn find_cwd_change_at(command: &str) -> Option<(&'static str, usize)> {
    const KWS: [&str; 4] = ["pushd", "popd", "chdir", "cd"];
    let b = command.as_bytes();
    let mut i = 0;
    // 当前是否处于“可能是一条新命令的开头”（行首或紧跟分隔符后）
    let mut at_cmd_start = true;
    let mut quote: Option<u8> = None; // ' 或 "
    while i < b.len() {
        let c = b[i];
        match quote {
            Some(q) => {
                if c == b'\\' && q == b'"' {
                    i += 2;
                    continue;
                }
                if c == q {
                    quote = None;
                }
                i += 1;
                continue;
            }
            None => {}
        }
        match c {
            b'\'' | b'"' => {
                quote = Some(c);
                at_cmd_start = false;
                i += 1;
                continue;
            }
            b' ' | b'\t' => {
                i += 1;
                continue; // 前导空白不重置 cmd start；词后空白由下方词边界处理
            }
            b'&' => {
                // && 或 & 后为新命令
                i += if i + 1 < b.len() && b[i + 1] == b'&' { 2 } else { 1 };
                at_cmd_start = true;
                continue;
            }
            b'|' | b';' | b'(' | b'\n' => {
                // |、||、;、换行、子 shell ( 后为新命令
                i += 1;
                at_cmd_start = true;
                continue;
            }
            b')' => {
                at_cmd_start = false;
                i += 1;
                continue;
            }
            _ => {}
        }
        if !at_cmd_start {
            // 非命令位：跳到下一分隔符，期间不重置
            i += 1;
            continue;
        }
        // 命令位：尝试匹配关键字
        let rest = &command[i..];
        let matched = KWS.iter().find(|k| rest.starts_with(**k)).copied();
        if let Some(kw) = matched {
            // 必须是完整词：后随空白/分隔符/行尾/引号起始
            let after = rest.as_bytes().get(kw.len()).copied();
            let is_word_end = match after {
                None => true,
                Some(c) => matches!(c, b' ' | b'\t' | b'\n' | b';' | b'|' | b'&' | b'\'' | b'"' | b')'),
            };
            if is_word_end {
                return Some((kw, i + kw.len()));
            }
            // 词前缀但不完整（如 cdrom）：不算
        }
        // 已消费掉命令关键字占位：跳过整词，令其后为新状态（同一语句内后面的关键字也会扫到）
        i += rest.find(|ch: char| ch.is_whitespace() || matches!(ch, ';' | '|' | '&')).map(|o| o + 1).unwrap_or(rest.len());
        at_cmd_start = false;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 只取关键字的便捷包装（识别结果的偏移由 `cd放宽到工作区内` 间接覆盖）。
    fn find_kw(cmd: &str) -> Option<&'static str> {
        find_cwd_change_at(cmd).map(|(kw, _)| kw)
    }

    #[test]
    fn cwd_switch_detection() {
        assert_eq!(find_kw("cd /tmp"), Some("cd"));
        assert_eq!(find_kw("cd /tmp && echo hi"), Some("cd"));
        assert_eq!(find_kw("echo x; cd .."), Some("cd"));
        assert_eq!(find_kw("(cd sub) && make"), Some("cd"));
        assert_eq!(find_kw("pushd src"), Some("pushd"));
        assert_eq!(find_kw("make && popd"), Some("popd"));
        assert_eq!(find_kw("chdir /"), Some("chdir"));
        // 非切换目录：不作为 cd 参数 / 引号内 / 别词
        assert_eq!(find_kw("ls cd dir"), None);
        assert_eq!(find_kw("echo \"cd /tmp\""), None);
        assert_eq!(find_kw("grep cd file.txt"), None);
        assert_eq!(find_kw("cdrom"), None);
        assert_eq!(find_kw("pwd"), None);
        assert_eq!(find_kw(""), None);
    }

    #[cfg(unix)]
    #[test]
    fn 零副作用重定向不算空间外写() {
        let ws = Path::new("/tmp/astorm-ws");
        // `2>/dev/null;` 的 `;` 粘在目标上，曾被判为空间外写 `/dev/null;`：
        // Plan 模式直接硬拒（「请切换到 Execute 或 Auto 模式」），Execute 模式弹多余审批。
        assert_eq!(static_outside_write(Some("s-test"), ws, "ls /Users 2>/dev/null;"), None);
        assert_eq!(static_outside_write(Some("s-test"), ws, "ls /Users 2>/dev/null&&echo ok"), None);
        assert_eq!(static_outside_write(Some("s-test"), ws, "ls /Users >/dev/null 2>&1;"), None);
        // 真空间外写仍拦截，且报出裁剪后的干净路径（用于审批文案 / 白名单匹配）
        assert_eq!(
            static_outside_write(Some("s-test"), ws, "echo hi > /tmp/outside.txt;").as_deref(),
            Some("/tmp/outside.txt")
        );
    }

    /// 静态分析器对**只读命令**的识别（回归保护）：`ls` / `stat` 曾被漏登记，
    /// 提不出读引用 → 运行期归因判成写 → Plan 模式被拒成「禁止写文件」。
    /// 现在只读命令表已补齐，预检即可判定为空间外**读**并走审批（不再依赖事后猜测）。
    #[test]
    fn 只读命令被静态分析识别为读引用() {
        let read_refs = |cmd: &str| -> Vec<String> {
            analyze_command(cmd)
                .file_refs
                .iter()
                .filter(|f| f.kind == AccessKind::Read)
                .map(|f| f.path.clone())
                .collect()
        };
        assert!(
            read_refs("ls -la /Users/x/Desktop 2>&1").contains(&"/Users/x/Desktop".to_string()),
            "ls 应产出读引用（只读命令表）"
        );
        assert!(
            read_refs("stat /etc/hosts").contains(&"/etc/hosts".to_string()),
            "stat 应产出读引用"
        );
        // 重定向仍是写：`ls /etc > /tmp/out` 的写引用与读引用并存
        let write_refs: Vec<String> = analyze_command("ls /etc > /tmp/astorm-out.txt")
            .file_refs
            .iter()
            .filter(|f| f.kind == AccessKind::Write)
            .map(|f| f.path.clone())
            .collect();
        assert!(write_refs.contains(&"/tmp/astorm-out.txt".to_string()), "重定向应产出写引用");
        // 解释器内读写静态分析看不到（零引用）→ 进沙箱后由内核拦，且无结构化分类
        assert!(
            analyze_command("python3 -c 'open(\"/x\").read()'").file_refs.is_empty(),
            "解释器内读写应零引用"
        );
    }

    /// Plan 只读的静态写闸门：**任意**写目标（含会话空间内 / 相对路径）都算违背只读契约；
    /// 零副作用设备与纯读命令不受影响。
    #[test]
    fn plan_any_write_覆盖空间内外与相对路径() {
        let ws = "/tmp/astorm-ws";
        // 空间外绝对路径写
        assert_eq!(
            plan_any_write("echo x > /tmp/astorm-out.txt").as_deref(),
            Some("/tmp/astorm-out.txt")
        );
        // 会话空间内写（file_operation 的 Plan 语义同样禁止，两者需对齐）
        assert_eq!(
            plan_any_write(&format!("echo x > {ws}/inner.txt")).as_deref(),
            Some(format!("{ws}/inner.txt").as_str())
        );
        // 相对路径写（cwd = 工作区）也算写
        assert_eq!(plan_any_write("echo x > out.txt").as_deref(), Some("out.txt"));
        // 纯读命令 / 零副作用重定向 → 无写目标（`2>/dev/null` 惯用法不受影响）
        assert_eq!(plan_any_write("cat /etc/hosts"), None);
        assert_eq!(plan_any_write("ls /Users 2>/dev/null;"), None);
        assert_eq!(plan_any_write("cargo --version >/dev/null 2>&1"), None);
    }

    /// 造一份最小的沙箱报告（capabilities=None → degraded 为空）。
    fn report_with(events: Vec<Event>, protection_on: bool, backend: &str) -> ExecReport {
        ExecReport {
            started_at: String::new(),
            ended_at: String::new(),
            elapsed_ms: 0,
            command: "echo hi".to_string(),
            mode: ExecMode::Sync,
            protection_on,
            strict: false,
            backend: backend.to_string(),
            capabilities: None,
            enforcement: Vec::new(),
            exit_code: Some(0),
            output: String::new(),
            stderr: String::new(),
            killed: false,
            evicted: false,
            prevented: Vec::new(),
            denials: Vec::new(),
            events,
            ok: true,
        }
    }

    /// P2：`cd` 放宽到工作区内，切出工作区/不可解析目标仍拒。
    #[test]
    fn cd放宽到工作区内() {
        let ws = Path::new("/tmp/astorm-ws-cd");
        let cwd = ws.join("app"); // 当前 cwd 是工作区子目录（工具的 cwd 参数）
        let ok = [
            "cd sub && make",
            "(cd sub) && npm test",
            "cd ./packages/a && yarn build",
            "cd /tmp/astorm-ws-cd/backend && cargo build",
            "pushd src && ls && popd", // 注意：popd 单独判定，见下
            "echo hi; cd build; make",
        ];
        // pushd 允许（目标在工作区内）；popd 单独出现仍拒（静态推不出落点）
        for c in ok {
            if c.contains("popd") {
                continue;
            }
            assert_eq!(
                cwd_change_blocked(c, &cwd, ws),
                None,
                "工作区内的 cd 应放行: {c}"
            );
        }
        let blocked = [
            "cd /tmp",
            "cd /etc && ls",
            "cd ../..",
            "cd ../../outside",
            "cd ~",
            "cd ~/proj",
            "cd $HOME/x",
            "cd $SESSION_WORKSPACE_OTHER",
            "cd -",
            "cd",
            "cd *.bak",
            "cd /Users/x/.ssh",
            "popd",
            "chdir /tmp/astorm-ws-cd",
        ];
        for c in blocked {
            assert!(
                cwd_change_blocked(c, &cwd, ws).is_some(),
                "应拒绝: {c}"
            );
        }
        // 引号内的 cd 不是命令 → 不算
        assert_eq!(cwd_change_blocked("echo \"cd /tmp\"", &cwd, ws), None);
        assert_eq!(cwd_change_blocked("grep cd file.txt", &cwd, ws), None);
    }

    /// 造一份"内核强制生效"的能力描述（供 kernel_blocked_again 用例）。
    fn caps(enforced: bool) -> astorm_sandbox::sandbox::SandboxCapabilities {
        use astorm_sandbox::sandbox::{KernelNetMode, Platform, ResourceCapabilities};
        astorm_sandbox::sandbox::SandboxCapabilities {
            platform: Platform::MacOs,
            file_runtime_enforced: enforced,
            net_runtime_enforced: KernelNetMode::EvictionOnly,
            command_runtime_intercept: true,
            resource: ResourceCapabilities::none(),
            degraded_reasons: Vec::new(),
            admin: false,
        }
    }

    /// P2：放行后的重跑改为"按目标扩白名单、**仍在沙箱内**"，而不是关沙箱。
    #[test]
    fn 放行重跑按目标扩白名单而非关沙箱() {
        let base = ProtectionConfig::default();
        // 读/写目标 → 各自白名单追加 `{目标}/**`
        let ext = RerunExt::from_targets(
            vec!["/tmp/rd.txt".to_string()],
            vec!["/tmp/wr.txt".to_string()],
            Vec::new(),
        );
        let p = extend_protection(base.clone(), &ext);
        assert!(p.enabled, "扩白名单不是关沙箱");
        assert!(p.file.read_white.contains(&"/tmp/rd.txt/**".to_string()));
        assert!(p.file.write_white.contains(&"/tmp/wr.txt/**".to_string()));
        assert!(p.net.use_whitelist, "没提网络就不动网络策略");

        // 具体主机 → 出/进白名单各加一条（同一套 domain:/ip: 语法）
        let ext = RerunExt::from_targets(vec![], vec![], vec!["api.example.com:443".to_string()]);
        assert!(!ext.net_all);
        let p = extend_protection(base.clone(), &ext);
        assert!(p.net.out_white.contains(&"domain:api.example.com:443".to_string()));
        assert!(p.net.in_white.contains(&"domain:api.example.com:443".to_string()));

        // 提不出主机（裸 `git push`）→ 只关联网白名单开关；文件/资源/危险命令仍受控
        let ext = RerunExt::from_targets(vec![], vec![], vec!["被默认拒绝".to_string()]);
        assert!(ext.net_all);
        let p = extend_protection(base.clone(), &ext);
        assert!(!p.net.use_whitelist, "本次不限制出站（主机级只剩黑名单）");
        assert!(p.enabled, "仍然受防护执行");

        // 显式 net_unrestricted / 空扩展
        assert!(RerunExt::net_unrestricted(vec![], vec![]).net_all);
        assert!(RerunExt::default().is_empty());
        assert!(!RerunExt::from_targets(vec![], vec![], Vec::new()).net_all);
    }

    /// 空间外目标提取：只并入真正会被拒的路径（系统区/工具链目录本就可读，不算）。
    #[cfg(unix)]
    #[test]
    fn 空间外目标提取只含真空间外路径() {
        let ws = Path::new("/tmp/astorm-ws-p2");
        let (reads, writes) =
            outside_targets("sess-p2", ws, "echo x > /tmp/astorm-p2-out.txt; cat /etc/hosts");
        assert!(writes.iter().any(|p| p == "/tmp/astorm-p2-out.txt"), "{writes:?}");
        assert!(reads.is_empty(), "系统区读不算空间外（内核本就放行）: {reads:?}");
        let (reads, _) = outside_targets("sess-p2", ws, "cat /Users/x/.ssh/id_rsa");
        assert!(reads.iter().any(|p| p.contains(".ssh")), "{reads:?}");
        // 工作区内写不进扩展
        let (_, writes) = outside_targets("sess-p2", ws, "echo x > /tmp/astorm-ws-p2/a.txt");
        assert!(!writes.iter().any(|p| p.ends_with("a.txt")), "{writes:?}");
    }

    /// "扩白名单后又被挡住"的判定：内核 EPERM / 驱逐 / 预检再次拦截都算（→ 用户批准路径兜底无防护）。
    #[test]
    fn 扩白名单再次被挡住的判定() {
        let mut ok = report_with(Vec::new(), true, "macos-seatbelt");
        ok.capabilities = Some(caps(true));
        ok.output = "all good".to_string();
        assert!(!kernel_blocked_again(&ok));

        let mut denied = report_with(Vec::new(), true, "macos-seatbelt");
        denied.capabilities = Some(caps(true));
        denied.output = "head: /x: Operation not permitted".to_string();
        assert!(kernel_blocked_again(&denied));

        let mut evicted = report_with(Vec::new(), true, "macos-seatbelt");
        evicted.capabilities = Some(caps(true));
        evicted.evicted = true;
        assert!(kernel_blocked_again(&evicted));

        let mut preflight = report_with(Vec::new(), true, "macos-seatbelt");
        preflight.capabilities = Some(caps(true));
        preflight.prevented = vec!["命中文件写黑名单".to_string()];
        assert!(kernel_blocked_again(&preflight));

        // 强制层未生效（无内核强制）时，EPERM 文本不是沙箱拦的 → 不触发兜底
        let mut degraded = report_with(Vec::new(), true, "passthrough");
        degraded.capabilities = Some(caps(false));
        degraded.output = "Operation not permitted".to_string();
        assert!(!kernel_blocked_again(&degraded));
    }

    /// 静态预检的读豁免与内核读白名单同源：系统区/工具链目录不该弹「空间外读」卡
    /// （会话 2026-10-08：`which git; /usr/bin/git --version` 也弹了一张卡）。
    #[cfg(unix)]
    #[test]
    fn 内核默认可读路径不弹空间外读卡() {
        assert!(policy::path_kernel_readable(Path::new("/usr/bin/git")));
        assert!(policy::path_kernel_readable(Path::new("/etc/hosts")));
        assert!(policy::path_kernel_readable(Path::new("/bin/echo")));
        assert!(!policy::path_kernel_readable(Path::new("/Applications/Xcode.app/x")));
        assert!(!policy::path_kernel_readable(Path::new(
            "/Users/x/.ssh/id_rsa"
        )));
    }

    /// 端到端：会话里的真实命令不再弹卡，真正的空间外读仍然弹卡。
    #[cfg(unix)]
    #[test]
    fn 静态读预检_系统路径放行_私密路径仍拦截() {
        let ctx = react_loop::ToolContext::new(tokio_util::sync::CancellationToken::new())
            .with_session_id("sess-precheck");
        let ws = Path::new("/tmp/astorm-precheck-ws");
        // 会话 2026-10-08 的真实命令（当时白弹一张卡）
        assert!(
            precheck_read_askable(&ctx, ws, "which git; /usr/bin/git --version 2>&1 | head -2").is_none(),
            "系统区的读不应弹卡"
        );
        assert!(
            precheck_read_askable(&ctx, ws, "ls /opt/homebrew/bin | head").is_none(),
            "工具链目录的读不应弹卡"
        );
        assert!(
            precheck_read_askable(&ctx, ws, "cat /tmp/astorm-precheck-ws/a.txt").is_none(),
            "工作区内的读不应弹卡"
        );
        // 真正的空间外读仍要卡
        assert!(
            precheck_read_askable(&ctx, ws, "cat /Users/x/.ssh/id_rsa").is_some(),
            "私钥属于真正的空间外读，必须弹卡"
        );
    }

    /// 工具结果必须带上沙箱事实：模型此前只看得到 returncode=137，
    /// 无从判断「被驱逐 / 被拦截 / 根本没在沙箱里跑」。
    #[test]
    fn 工具结果带沙箱事实与头尾截断() {
        let mut long = String::new();
        for i in 0..3000 {
            long.push_str(&format!("line {i}\n"));
        }
        let mut rep = report_with(
            vec![
                Event::Evicted {
                    category: Category::Resource,
                    reason: "进程树内存超过上限，已终止进程树".to_string(),
                },
                Event::Denied {
                    category: Category::DangerCommand,
                    detail: "子进程命中危险命令黑名单，已实时终止".to_string(),
                },
            ],
            false,
            "passthrough",
        );
        rep.output = long;
        let v: serde_json::Value =
            serde_json::from_str(&format_json_result("echo hi", rep, "sess-test", Path::new("/ws")))
                .expect("结果应为合法 JSON");
        assert_eq!(v["sandbox"]["protected"], false, "无防护重跑必须标出来");
        assert_eq!(v["sandbox"]["backend"], "passthrough");
        assert_eq!(
            v["sandbox"]["evicted"][0],
            "进程树内存超过上限，已终止进程树"
        );
        assert_eq!(v["sandbox"]["denied"][0], "子进程命中危险命令黑名单，已实时终止");
        assert_eq!(v["sandbox"]["output_truncated"], true);
        let out = v["stdout"].as_str().unwrap();
        assert!(out.starts_with("line 0"), "头部应保留: {out:.80}");
        assert!(out.contains("中间省略"), "应标出中间省略");
        assert!(
            out.trim_end().ends_with("line 2999"),
            "**尾部必须保留**（错误与 [沙箱提示] 都在末尾）: {out:.120}"
        );
    }

    /// 工具卡事实：成功结果里的沙箱事实要进 `result_meta`（供前端标注无防护 / 驱逐 / 截断），
    /// 且非 JSON（拒绝信封 / 普通失败文本）时静默跳过，不影响既有 `denial` 形状。
    #[test]
    fn 结果元数据带沙箱事实且非JSON静默() {
        use crate::tool::Tool as _;
        let rep = report_with(
            vec![Event::Evicted {
                category: Category::Resource,
                reason: "进程树内存超过上限，已终止进程树".to_string(),
            }],
            false,
            "passthrough",
        );
        let result = format_json_result("echo hi", rep, "sess-meta", Path::new("/ws"));
        // 成功（ok=true）也要给前端事实；不写 status（引擎按 ok 默认 ok）
        let meta = ShellTool.result_meta(true, &result).expect("成功结果应带上沙箱事实");
        assert_eq!(meta["sandbox"]["protected"], false);
        assert_eq!(meta["sandbox"]["evicted"][0], "进程树内存超过上限，已终止进程树");
        assert_eq!(meta["sandbox"]["timed_out"], false);
        assert_eq!(meta["sandbox"]["output_truncated"], false);
        assert!(meta.get("status").is_none(), "成功路径不应改状态: {meta}");

        // 拒绝信封（无 sandbox 字段）→ 仍只产出 denial 元数据
        let envelope = denial::denial_error(&Denial::new(
            DenialCategory::DangerCommand,
            DenialSource::SandboxRuntime,
            "危险命令",
        ));
        let dmeta = ShellTool.result_meta(false, &envelope).expect("拒绝信封应产出分类元数据");
        assert_eq!(dmeta["status"], "error");
        assert_eq!(dmeta["denial"]["category"], "danger_command");
        assert!(dmeta.get("sandbox").is_none(), "信封无事实时不应硬造: {dmeta}");

        // 普通失败文本（非 JSON）→ 不产出元数据，也不 panic
        assert!(ShellTool.result_meta(false, "工具 'bash' 执行失败: boom").is_none());
    }

    /// 字段级强制力自曝：结果 JSON 只带**非 Enforced** 项（近似/不支持才是需要模型知道的
    /// 边界），Enforced 项不出现；无策略（防护关闭/未编译）时给空数组，不硬造。
    #[test]
    fn 结果只带非强制项() {
        use astorm_sandbox::enforcement::{EnforcementState, FieldEnforcement};
        let mut rep = report_with(Vec::new(), true, "macos-seatbelt");
        rep.enforcement = vec![
            FieldEnforcement {
                field: "file.read_black".to_string(),
                state: EnforcementState::Enforced,
                mechanism: "Seatbelt 黑名单行先于放行".to_string(),
                caveat: None,
            },
            FieldEnforcement {
                field: "resource.memory_mb".to_string(),
                state: EnforcementState::Approximate,
                mechanism: "50ms RSS 采样驱逐".to_string(),
                caveat: Some("瞬时冲高可能漏过".to_string()),
            },
            FieldEnforcement {
                field: "resource.disk_mb".to_string(),
                state: EnforcementState::NotSupported,
                mechanism: "未实现".to_string(),
                caveat: Some("Windows 侧不读取".to_string()),
            },
        ];
        let result = format_json_result("echo hi", rep, "sess-enf", Path::new("/ws"));
        let v: serde_json::Value = serde_json::from_str(&result).expect("结果应为合法 JSON");
        let list = v["sandbox"]["enforcement"]
            .as_array()
            .expect("sandbox.enforcement 应为数组");
        assert_eq!(list.len(), 2, "只保留非 Enforced 项: {list:?}");
        assert_eq!(
            list[0].as_str().unwrap(),
            "resource.memory_mb=Approximate: 50ms RSS 采样驱逐(瞬时冲高可能漏过)"
        );
        assert!(list[1]
            .as_str()
            .unwrap()
            .starts_with("resource.disk_mb=NotSupported: "));
        assert!(!result.contains("file.read_black"), "Enforced 项不得进入结果: {result}");

        // 无策略（防护关闭 / 策略未编译）→ 空数组
        let rep = report_with(Vec::new(), true, "macos-seatbelt");
        let v: serde_json::Value =
            serde_json::from_str(&format_json_result("echo hi", rep, "sess-enf2", Path::new("/ws")))
                .unwrap();
        assert_eq!(v["sandbox"]["enforcement"], serde_json::json!([]));
    }

    /// stdout / stderr 分流（沙箱按流上报，工具结果分别给出）。
    #[test]
    fn stdout与stderr分流() {
        let mut rep = report_with(Vec::new(), true, "macos-seatbelt");
        rep.output = "正常输出\nerr: 失败\n".to_string();
        rep.stderr = "err: 失败\n".to_string();
        let v: serde_json::Value =
            serde_json::from_str(&format_json_result("cmd", rep, "sess-test", Path::new("/ws")))
                .expect("结果应为合法 JSON");
        assert_eq!(v["stdout"], "正常输出\nerr: 失败");
        assert_eq!(v["stderr"], "err: 失败");
        assert_eq!(v["sandbox"]["protected"], true);
        assert_eq!(v["sandbox"]["output_truncated"], false);
    }
}
