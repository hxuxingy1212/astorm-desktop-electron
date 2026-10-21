//! `office` 工具：驱动随包分发的 astorm-office 五 CLI
//! （统一入口 astorm-office + json2docx / json2xlsx / json2pptx / json2pdf）。
//!
//! astorm-office 是外部仓库（office-kit 随应用分发），五个 CLI 共享同一命令契约
//! （stdout 只出数据、stderr 走状态、退出码 0/1/2/3、产物目录为中间形态；见 docs/cli-conventions.md
//! 与内置技能 `astorm-office`）。本工具只做**进程适配**：
//! - `bin` 选格式 → 对应可执行文件（`office` = 统一入口，可跨格式 batch / 按扩展名自动分发）；
//!   `args` 原样透传（在会话工作区执行）；
//! - 只读命令（view/validate/extract/qa/…、edit get、meta get、form info、raw）直执行；
//!   写命令走分级授权（Plan 硬拒 → 已拒短路 → 审批 / 自动放行，与 `browser` 工具同一套 policy）；
//! - `serve` / `mcp`（常驻服务）硬拒——工具语义是一次性调用，常驻编辑请逐条调用；
//! - 超时 / 取消（会话打断）杀子进程；输出截断；
//! - `render png` / `convert office` 的外部引擎路径经配置注入 CLI 环境变量
//!   （`ASTORM_CHROME_BIN` / `ASTORM_SOFFICE_PATH`，office 侧探测链最优先）。

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::Path;
use std::time::Duration;

use react_loop::ToolContext;

use super::audit::{AuditApproval, AuditKind};
use super::denial::{Denial, DenialCategory, DenialSource};
use super::policy::{self, ApprovalReq};
use super::{Tool, resolve_workspace};
use crate::app::App;

/// 结果默认截断（字符；数据型输出可能很大，模型按需再 view 细看）。
const MAX_OUTPUT: usize = 8000;

/// 只读子命令（首段，无输出标志时直执行不审批）。
const READ_COMMANDS: [&str; 14] = [
    "view",
    "validate",
    "schema",
    "help",
    "templates",
    "dump",
    "query",
    "env",
    // v0.2 新增：统一入口 / PDF 分析与质检类命令（均只读 stdout）
    "formats",
    "qa",
    "check",
    "palette",
    "design",
    "raw",
];

/// 常驻服务类命令：本工具不支持（stdin JSON 行 / stdio JSON-RPC 会一直等待）。
const DENIED_COMMANDS: [&str; 2] = ["serve", "mcp"];

/// 统一入口的格式别名（`astorm-office <格式> <命令...>`；也接受底层二进制名）。
/// 定位真正执行的子命令时跳过它。
const FORMAT_ALIASES: [&str; 20] = [
    "word",
    "docx",
    "doc",
    "docs",
    "excel",
    "xlsx",
    "xls",
    "sheet",
    "ppt",
    "pptx",
    "slides",
    "presentation",
    "pdf",
    "json2docx",
    "json2xlsx",
    "json2pptx",
    "json2pdf",
    "astorm-office",
    "office",
    "all",
];

/// `office` 工具。
pub struct OfficeTool;

/// 格式 / 统一入口 → CLI 可执行文件名（windows 加 .exe）。
fn bin_name(bin: &str) -> Option<String> {
    let stem = match bin {
        "office" | "astorm" | "astorm-office" | "all" => "astorm-office",
        "docx" | "word" => "json2docx",
        "xlsx" | "excel" => "json2xlsx",
        "pptx" | "ppt" => "json2pptx",
        "pdf" => "json2pdf",
        _ => return None,
    };
    Some(if cfg!(windows) {
        format!("{stem}.exe")
    } else {
        stem.to_string()
    })
}

#[derive(Debug, Deserialize)]
struct OfficeArgs {
    /// 目标格式：docx / xlsx / pptx / pdf；office = 统一入口
    bin: String,
    /// CLI 子命令与参数（原样透传，如 ["edit", "/abs/a.docx", "/part[1]/paragraph[2]", "set"]）
    args: Vec<String>,
    /// 本次调用的目的（展示给用户，与 `browser` 同款可选字段）；仅过程行展示用，不参与执行
    #[serde(default)]
    description: Option<String>,
}

/// 模型填写的本次调用目的（可选）：审批卡与审计里带上，让用户知道**为什么**改这份文档；
/// 未填 / 空串 → None（调用方按无目的处理）。
fn purpose_of(args: &OfficeArgs) -> Option<String> {
    args.description
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// 是否带写盘标志：`-o` / `--output`（含 `--output=x`、`-oDIR` 连写），以及 view 的资源导出
/// （`--attachments` / `--media`）与 `--apply`（content sanitize 写回）。
fn has_output_flag(args: &[String]) -> bool {
    args.iter().any(|a| {
        let a = a.trim();
        a == "-o"
            || a == "--output"
            || a.starts_with("--output=")
            // `-oDIR` 连写形态（如 -oDIR/out.docx）
            || (a.starts_with("-o") && a.len() > 2 && !a.starts_with("--"))
            || a == "--attachments"
            || a.starts_with("--attachments=")
            || a == "--media"
            || a.starts_with("--media=")
            || a == "--apply"
    })
}

/// 定位真正执行的 CLI 子命令下标：跳过全局旗标与统一入口的格式前缀
/// （`astorm-office [-f] <格式> <命令...>`；也接受 `--format=pdf` 与底层二进制名）。
/// 旗标与格式别名可任意先后（如 `word --json view …`）。
fn command_pos(args: &[String]) -> usize {
    let mut i = 0;
    let mut skipped_format = false;
    while let Some(a) = args.get(i) {
        let a = a.trim();
        if matches!(a, "-f" | "--format" | "-o" | "--output") {
            i += 2;
            continue;
        }
        if matches!(a, "--json" | "--quiet" | "--verbose")
            || a.starts_with("--format=")
            || a.starts_with("--output=")
        {
            i += 1;
            continue;
        }
        if !skipped_format && FORMAT_ALIASES.contains(&a.to_lowercase().as_str()) {
            skipped_format = true;
            i += 1;
            continue;
        }
        break;
    }
    i
}

/// 统一入口下判断 extract 是否只读：显式 pdf（或参数里出现 `.pdf`）的
/// text/table/attachment 只读；Office 的 extract 是模板提取（写文件）。
fn extract_read_only(bin: &str, pos: usize, args: &[String]) -> bool {
    let sub = args
        .get(pos + 1)
        .map(|s| s.trim().to_lowercase())
        .unwrap_or_default();
    if !matches!(sub.as_str(), "text" | "table" | "attachment") {
        return false;
    }
    match bin {
        "pdf" => true,
        "office" | "astorm" | "astorm-office" | "all" => {
            let prev = pos
                .checked_sub(1)
                .and_then(|i| args.get(i))
                .map(|s| s.trim());
            match prev {
                Some("pdf") => true,
                // -f 的值或显式格式别名（office 三格式）：extract 写模板
                Some(f) if FORMAT_ALIASES.contains(&f.to_lowercase().as_str()) => false,
                _ => args
                    .iter()
                    .any(|a| a.trim().to_lowercase().ends_with(".pdf")),
            }
        }
        _ => false,
    }
}

/// 本次调用是否只读：命令 ∈ READ_COMMANDS（或 `edit … get` / `meta get` / `form info` / `raw`）
/// 且**无写盘标志**。
fn is_read_only(bin: &str, args: &[String]) -> bool {
    if has_output_flag(args) {
        return false;
    }
    // 帮助 / 版本只输出 stdout
    if args
        .iter()
        .any(|a| matches!(a.trim(), "-h" | "--help" | "-V" | "--version"))
    {
        return true;
    }
    let pos = command_pos(args);
    let cmd = args
        .get(pos)
        .map(|s| s.trim().to_lowercase())
        .unwrap_or_default();
    let sub = args.get(pos + 1).map(|s| s.trim().to_lowercase());
    match cmd.as_str() {
        // `edit <输入> <路径> get`：get 在路径之后，不是紧跟 edit
        "edit" => args.iter().skip(pos + 2).any(|a| a.trim() == "get"),
        "meta" => sub.as_deref() == Some("get"),
        "form" => sub.as_deref() == Some("info"),
        "extract" => extract_read_only(bin, pos, args),
        // batch 可能含写操作；pages 全是写操作；code/content sanitize 会写回
        "raw-set" | "batch" | "pages" | "code" | "content" => false,
        _ => READ_COMMANDS.contains(&cmd.as_str()) || cmd == "raw",
    }
}

/// 审批台账 key（首段命令 + 前几个参数，避免超长命令撑爆台账）。
fn ledger_key(bin: &str, args: &[String]) -> String {
    let joined: Vec<String> = args.iter().take(4).cloned().collect();
    let mut key = format!("office {bin} {}", joined.join(" "));
    if args.len() > 4 {
        key.push_str(" …");
    }
    key
}

/// 涉及目标（denial 展示用）：首个文件参数（输入）与 `-o` / `--media` / `--attachments`
/// 的值（输出）。统一入口先跳过格式别名；树形路径（如 `/part[1]`）不算文件目标。
fn targets_of(args: &[String]) -> Vec<String> {
    let start = command_pos(args) + 1;
    let mut input: Option<String> = None;
    let mut output: Option<String> = None;
    let mut skip_next = false;
    let looks_like_path = |a: &str| a.contains('.') || a.contains('/') || a.contains('\\');
    for a in args.iter().skip(start) {
        let a = a.trim();
        if skip_next {
            if !a.starts_with('-') && output.is_none() {
                output = Some(a.to_string());
            }
            skip_next = false;
            continue;
        }
        match a {
            "-o" | "--output" | "--media" | "--attachments" => {
                skip_next = true;
                continue;
            }
            _ => {}
        }
        if let Some(v) = a
            .strip_prefix("--output=")
            .or_else(|| a.strip_prefix("--media="))
            .or_else(|| a.strip_prefix("--attachments="))
        {
            if output.is_none() {
                output = Some(v.to_string());
            }
            continue;
        }
        // 树形路径（如 /part[1]）不是文件目标
        if a.starts_with('-') || (a.starts_with('/') && a.contains('[')) {
            continue;
        }
        if input.is_none() && looks_like_path(a) {
            input = Some(a.to_string());
        }
    }
    input.into_iter().chain(output).take(2).collect()
}

/// 截断（字符级；中文按 char）。
fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push_str("…（已截断）");
    out
}

#[async_trait]
impl Tool for OfficeTool {
    fn name(&self) -> &str {
        "office"
    }

    fn description(&self) -> &str {
        "驱动 astorm-office 处理 Word/Excel/PPT/PDF：bin 选格式（docx/xlsx/pptx/pdf），或用 office \
统一入口（可跨格式 batch、按扩展名自动分发）；args 原样传 CLI 子命令与参数。四格式命令面一致：\
unpack/repack（产物目录↔二进制）、view（结构感知 Markdown；--revisions 修订稿、--media/--attachments \
导出资源）、edit（树形路径精准修改，如 /part[1]/paragraph[2]）、render（渲染核对）、\
validate / dump / batch / templates / convert（旧格式与 PDF↔Office 互转），PDF 另有 extract\
（文本/表格/图片/内嵌附件/OCR）、pages、meta、form、palette、design、qa。完整用法与路径语法见内置技能 \
astorm-office（先 skills load 再用）。写操作（-o / 就地 edit / batch / 资源导出）需审批，Plan 模式拒绝；\
serve / mcp 常驻服务不可用，请改单命令。"
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "bin": {
                    "type": "string",
                    "enum": ["docx", "xlsx", "pptx", "pdf", "office"],
                    "description": "目标格式（决定调用哪个 CLI）；office = 统一入口（args 首参可为格式别名或 batch/formats，省略格式时按扩展名自动分发）"
                },
                "args": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "CLI 子命令与参数，原样透传。输入/输出路径建议绝对路径（命令在会话工作区执行）；四格式命令面一致，PDF 的子命令见技能分册。建议带全局 --json 取机器可读输出"
                },
                "description": {
                    "type": "string",
                    "description": "一句话说明本次调用的目的（展示给用户，简短；缺省时界面按子命令给通用说法）"
                }
            },
            "required": ["bin", "args"]
        })
    }

    /// 审批拦截结果（纯 JSON）→ 中断载荷：引擎据此发带 `approval` 的 interrupt 事件弹审批卡。
    fn interrupt_payload(&self, result: &str) -> Option<Value> {
        policy::interrupt_request_from_result(result)
    }

    async fn execute(&self, params: Value, ctx: &ToolContext) -> Result<String, String> {
        let args: OfficeArgs =
            serde_json::from_value(params).map_err(|e| format!("office 参数解析失败: {e}"))?;
        let bin = args.bin.trim().to_lowercase();
        let exe = bin_name(&bin).ok_or_else(|| {
            format!(
                "不支持的 bin: {}（可用 docx / xlsx / pptx / pdf / office）",
                args.bin
            )
        })?;
        // 展示名（去掉 windows 的 .exe 后缀；统一入口为 astorm-office）
        let cli_name = exe.trim_end_matches(".exe").to_string();
        if args.args.is_empty() {
            return Err("args 不能为空：请传 CLI 子命令与参数（如 [\"view\", \"a.docx\", \"/part[1]\", \"text\"]）".to_string());
        }
        let pos = command_pos(&args.args);
        let first = args
            .args
            .get(pos)
            .map(|s| s.trim().to_string())
            .unwrap_or_default();
        if DENIED_COMMANDS.contains(&first.as_str()) {
            return Err(format!(
                "office 工具不支持常驻服务类命令（{first}）：请改用单命令（view / edit / render …）逐条调用"
            ));
        }

        // 可执行文件解析：应用配置 bin_dir → 环境变量 ASTORM_OFFICE_BIN_DIR
        let app = App::try_global();
        let cfg = app.map(|a| a.config().office.clone()).unwrap_or_default();
        let bin_dir = cfg
            .bin_dir
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(Path::new)
            .map(|p| p.to_owned())
            .or_else(|| {
                std::env::var_os("ASTORM_OFFICE_BIN_DIR").map(|v| Path::new(&v).to_owned())
            });
        let cli_path = bin_dir
            .as_ref()
            .map(|d| d.join(&exe))
            .ok_or_else(|| {
                "未配置 office CLI 目录（office.binDir）：astorm-office 套件未随应用分发，\
请运行 scripts/sync-office.mjs 同步后重启"
                    .to_string()
            })?
            .to_path_buf();
        if !cli_path.is_file() {
            return Err(format!(
                "未找到 office CLI: {}（office.binDir 配置指向的目录不完整）\n请运行 pnpm run sync:office（需 astorm-office 仓库）后重启",
                cli_path.display()
            ));
        }

        let read_only = is_read_only(&bin, &args.args);

        // 写类：Plan 硬拒 → 已拒短路 → 审批 / 自动放行（与 browser 工具同一套 policy）
        let mut approved = false;
        if !read_only {
            if policy::plan_read_only(&ctx.session_id) {
                let targets = targets_of(&args.args);
                return Err(super::denial::denial_error(
                    &Denial::new(
                        DenialCategory::FileWrite,
                        DenialSource::StaticPrecheck,
                        format!(
                            "当前模式为 Plan，office 工具的写命令（{first}）被拒绝。只读命令：{}；\
                         需要修改文档请切换到 Execute 或 Auto 模式后重试。",
                            READ_COMMANDS.join(" / ")
                        ),
                    )
                    .with_targets(targets),
                ));
            }
            let key = ledger_key(&bin, &args.args);
            if policy::is_denied(&ctx.session_id, &key) {
                return Err(format!(
                    "该 office 操作刚被用户拒绝：{key}\n请换思路（例如先 view 确认文档结构），不要重复同一操作。"
                ));
            }
            approved = policy::consume_allowed_rerun(&ctx.session_id, &key);
            if !approved && !policy::auto_guard(&ctx.session_id) {
                let targets = targets_of(&args.args);
                let target_text = if targets.is_empty() {
                    "见命令".to_string()
                } else {
                    targets.join("、")
                };
                let purpose = purpose_of(&args)
                    .map(|p| format!("，目的：{p}"))
                    .unwrap_or_default();
                return Ok(policy::needs_approval(ApprovalReq {
                    denial: Denial::new(
                        DenialCategory::FileWrite,
                        DenialSource::StaticPrecheck,
                        format!(
                            "AI 要用 astorm-office CLI 执行写操作：{cli_name} {}（目标：{}）{}",
                            args.args.join(" "),
                            target_text,
                            purpose
                        ),
                    )
                    .with_targets(targets)
                    .with_command(key.clone()),
                    command: &key,
                    session_id: &ctx.session_id,
                    tool_name: "office",
                    tool_call_id: &ctx.tool_call_id,
                }));
            }
        }

        let out = run_cli(&cli_path, &args.args, &cfg, ctx).await?;

        if !read_only {
            let purpose = purpose_of(&args)
                .map(|p| format!("（目的：{p}）"))
                .unwrap_or_default();
            policy::audit(
                &ctx.session_id,
                AuditKind::FileWrite,
                &format!(
                    "office CLI 写操作：{} {}{}",
                    cli_name,
                    args.args.join(" "),
                    purpose
                ),
                if approved {
                    AuditApproval::Agree
                } else {
                    AuditApproval::Auto
                },
                Some(&ledger_key(&bin, &args.args)),
                Some(DenialCategory::FileWrite),
                Some(DenialSource::StaticPrecheck),
            );
        }

        let stdout = out.0;
        let stderr = out.1;
        if !out.2 {
            // 退出码 3 = 产物校验问题：问题清单是**数据**（在 stdout），照常返回但明确提示
            if out.3 == Some(3) {
                let mut text =
                    String::from("【产物校验存在问题（退出码 3），以下为问题清单与状态】\n");
                if !stdout.trim().is_empty() {
                    text.push_str(stdout.trim());
                    text.push('\n');
                }
                if !stderr.trim().is_empty() {
                    text.push_str(stderr.trim());
                }
                return Ok(truncate(&text, MAX_OUTPUT));
            }
            let detail = if stderr.trim().is_empty() {
                stdout.trim()
            } else {
                stderr.trim()
            };
            let detail = if detail.is_empty() {
                format!("office CLI 失败（退出码 {:?}，无输出）", out.3)
            } else {
                detail.to_string()
            };
            return Err(truncate(&detail, 4000));
        }

        let mut text = String::new();
        if !stdout.trim().is_empty() {
            text.push_str(stdout.trim());
        }
        if !stderr.trim().is_empty() {
            if !text.is_empty() {
                text.push_str("\n\n");
            }
            text.push_str("【CLI 状态】\n");
            text.push_str(stderr.trim());
        }
        if text.is_empty() {
            text = "（执行成功，无输出）".to_string();
        }
        Ok(truncate(&text, MAX_OUTPUT))
    }
}

/// 一次 CLI 调用的输出（stdout / stderr / 是否成功 / 退出码）。
type CliOut = (String, String, bool, Option<i32>);

/// 执行 CLI（在会话工作区；注入外部引擎环境变量；超时 / 取消杀子进程）。
async fn run_cli(
    cli_path: &Path,
    args: &[String],
    cfg: &crate::app::config::OfficeConfig,
    ctx: &ToolContext,
) -> Result<CliOut, String> {
    let timeout = Duration::from_secs(cfg.timeout_sec.clamp(30, 600));
    let workspace = resolve_workspace(&ctx.session_id);
    let mut cmd = tokio::process::Command::new(cli_path);
    cmd.args(args).current_dir(&workspace);
    if let Some(c) = cfg
        .chrome_bin
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        cmd.env("ASTORM_CHROME_BIN", c);
    }
    if let Some(s) = cfg
        .soffice_path
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        cmd.env("ASTORM_SOFFICE_PATH", s);
    }

    // 并发读 stdout/stderr；超时/取消按进程树回收（office CLI 会拉起引擎子进程）。
    let out = crate::process::output_with_tree_kill(cmd, timeout, Some(&ctx.cancel))
        .await
        .map_err(|e| match e {
            crate::process::TreeRunError::Spawn(e) => format!("启动 office CLI 失败: {e}"),
            crate::process::TreeRunError::Io(e) => format!("读取 office CLI 输出失败: {e}"),
            crate::process::TreeRunError::Timeout => format!(
                "office CLI 超时（{}s）已终止：大文档 render / convert 较慢，可拆小步或放宽 office.timeoutSec 配置",
                timeout.as_secs()
            ),
            crate::process::TreeRunError::Cancelled => "已取消（用户中断）。".to_string(),
        })?;

    Ok((
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.success(),
        out.status.code(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn read_only_commands() {
        // 四格式公共只读命令
        assert!(is_read_only(
            "docx",
            &args(&["view", "a.docx", "/part[1]", "text"])
        ));
        assert!(is_read_only("docx", &args(&["validate", "a.docx"])));
        // edit 的 get 在路径之后；set 是写
        assert!(is_read_only(
            "docx",
            &args(&["edit", "a.docx", "/part[1]", "get"])
        ));
        assert!(!is_read_only(
            "docx",
            &args(&["edit", "a.docx", "/part[1]", "set"])
        ));
        // 资源导出会写盘
        assert!(!is_read_only(
            "docx",
            &args(&["view", "a.docx", "/", "text", "--media", "out/"])
        ));
        assert!(!is_read_only(
            "docx",
            &args(&["view", "a.docx", "/", "text", "--attachments", "out/"])
        ));
        // batch / pages / sanitize 不算只读
        assert!(!is_read_only("docx", &args(&["batch", "steps.txt"])));
        assert!(!is_read_only("pdf", &args(&["pages", "split", "a.pdf"])));
        assert!(!is_read_only("pdf", &args(&["content", "sanitize"])));
        assert!(!is_read_only("pdf", &args(&["code", "sanitize"])));
        // 帮助 / 版本
        assert!(is_read_only("docx", &args(&["convert", "--help"])));
    }

    #[test]
    fn pdf_analysis_commands() {
        assert!(is_read_only("pdf", &args(&["extract", "text", "a.pdf"])));
        assert!(is_read_only(
            "pdf",
            &args(&["extract", "attachment", "a.pdf"])
        ));
        // image 缺省写当前目录
        assert!(!is_read_only("pdf", &args(&["extract", "image", "a.pdf"])));
        assert!(is_read_only("pdf", &args(&["meta", "get", "a.pdf"])));
        assert!(!is_read_only("pdf", &args(&["meta", "set", "a.pdf"])));
        assert!(is_read_only("pdf", &args(&["form", "info", "a.pdf"])));
        assert!(!is_read_only("pdf", &args(&["form", "fill", "a.pdf"])));
        assert!(is_read_only("pdf", &args(&["qa", "a.pdf"])));
        assert!(is_read_only("pdf", &args(&["check", "font", "a.pdf"])));
        assert!(is_read_only("pdf", &args(&["palette", "generate"])));
        assert!(is_read_only("pdf", &args(&["design", "derive", "标题"])));
        assert!(is_read_only("pdf", &args(&["env", "check"])));
    }

    #[test]
    fn unified_entry_dispatch() {
        // 跳过格式别名后判定真实子命令
        assert!(is_read_only(
            "office",
            &args(&["word", "view", "a.docx", "/", "text"])
        ));
        assert!(is_read_only(
            "office",
            &args(&["pdf", "extract", "text", "a.pdf"])
        ));
        // Office 的 extract 是模板提取（写文件）
        assert!(!is_read_only(
            "office",
            &args(&["word", "extract", "a.docx"])
        ));
        // -f / --format 形式
        assert!(is_read_only(
            "office",
            &args(&["-f", "pdf", "extract", "text", "a.pdf"])
        ));
        // 旗标可与格式别名任意先后
        assert!(is_read_only(
            "office",
            &args(&["word", "--json", "view", "a.docx", "/", "text"])
        ));
        assert!(is_read_only(
            "office",
            &args(&["--json", "word", "view", "a.docx", "/", "text"])
        ));
        assert!(is_read_only("office", &args(&["formats"])));
        assert!(!is_read_only("office", &args(&["batch", "steps.txt"])));
        // serve / mcp 定位到真实子命令
        let pos = command_pos(&args(&["word", "serve"]));
        assert_eq!(
            args(&["word", "serve"]).get(pos).map(String::as_str),
            Some("serve")
        );
    }

    #[test]
    fn approval_targets() {
        // 统一入口的格式别名不算文件目标
        assert_eq!(
            targets_of(&args(&["word", "edit", "a.docx", "/part[1]", "set"])),
            vec!["a.docx"]
        );
        // 输入 + 导出目录
        assert_eq!(
            targets_of(&args(&["view", "a.docx", "/", "text", "--media", "out/"])),
            vec!["a.docx", "out/"]
        );
        // 嵌套子命令（text）不算文件目标
        assert_eq!(
            targets_of(&args(&["extract", "text", "a.pdf"])),
            vec!["a.pdf"]
        );
    }
}
