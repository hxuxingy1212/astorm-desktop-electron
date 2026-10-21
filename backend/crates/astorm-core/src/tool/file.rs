//! `file_operation` 工具：文件读 / 写 / 编辑 / 追加 / 列目录 / 正则搜索 / glob 搜索。
//!
//! - 路径相对会话工作区解析；支持 `$SESSION_WORKSPACE` / `$SESSION_ID` 变量
//! - 会话模式策略：Plan = 空间内读 / 搜索可，写(任意，含空间内)硬拒，空间外读审批；
//!   Execute = 空间内读写可，空间外读写审批；Auto/自动审批 = 可审批项免卡全放行。
//!   **Plan 只读是硬规则，与权限模式正交**：`permission = auto` 只免审批卡，不放宽写禁令。
//!   需审批时返回**中断信封**（`approval` 段）纯 JSON → 引擎中断循环、前端弹审批卡，
//!   用户同意后由会话层自动重放同一操作（无防护）。
//! - 搜索类命中数设上限，避免输出过大

use std::fs;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use globset::Glob;
use regex::Regex;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::tool::denial::{Denial, DenialCategory, DenialSource};
use crate::tool::{expand_session_vars, resolve_workspace};

/// 读 / 搜索返回文本的字符上限。
const READ_MAX_CHARS: usize = 8000;
/// 正则搜索命中条数上限。
const GREP_MAX_HITS: usize = 50;
/// glob 搜索命中条数上限。
const GLOB_MAX_HITS: usize = 100;
/// 文件 diff 预览的上下文行数（每个改动块两侧保留的未修改行）。
const DIFF_CONTEXT_LINES: usize = 3;
/// diff patch 行数上限；超过则只回增删计数（`truncated=true`），避免历史体积失控。
const DIFF_MAX_LINES: usize = 2000;
/// diff patch 字节上限。
const DIFF_MAX_BYTES: usize = 256 * 1024;
/// `operation` 合法取值 —— schema enum / 策略判定 / 报错文案的**唯一来源**。
const OPERATIONS: &[&str] = &["read", "write", "append", "edit", "list", "grep", "glob"];
/// 写操作（Plan 模式硬拒；空间外需 `file_write` 审批）。
const WRITE_OPS: &[&str] = &["write", "append", "edit"];
/// 只读操作（Plan 模式放行；空间外读走 `file_read` 审批）。
const READ_ONLY_OPS: &[&str] = &["read", "list", "grep", "glob"];

/// `file_operation` 工具。
pub struct FileOperationTool;

/// 工具参数（LLM 生成的 JSON，缺失字段用默认值）。
#[derive(Debug, Deserialize)]
struct FileParams {
    /// read / write / append / edit / list / grep / glob
    operation: String,
    path: String,
    #[serde(default)]
    content: Option<String>,
    /// 读文件起止行号（1 起，可选）
    #[serde(default)]
    start_line: Option<usize>,
    #[serde(default)]
    end_line: Option<usize>,
    /// edit：被替换原文（必须存在）
    #[serde(default)]
    old_text: Option<String>,
    /// edit：替换文本
    #[serde(default)]
    new_text: Option<String>,
    /// grep：正则表达式
    #[serde(default)]
    pattern: Option<String>,
    /// glob：匹配模式
    #[serde(default)]
    glob_pattern: Option<String>,
}

#[async_trait]
impl crate::tool::Tool for FileOperationTool {
    fn name(&self) -> &str {
        "file_operation"
    }

    fn description(&self) -> &str {
        "工作区文件操作：read（读取，可指定行号范围）/ write（写入）/ append（追加）/ edit（old_text 替换为 new_text）/ list（列目录）/ grep（正则搜索）/ glob（模式搜索）。写操作仅限工作区内，Plan 模式禁止写入。涉及会话空间外路径（或 Plan 模式空间外读）时本轮循环会中断并由用户在前端审批：用户同意后系统自动重放同一操作，无需再次调用本工具。"
    }

    /// 审批拦截结果（纯 JSON）→ 中断载荷：引擎据此发带 `approval` 的 interrupt 事件。
    fn interrupt_payload(&self, result: &str) -> Option<serde_json::Value> {
        crate::tool::policy::interrupt_request_from_result(result)
    }

    /// 硬拒绝（Plan 模式禁写 / 技能目录只读等）：错误原文即拒绝信封 → 分类元数据。
    fn result_meta(&self, ok: bool, result: &str) -> Option<serde_json::Value> {
        if ok {
            return None;
        }
        crate::tool::denial::denial_meta_from_error(result)
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "operation": {
                    "type": "string",
                    "description": "要执行的操作：read 读取文件（可配 start_line / end_line）/ write 覆盖写入（配 content）/ append 追加（配 content）/ edit 替换（配 old_text + new_text）/ list 列目录 / grep 正则搜索（配 pattern）/ glob 模式搜索（配 glob_pattern）。本参数填上述操作名，不是工具名。",
                    "enum": OPERATIONS
                },
                "path": { "type": "string", "description": "文件/目录路径（相对工作区，或 $SESSION_WORKSPACE 开头）" },
                "content": { "type": "string" },
                "start_line": { "type": "integer", "minimum": 1 },
                "end_line": { "type": "integer", "minimum": 1 },
                "old_text": { "type": "string", "description": "edit 操作中被替换的原文" },
                "new_text": { "type": "string", "description": "edit 操作中的替换文本" },
                "pattern": { "type": "string", "description": "grep 操作的正则表达式" },
                "glob_pattern": { "type": "string", "description": "glob 操作的匹配模式，如 **/*.rs" }
            },
            "required": ["operation", "path"]
        })
    }

    async fn execute(
        &self,
        params: serde_json::Value,
        ctx: &react_loop::ToolContext,
    ) -> Result<String, String> {
        self.run(params, ctx).await.map(|(text, _)| text)
    }

    /// 执行并回传文件级结构化元数据（read_stats / file_diff），供前端过程行展示。
    async fn execute_with_meta(
        &self,
        params: serde_json::Value,
        ctx: &react_loop::ToolContext,
    ) -> Result<(String, Option<Value>), String> {
        self.run(params, ctx).await
    }
}

impl FileOperationTool {
    async fn run(
        &self,
        params: serde_json::Value,
        ctx: &react_loop::ToolContext,
    ) -> Result<(String, Option<Value>), String> {
        let mut p: FileParams = serde_json::from_value(params)
            .map_err(|e| format!("file_operation 参数解析失败: {e}"))?;

        // `operation` 归一化 + 校验**先于一切策略判定**：非法值（常见于模型把工具名
        // 当操作值，如 `"operation": "file_operation"`）直接报可纠正错误、不执行任何
        // 文件动作——既避免被 Plan 写禁令分支误标成「拒绝写」，也让 approval_key /
        // 策略判定 / 分发共用同一个归一化后的值（fail-closed：不猜意图）。
        p.operation = normalize_operation(&p.operation)?;

        let workspace = resolve_workspace(&ctx.session_id);
        let raw = expand_session_vars(&p.path, &ctx.session_id, &workspace);
        let mut path = resolve_path(&workspace, &raw);

        let plan_only = crate::tool::policy::plan_read_only(&ctx.session_id);
        let is_write = WRITE_OPS.contains(&p.operation.as_str());
        let read_only = READ_ONLY_OPS.contains(&p.operation.as_str());
        let auto = crate::tool::policy::auto_guard(&ctx.session_id);

        // 只读操作：模型把已知目录名又拼了一次（`…/X/X`）时回退到 `…/X`。
        // 在策略判定之前替换 —— 审批 key / 越界判定都用**最终**路径，重放时同样成立。
        let path_note = if read_only {
            dedup_duplicated_tail(&path).map(|fixed| {
                let note = format!("（请求路径多了一层同名目录，已按 {} 解析）", fixed.display());
                path = fixed;
                note
            })
        } else {
            None
        };

        // 审批台账 key（与 ask_file 注册的 command 格式一致；Plan / Execute 共用）
        let approval_key = format!("file_operation {} {}", p.operation, path.display());

        // Plan（只读）的**写禁令是硬规则**：先于台账与 auto 判定——`permission = auto`
        // 只免审批卡，不解除写禁令；也避免 Execute 期遗留的一次性额度在 Plan 下被消费而放行写。
        if plan_only && !read_only {
            return Err(crate::tool::denial::denial_error(
                &Denial::new(
                    DenialCategory::FileWrite,
                    DenialSource::StaticPrecheck,
                    "Plan 模式禁止修改文件；Plan 只读与权限模式无关（自动审批也不会放宽）。如需写入请切换到 Execute 或 Auto 模式；如需读取会话空间外文件，可在审批卡中同意后由系统自动重放。",
                )
                .with_targets(vec![path.display().to_string()]),
            ));
        }

        // 审批台账：已同意放行（一次性额度命中即消费）→ 直接执行；已拒绝 → 短路，
        // 避免同操作反复生成新审批。Plan 空间外读的「一次性同意」重放同样走这里。
        if crate::tool::policy::consume_allowed_rerun(&ctx.session_id, &approval_key) {
            return attach_path_note(dispatch_file(p, &path), path_note.as_deref());
        }
        if crate::tool::policy::is_denied(&ctx.session_id, &approval_key) {
            return Err(format!(
                "该文件操作已被用户拒绝执行：{approval_key}\n请改换思路，不要重复执行同一被拒操作。"
            ));
        }

        // Plan 读（写已在上方硬拒）：空间内/白名单放行；空间外读自动审批免卡，否则弹卡。
        // 安全配置额外白名单（pathAllowedBySecurity）与会话内存白名单（审批卡「当前会话
        // 允许」）命中的空间外路径视为已授权（仅读；写禁令不受会话授权放宽）。
        if plan_only {
            if in_workspace(&workspace, &path)
                || crate::tool::policy::path_allowed_by_security(&path, false)
                || crate::tool::policy::path_allowed_by_session(&ctx.session_id, &path, false)
            {
                return attach_path_note(dispatch_file(p, &path), path_note.as_deref());
            }
            // 空间外读：自动审批下免卡放行（读不属写禁令），否则弹审批卡
            if auto {
                return attach_path_note(dispatch_file(p, &path), path_note.as_deref());
            }
            return ask_file("file_read", "read", &workspace, &path, ctx, p.start_line, p.end_line)
                .map(|text| (text, None));
        }

        // Auto 会话 / 自动审批：直接放行（无审批）。
        if auto {
            return attach_path_note(dispatch_file(p, &path), path_note.as_deref());
        }

        // Execute：空间内读写放行（含安全配置额外白名单 / 会话白名单）；空间外读写 → 审批门。
        let inside = in_workspace(&workspace, &path)
            || crate::tool::policy::path_allowed_by_security(&path, is_write)
            || crate::tool::policy::path_allowed_by_session(&ctx.session_id, &path, is_write);
        if inside {
            return attach_path_note(dispatch_file(p, &path), path_note.as_deref());
        }
        // 空间外：写 → 审批（file_write）；读 → 审批（file_read）
        let kind = if is_write { "file_write" } else { "file_read" };
        ask_file(kind, &p.operation, &workspace, &path, ctx, p.start_line, p.end_line)
            .map(|text| (text, None))
    }
}

/// 归一化 `operation`（去首尾空白 + 转小写）并校验合法性。
///
/// 合法值由 [`OPERATIONS`] 单一维护；非法值（如模型把**工具名**当成操作值：
/// `"operation": "file_operation"`）返回**可纠正**错误——直接列出可选项，供模型下一轮
/// 自纠重试。不按其他字段猜测意图，避免猜错语义（如把 `write` 猜成 `read`）。
fn normalize_operation(raw: &str) -> Result<String, String> {
    let op = raw.trim().to_ascii_lowercase();
    if OPERATIONS.contains(&op.as_str()) {
        return Ok(op);
    }
    Err(format!(
        "未知文件操作: {raw}。operation 必须是 {} 之一；读取文件请用 operation=read（本参数填操作名，不是工具名）。",
        OPERATIONS.join(" | ")
    ))
}

/// 空间内判定（复用 policy 归一化）。
fn in_workspace(workspace: &std::path::Path, path: &std::path::Path) -> bool {
    crate::tool::policy::path_in_workspace(workspace, path)
}

/// 直接分发到具体文件操作（未越界 / 已放行路径）。
///
/// 返回（结果文本, 结构化元数据）：read → `read_stats`，write/edit → `file_diff`，
/// append → 仅增行计数，其余为 `None`。
fn dispatch_file(p: FileParams, path: &std::path::Path) -> Result<(String, Option<Value>), String> {
    match p.operation.as_str() {
        "read" => {
            let (text, stats) = read_file(path, p.start_line, p.end_line)?;
            Ok((text, Some(json!({ "read_stats": stats }))))
        }
        "write" => {
            let content = p.content.ok_or("write 操作缺少 content")?;
            let (text, meta) = write_file(path, &content)?;
            Ok((text, Some(meta)))
        }
        "append" => {
            let content = p.content.ok_or("append 操作缺少 content")?;
            let (text, meta) = append_file(path, &content)?;
            Ok((text, Some(meta)))
        }
        "edit" => {
            let old = p.old_text.ok_or("edit 操作缺少 old_text")?;
            let new = p.new_text.unwrap_or_default();
            let (text, meta) = edit_file(path, &old, &new)?;
            Ok((text, Some(meta)))
        }
        "list" => Ok((list_dir(path)?, None)),
        "grep" => {
            let pattern = p.pattern.ok_or("grep 操作缺少 pattern")?;
            Ok((grep_search(path, &pattern)?, None))
        }
        "glob" => {
            let pattern = p.glob_pattern.ok_or("glob 操作缺少 glob_pattern")?;
            Ok((glob_search(path, &pattern)?, None))
        }
        other => Err(format!("未知文件操作: {other}")),
    }
}

/// 文件 diff 元数据：`{path, additions, deletions, newLines, patch?, truncated}`。
///
/// patch 超过行数 / 字节上限时只回计数并标 `truncated=true`（前端只展示 +N -N）。
fn file_diff_meta(path: &Path, before: &str, after: &str) -> Value {
    let display = path.display().to_string();
    let label = crate::tool::diff::file_leaf(&display);
    let (additions, deletions) = crate::tool::diff::compute_line_change_stat(before, after);
    let patch = crate::tool::diff::build_unified_diff(before, after, label, DIFF_CONTEXT_LINES);
    let truncated = patch.lines().count() > DIFF_MAX_LINES || patch.len() > DIFF_MAX_BYTES;
    let mut meta = json!({
        "path": display,
        "additions": additions,
        "deletions": deletions,
        "newLines": logical_line_count(after),
        "truncated": truncated,
    });
    if !truncated {
        meta["patch"] = json!(patch);
    }
    meta
}

/// 逻辑行数：末尾单个换行不算多一行（与 `diff::split_lines` 同口径）。
fn logical_line_count(text: &str) -> usize {
    if text.is_empty() {
        return 0;
    }
    let mut count = text.split('\n').count();
    if text.ends_with('\n') {
        count -= 1;
    }
    count
}

/// 需审批项：登记待决议并返回**中断信封**纯 JSON（引擎中断，用户同意后自动重放）。
#[allow(clippy::too_many_arguments)]
fn ask_file(
    kind: &str,
    operation: &str,
    workspace: &std::path::Path,
    path: &std::path::Path,
    ctx: &react_loop::ToolContext,
    _start_line: Option<usize>,
    _end_line: Option<usize>,
) -> Result<String, String> {
    let rel = path.strip_prefix(workspace).unwrap_or(path);
    let target = rel.display().to_string();
    let category = if kind == "file_read" {
        DenialCategory::FileRead
    } else {
        DenialCategory::FileWrite
    };
    let denial = Denial::new(
        category,
        DenialSource::StaticPrecheck,
        format!("操作「{operation}」涉及会话空间外路径：{target}"),
    )
    .with_targets(vec![target])
    .with_command(format!("file_operation {operation} {}", path.display()));
    let command = format!("file_operation {operation} {}", path.display());
    // 审计：拦截（审批待定）
    crate::tool::policy::audit(
        &ctx.session_id,
        crate::tool::policy::audit_kind_of(category),
        &denial.detail,
        crate::tool::audit::AuditApproval::None,
        Some(command.as_str()),
        Some(category),
        Some(denial.source),
    );
    Ok(crate::tool::policy::needs_approval(
        crate::tool::policy::ApprovalReq {
            denial,
            command: &command,
            session_id: &ctx.session_id,
            tool_name: "file_operation",
            tool_call_id: &ctx.tool_call_id,
        },
    ))
}

/// 相对工作区解析路径（绝对路径原样使用）。
fn resolve_path(workspace: &Path, raw: &str) -> PathBuf {
    let p = PathBuf::from(raw);
    if p.is_absolute() {
        p
    } else {
        workspace.join(p)
    }
}

/// 只读操作的「重复末段」自愈：`…/X/X` 不存在而 `…/X` 存在时，返回 `…/X`。
///
/// 模型偶尔把手边已知的目录名再拼一次（实测：先列 `…/Desktop/skills/astorm-ppt-generator`
/// 成功，同一条消息里又列 `…/astorm-ppt-generator/astorm-ppt-generator`），得到纯 ENOENT，
/// 白费一轮。**只在目标不存在时**回退，且回退目标必须存在 —— 正常路径一律原样使用；
/// 写类操作绝不走这里（自动改路径会写到非预期位置）。
fn dedup_duplicated_tail(path: &Path) -> Option<PathBuf> {
    if path.exists() {
        return None;
    }
    let name = path.file_name()?;
    let parent = path.parent()?;
    if parent.file_name()? != name {
        return None;
    }
    parent.exists().then(|| parent.to_path_buf())
}

/// 把「路径已纠正」的说明附在成功结果前（失败 / 审批信封不加）。
fn attach_path_note(
    res: Result<(String, Option<Value>), String>,
    note: Option<&str>,
) -> Result<(String, Option<Value>), String> {
    match (res, note) {
        (Ok((text, meta)), Some(note)) => Ok((format!("{note}\n{text}"), meta)),
        (r, _) => r,
    }
}

/// 文本文件编码（编辑后按原编码写回，避免 UTF-16 文件被改成 UTF-8）。
#[derive(Clone, Copy)]
enum TextEncoding {
    Utf8,
    Utf8Bom,
    Utf16Le,
    Utf16Be,
}

/// 读取文本文件：识别 UTF-8 BOM / UTF-16LE / UTF-16BE。
///
/// Windows 上 PowerShell 5.1 的 `>` / `Out-File` 默认产出 UTF-16LE，
/// 直接 `read_to_string` 会报 “stream did not contain valid UTF-8”。
fn read_text(path: &Path) -> Result<(String, TextEncoding), String> {
    let bytes = fs::read(path).map_err(|e| format!("读取失败 {}: {e}", path.display()))?;
    decode_text(&bytes)
}

fn decode_text(bytes: &[u8]) -> Result<(String, TextEncoding), String> {
    if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        let text = String::from_utf8(rest.to_vec())
            .map_err(|e| format!("文件不是有效 UTF-8: {e}"))?;
        return Ok((text, TextEncoding::Utf8Bom));
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        let units: Vec<u16> = rest
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        return Ok((String::from_utf16_lossy(&units), TextEncoding::Utf16Le));
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        let units: Vec<u16> = rest
            .chunks_exact(2)
            .map(|c| u16::from_be_bytes([c[0], c[1]]))
            .collect();
        return Ok((String::from_utf16_lossy(&units), TextEncoding::Utf16Be));
    }
    let text = String::from_utf8(bytes.to_vec())
        .map_err(|e| format!("文件不是有效 UTF-8: {e}"))?;
    Ok((text, TextEncoding::Utf8))
}

fn encode_text(text: &str, enc: TextEncoding) -> Vec<u8> {
    match enc {
        TextEncoding::Utf8 => text.as_bytes().to_vec(),
        TextEncoding::Utf8Bom => {
            let mut out = vec![0xEF, 0xBB, 0xBF];
            out.extend_from_slice(text.as_bytes());
            out
        }
        TextEncoding::Utf16Le => {
            let mut out = vec![0xFF, 0xFE];
            for unit in text.encode_utf16() {
                out.extend_from_slice(&unit.to_le_bytes());
            }
            out
        }
        TextEncoding::Utf16Be => {
            let mut out = vec![0xFE, 0xFF];
            for unit in text.encode_utf16() {
                out.extend_from_slice(&unit.to_be_bytes());
            }
            out
        }
    }
}

/// 读取文件（可限定行号范围 1 起），超长截断；回传（文本, read_stats）。
fn read_file(
    path: &Path,
    start_line: Option<usize>,
    end_line: Option<usize>,
) -> Result<(String, Value), String> {
    let (content, _enc) = read_text(path)?;
    let lines: Vec<&str> = content.lines().collect();
    let start = start_line.unwrap_or(1).saturating_sub(1);
    let end = end_line.map(|e| e.min(lines.len())).unwrap_or(lines.len());
    let mut shown: String = if start >= end {
        String::new()
    } else {
        lines[start..end].join("\n")
    };
    let mut truncated = false;
    if shown.chars().count() > READ_MAX_CHARS {
        shown = shown.chars().take(READ_MAX_CHARS).collect();
        shown.push_str("\n…（内容过长已截断）");
        truncated = true;
    }
    // 统计**内容本身**（不含截断提示），供过程行显示「N行M字符」
    let visible = shown
        .strip_suffix("\n…（内容过长已截断）")
        .unwrap_or(&shown);
    let stats = json!({
        "lines": logical_line_count(visible),
        "chars": visible.chars().count(),
        "truncated": truncated,
    });
    Ok((shown, stats))
}

/// 写入文件（自动创建父目录）；回传（文本, file_diff）。
fn write_file(path: &Path, content: &str) -> Result<(String, Value), String> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("创建目录失败 {}: {e}", dir.display()))?;
    }
    fs::write(path, content).map_err(|e| format!("写入失败 {}: {e}", path.display()))?;
    let meta = file_diff_meta(path, "", content);
    Ok((
        format!("已写入 {} 字符到 {}", content.chars().count(), path.display()),
        meta,
    ))
}

/// 追加内容到文件（不存在则创建）；回传（文本, 仅增行计数）。
fn append_file(path: &Path, content: &str) -> Result<(String, Value), String> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("创建目录失败 {}: {e}", dir.display()))?;
    }
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| format!("打开失败 {}: {e}", path.display()))?;
    use std::io::Write;
    file.write_all(content.as_bytes())
        .map_err(|e| format!("追加失败 {}: {e}", path.display()))?;
    // 追加不预读原文，只回新增行数（与前端约定一致）
    let lines = logical_line_count(content);
    let meta = json!({
        "path": path.display().to_string(),
        "additions": lines,
        "deletions": 0,
        "newLines": lines,
    });
    Ok((
        format!("已追加 {} 字符到 {}", content.chars().count(), path.display()),
        meta,
    ))
}

/// 编辑文件：old_text 全文替换为 new_text；回传（文本, file_diff，含整份文件上下文）。
fn edit_file(path: &Path, old: &str, new: &str) -> Result<(String, Value), String> {
    let (content, enc) = read_text(path)?;
    // 模型读到的内容已按行归一化为 LF；文件可能是 CRLF（Windows / git autocrlf）。
    // 先按原文匹配；不中再按 LF 归一化匹配，写回时恢复原换行，避免误报“未找到”。
    let (base, old_pat, new_pat, crlf) = if content.contains(old) {
        (content.clone(), old.to_string(), new.to_string(), false)
    } else if content.contains("\r\n") {
        let lf = content.replace("\r\n", "\n");
        let old_lf = old.replace("\r\n", "\n");
        let new_lf = new.replace("\r\n", "\n");
        if !lf.contains(&old_lf) {
            return Err(format!("未找到 old_text: {old}"));
        }
        (lf, old_lf, new_lf, true)
    } else {
        return Err(format!("未找到 old_text: {old}"));
    };
    let count = base.matches(&old_pat).count();
    let mut replaced = base.replace(&old_pat, &new_pat);
    if crlf {
        replaced = replaced.replace('\n', "\r\n");
    }
    fs::write(path, encode_text(&replaced, enc))
        .map_err(|e| format!("写入失败 {}: {e}", path.display()))?;
    let meta = file_diff_meta(path, &content, &replaced);
    Ok((format!("已替换 {count} 处: {}", path.display()), meta))
}

/// 列目录：名称 + 目录标记，按名称排序。空目录输出占位文本
/// （空字符串结果会让「冷启动重建 → 请求过滤器」丢掉这条 tool 消息，上游 400）。
fn list_dir(path: &Path) -> Result<String, String> {
    let mut entries: Vec<String> = fs::read_dir(path)
        .map_err(|e| format!("列目录失败 {}: {e}", path.display()))?
        .filter_map(|e| e.ok())
        .map(|e| {
            let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
            let name = e.file_name().to_string_lossy().into_owned();
            if is_dir {
                format!("{name}/")
            } else {
                name
            }
        })
        .collect();
    entries.sort();
    if entries.is_empty() {
        return Ok("（目录为空）".to_string());
    }
    Ok(entries.join("\n"))
}

/// 正则搜索：递归遍历目录内文本文件，输出 `文件:行号:内容`，命中数上限 [`GREP_MAX_HITS`]。
fn grep_search(path: &Path, pattern: &str) -> Result<String, String> {
    let re = Regex::new(pattern).map_err(|e| format!("正则表达式无效: {e}"))?;
    let mut hits = Vec::new();
    'outer: for entry in ignore::WalkBuilder::new(path).hidden(true).build().flatten() {
        if !entry.file_type().map_or(false, |t| t.is_file()) {
            continue;
        }
        let file_path = entry.path();
        // 跳过本会话的消息归档/日志文件
        if file_path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
            continue;
        }
        let content = match fs::read_to_string(file_path) {
            Ok(c) => c,
            Err(_) => continue,
        };
        for (i, line) in content.lines().enumerate() {
            if re.is_match(line) {
                hits.push(format!("{}:{}:{}", file_path.display(), i + 1, elide(line, 200)));
                if hits.len() >= GREP_MAX_HITS {
                    break 'outer;
                }
            }
        }
    }
    if hits.is_empty() {
        return Ok("未找到匹配".into());
    }
    Ok(hits.join("\n"))
}

/// glob 搜索：按模式匹配工作区内文件相对路径，命中数上限 [`GLOB_MAX_HITS`]。
fn glob_search(path: &Path, pattern: &str) -> Result<String, String> {
    let matcher = Glob::new(pattern)
        .map_err(|e| format!("glob 模式无效: {e}"))?
        .compile_matcher();
    let mut hits = Vec::new();
    for entry in ignore::WalkBuilder::new(path).hidden(true).build().flatten() {
        if !entry.file_type().map_or(false, |t| t.is_file()) {
            continue;
        }
        let p = entry.path();
        if matcher.is_match(p) {
            hits.push(p.display().to_string());
            if hits.len() >= GLOB_MAX_HITS {
                break;
            }
        }
    }
    if hits.is_empty() {
        return Ok("未找到匹配".into());
    }
    Ok(hits.join("\n"))
}

/// 截断为单行片段（grep 命中行展示用）。
fn elide(line: &str, max: usize) -> String {
    if line.chars().count() <= max {
        line.to_string()
    } else {
        line.chars().take(max).collect::<String>() + "…"
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::Tool;

    fn tmp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("astorm-file-dedup-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn 只读路径重复末段回退到父目录() {
        let root = tmp_root("dup");
        fs::create_dir_all(root.join("outer")).unwrap();
        fs::write(root.join("outer").join("a.txt"), "hi").unwrap();

        // 已存在的路径原样使用（含真实的嵌套同名目录，如 vendor/vendor）
        assert_eq!(dedup_duplicated_tail(&root.join("outer")), None);
        fs::create_dir_all(root.join("vendor").join("vendor")).unwrap();
        assert_eq!(dedup_duplicated_tail(&root.join("vendor").join("vendor")), None);

        // 末段重复且目标不存在、父目录存在 → 回退
        assert_eq!(
            dedup_duplicated_tail(&root.join("outer").join("outer")),
            Some(root.join("outer"))
        );
        // 末段不重复 → 不动（不猜别的路径）
        assert_eq!(dedup_duplicated_tail(&root.join("outer").join("nope")), None);
        // 父目录也不存在 → 不动（保持原样的 ENOENT 报错）
        assert_eq!(
            dedup_duplicated_tail(&root.join("missing").join("missing")),
            None
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn 路径说明只附在成功结果上() {
        let note = Some("（请求路径多了一层同名目录，已按 /a/outer 解析）");
        assert_eq!(
            attach_path_note(Ok(("a.txt".into(), None)), note).unwrap().0,
            "（请求路径多了一层同名目录，已按 /a/outer 解析）\na.txt"
        );
        assert_eq!(
            attach_path_note(Err("列目录失败".into()), note).unwrap_err(),
            "列目录失败"
        );
        assert_eq!(attach_path_note(Ok(("x".into(), None)), None).unwrap().0, "x");
    }

    #[test]
    fn 非法操作报可纠正错误() {
        // 真实案例：模型把工具名当成操作值
        let err = normalize_operation("file_operation").unwrap_err();
        assert!(err.contains("file_operation"), "应回指非法值: {err}");
        for op in OPERATIONS {
            assert!(err.contains(op), "报错应列出合法值 {op}: {err}");
        }

        // 空白 / 大小写归一化
        assert_eq!(normalize_operation("  READ  ").unwrap(), "read");
        assert_eq!(normalize_operation("Grep").unwrap(), "grep");
        // 空值与近义动词同样拒绝（不猜意图）
        for junk in ["", " ", "cat", "search", "删除"] {
            assert!(normalize_operation(junk).is_err(), "{junk} 应被拒绝");
        }
    }

    #[test]
    fn schema枚举与操作表一致() {
        let schema = FileOperationTool.parameters_schema();
        let property = &schema["properties"]["operation"];
        let declared: Vec<&str> = property["enum"]
            .as_array()
            .expect("operation 应声明 enum")
            .iter()
            .map(|v| v.as_str().expect("enum 元素应为字符串"))
            .collect();
        assert_eq!(declared, OPERATIONS, "schema enum 必须与 OPERATIONS 一致");
        assert!(
            property["description"].as_str().is_some_and(|d| !d.is_empty()),
            "operation 应带 description（模型据此选值）"
        );
    }

    #[test]
    fn 写与只读操作恰好划分全部操作() {
        // 划分完整 ⇔ Plan 写禁令（`plan_only && !read_only`）对合法值永远判定正确
        for op in OPERATIONS {
            let n = WRITE_OPS.contains(op) as usize + READ_ONLY_OPS.contains(op) as usize;
            assert_eq!(n, 1, "{op} 应恰好属于写或只读之一");
        }
        for op in WRITE_OPS.iter().chain(READ_ONLY_OPS) {
            assert!(OPERATIONS.contains(op), "{op} 应在 OPERATIONS 内");
        }
    }

    #[test]
    fn 编辑回传文件diff元数据() {
        let root = tmp_root("edit-meta");
        fs::create_dir_all(&root).unwrap();
        let f = root.join("Cargo.toml");
        fs::write(&f, "[package]\nname = \"json2xlsx\"\nversion.workspace = true\n").unwrap();

        let (text, meta) = edit_file(&f, "name = \"json2xlsx\"", "name = \"json2xlsx-cli\"").unwrap();
        assert!(text.contains("已替换 1 处"), "{text}");
        assert_eq!(meta["additions"], 1);
        assert_eq!(meta["deletions"], 1);
        assert_eq!(meta["truncated"], false);
        let patch = meta["patch"].as_str().unwrap();
        assert!(patch.contains("+name = \"json2xlsx-cli\""), "{patch}");
        assert!(patch.contains("-name = \"json2xlsx\""), "{patch}");
        // 上下文行（未修改）也应保留
        assert!(patch.contains(" version.workspace = true"), "{patch}");

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn 读取回传行字符统计() {
        let root = tmp_root("read-meta");
        fs::create_dir_all(&root).unwrap();
        let f = root.join("a.txt");
        fs::write(&f, "l1\nl2\nl3\n").unwrap();

        let (_text, stats) = read_file(&f, None, None).unwrap();
        assert_eq!(stats["lines"], 3);
        assert_eq!(stats["chars"], 8, "l1\\nl2\\nl3 = 8 字符");
        assert_eq!(stats["truncated"], false);

        let _ = fs::remove_dir_all(&root);
    }
}
