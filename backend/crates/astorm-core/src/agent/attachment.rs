//! 用户消息附件：**图片复制进会话工作区并内联上送；其他文件只做原文路径引用**。
//!
//! 数据流（前端「+」选择任意文件 / 粘贴截图 / 拖拽 → 主进程 staging → 请求 `attachments`）：
//! 1. [`finalize`] 逐项判定（读文件头 64 字节嗅探魔数）：
//!    - **图片**（JPEG/PNG/GIF/WebP）：校验单张与总量上限 → 复制进
//!      `<会话工作区>/.astorm/attachment/<uuid>_<原名>.<ext>` → 生成内联 base64 图片片段；
//!    - **其他文件**：**不读内容、不复制**，只记原始绝对路径与大小（`mime` 记
//!      `application/octet-stream`），由 agent 按类型自行读取（图片用 `read_images`、
//!      PDF/Office 用 astorm-office 技能、文本/代码用 `file_operation`；工作区外路径的
//!      读取会走审批卡，见 `tool::policy`）。
//! 2. 元数据写进消息 `metadata.attachments`（随消息 jsonl 落库；发送给 LLM 前由引擎剥离）；
//!    非图片的原文路径另以「【附件】…」段追加在**用户消息文本**里，让模型知道去哪读。
//! 3. [`rebuild_parts`]：冷启动重建会话历史时按 `metadata.attachments[].path` 重新读盘挂回
//!    `content_parts`（**仅图片**；外部引用本就没有片段，静默跳过），文件缺失/超限则跳过。
//!
//! 图片只能出现在 **user** 消息（DeepSeek 文档约束），故附件并入用户消息本身而非另发追加消息。
//! 附件为用户显式提供（非模型自主读取），**不走审批门**。
//!
//! 为什么图片由后端复制而非前端：新建会话的 workspace 由后端决定（默认时间戳工作区），
//! 前端在首条消息发送前拿不到最终路径；后端统一复制可让三个入口与排队/立即发送一致。
//! 非图片反过来**刻意不复制**：PDF/Office/文本按原路径读取即可，
//! 复制一份只会平白占用工作区、还可能因体积上限被拒。

use std::collections::HashSet;
use std::path::Path;

use astorm_common::session::req::{MentionKind, MentionRef, QuoteRef};
use llm_provider::{ChatMessage, ContentPart, ImageUrlContent};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::tool::read_images::{data_uri, detect_format, ImageFormat};

/// 单条消息**图片**数量上限（非图片只是路径引用，见 [`MAX_EXTERNAL_FILES`]）。
pub(crate) const MAX_ATTACHMENTS: usize = 8;
/// 单张**图片**字节上限（与 read_images 同口径；非图片不读内容，不受此限）。
pub(crate) const MAX_ATTACHMENT_BYTES: u64 = 20 * 1024 * 1024;
/// 单条消息**图片**原始字节总量上限：base64 膨胀 ≈4/3 → ≈40MiB，为 48MiB 请求体留余量。
pub(crate) const MAX_ATTACHMENT_TOTAL_BYTES: u64 = 30 * 1024 * 1024;
/// 单条消息**非图片**附件数量上限（仅路径引用，不读不复制，故可放宽很多）。
pub(crate) const MAX_EXTERNAL_FILES: usize = 20;

/// 工作区内附件目录（相对会话工作区）。
const ATTACHMENT_DIR: &str = "attachment";
/// 文件名清洗后保留的最大字符数。
const MAX_STEM_CHARS: usize = 40;
/// 判定文件类型只需读文件头这么多字节（4 种图片格式的魔数都在这之内）。
const HEAD_SNIFF_BYTES: usize = 64;
/// 非图片附件的 MIME：只有扩展名可用（不做内容嗅探），统一记通用二进制。
const EXTERNAL_MIME: &str = "application/octet-stream";

/// 单个附件的持久化描述（写进 user 消息 `metadata.attachments`，前端据此渲染缩略图 / 文件胶囊）。
///
/// 图片的 `path` 是**工作区内的副本**；非图片的 `path` 是**用户原始路径**（未复制）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct AttachmentMeta {
    /// 可读取的**绝对路径**（图片 = 工作区内副本，前端经 `astorm-attachment://` 读缩略图）。
    pub path: String,
    /// 原始文件名（展示用）。
    pub name: String,
    /// MIME（图片按 magic bytes 判定；非图片固定 `application/octet-stream`）。
    pub mime: String,
    /// 字节数。
    pub size: u64,
}

/// 元数据是否来自「复制进工作区的图片」——`rebuild_parts` 只重建这类，非图片跳过。
fn is_image_meta(meta: &AttachmentMeta) -> bool {
    meta.mime.starts_with("image/")
}

/// 附件装配结果。
pub(crate) struct FinalizedAttachments {
    /// 图片片段（已内联 base64 data URI）。
    pub parts: Vec<ContentPart>,
    /// 持久化元数据（与 `parts` 一一对应）。
    pub metas: Vec<AttachmentMeta>,
    /// 逐项失败说明（`文件名：原因`）。
    pub failures: Vec<String>,
}

/// 组装用户消息：文本 + 附件（有图片则 `content_parts` 携带图片，text 作为首个文本片段上送；
/// 非图片只把**原文路径**写进文本，见 [`external_note`]）。
///
/// 失败项以「（以下附件未读入：…）」追加到文本尾部，让模型知道有附件缺失而非静默丢弃。
pub(crate) fn build_user_message(
    session_id: &str,
    text: &str,
    attachments: Option<&[String]>,
) -> ChatMessage {
    let text = text.to_string();
    let Some(sources) = attachments.filter(|s| !s.is_empty()) else {
        return ChatMessage::user(text);
    };
    let done = finalize(session_id, sources);

    let mut content = text;
    // 非图片附件：只告知原文路径（未复制进工作区），由模型按类型自行读取
    let externals: Vec<&AttachmentMeta> =
        done.metas.iter().filter(|m| !is_image_meta(m)).collect();
    if !externals.is_empty() {
        if !content.is_empty() {
            content.push_str("\n\n");
        }
        content.push_str(&external_note(&externals));
    }
    if !done.failures.is_empty() {
        if !content.is_empty() {
            content.push_str("\n\n");
        }
        content.push_str(&format!("（以下附件未读入：{}）", done.failures.join("；")));
    }

    let mut message = if done.parts.is_empty() {
        ChatMessage::user(content)
    } else {
        let mut m = ChatMessage::multimodal(done.parts);
        m.content = Some(content);
        m
    };
    if !done.metas.is_empty() {
        message.metadata = Some(json!({ "attachments": done.metas }));
    }
    message
}

/// user 消息 metadata 里保存「引用说明」的键（**不渲染**；喂模型与冷启动重建时拼回正文）。
pub(crate) const REFS_NOTE_KEY: &str = "refsNote";

/// 渲染完整「引用说明」：结构化提及指令块 + 浏览器元素围栏原文 + office 路径围栏原文；无内容返回空串。
///
/// - 提及（`@文件` / `$智能体` / `$技能`）：见 [`render_mentions`]；
/// - 元素围栏：每段 ```element 原文按序拼接（前端已生成，后端原样透传）；
/// - office 围栏：每段 ```office-ref 原文按序拼接（同上）。
pub(crate) fn render_refs_note(
    mentions: &[MentionRef],
    elements: &[String],
    office_refs: &[String],
    quotes: &[QuoteRef],
    workspace: Option<&str>,
) -> String {
    let mut parts: Vec<String> = Vec::new();
    let block = render_mentions(mentions, workspace);
    if !block.is_empty() {
        parts.push(block);
    }
    let fences: Vec<&str> = elements
        .iter()
        .chain(office_refs.iter())
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();
    if !fences.is_empty() {
        parts.push(fences.join("\n\n"));
    }
    let quoted = render_quotes(quotes);
    if !quoted.is_empty() {
        parts.push(quoted);
    }
    parts.join("\n\n")
}

/// 渲染「引用对话」块：正文引用（「添加到当前任务」选中的一段）+ 来源消息。
fn render_quotes(quotes: &[QuoteRef]) -> String {
    let mut lines: Vec<String> = Vec::new();
    for q in quotes {
        let text = q.text.trim();
        if text.is_empty() {
            continue;
        }
        let source = match q.role.as_deref() {
            Some("user") => "用户消息",
            _ => "助手消息",
        };
        let seq = q.seq.map(|s| format!(" #{s}")).unwrap_or_default();
        lines.push(format!("- （{source}{seq}）{text}"));
    }
    if lines.is_empty() {
        return String::new();
    }
    format!("【引用对话】\n{}", lines.join("\n"))
}

/// 把「引用说明」追加到消息正文尾部（空说明原样返回）。
pub(crate) fn append_note(mut message: ChatMessage, note: &str) -> ChatMessage {
    if note.trim().is_empty() {
        return message;
    }
    let mut content = message.content.take().unwrap_or_default();
    if !content.is_empty() {
        content.push_str("\n\n");
    }
    content.push_str(note);
    message.content = Some(content);
    message
}

/// 把「引用说明」写进消息 metadata（空说明时不动）。
pub(crate) fn set_refs_note(metadata: &mut Option<Value>, note: &str) {
    if note.trim().is_empty() {
        return;
    }
    let mut obj = metadata.take().unwrap_or_else(|| json!({}));
    if !obj.is_object() {
        obj = json!({});
    }
    obj[REFS_NOTE_KEY] = Value::String(note.to_string());
    *metadata = Some(obj);
}

/// 从消息 metadata 读取「引用说明」（缺省 / 非法 → 空串）。
pub(crate) fn refs_note_of(metadata: Option<&Value>) -> String {
    metadata
        .and_then(|m| m.get(REFS_NOTE_KEY))
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string()
}

/// 渲染 `【引用指令】` 文本块；无有效条目时返回空串。
fn render_mentions(mentions: &[MentionRef], workspace: Option<&str>) -> String {
    let mut seen: HashSet<&str> = HashSet::new();
    let mut lines: Vec<String> = Vec::new();
    for m in mentions {
        if !seen.insert(m.token.as_str()) {
            continue;
        }
        match m.kind {
            // 命令是控制面板语义，当前不交给模型
            MentionKind::Command => continue,
            MentionKind::Skill => {
                let name = if m.id.trim().is_empty() {
                    m.token.trim_start_matches('$')
                } else {
                    m.id.trim()
                };
                lines.push(format!(
                    "- {}（技能「{}」）：本轮请先调用 skills 工具加载该技能（action=\"load\", skills=[\"{}\"]），再按其指引执行。",
                    m.token, name, name
                ));
            }
            MentionKind::Agent => {
                let id = m.id.trim();
                let name = m.label.as_deref().map(str::trim).filter(|s| !s.is_empty()).unwrap_or(id);
                lines.push(format!(
                    "- {}（子智能体「{}」）：请调用 sub_session 工具（agent_type=\"{}\"）将本任务委派给该子智能体。",
                    m.token, name, id
                ));
            }
            MentionKind::File => {
                let rel = m.id.trim();
                let shown = if rel.is_empty() { m.token.as_str() } else { rel };
                let mut line = format!("- {}（文件引用）：工作区相对路径 {}。", m.token, shown);
                if let Some(abs) = resolve_absolute(workspace, rel) {
                    line.push_str(&format!("绝对路径 {}。", abs));
                }
                line.push_str("如需文件内容请用 file_operation 读取。");
                lines.push(line);
            }
        }
    }
    if lines.is_empty() {
        return String::new();
    }
    format!("【引用指令】\n{}", lines.join("\n"))
}

/// 文件引用绝对路径：已是绝对路径则原样返回；否则拼到会话工作区下。
fn resolve_absolute(workspace: Option<&str>, rel: &str) -> Option<String> {
    if rel.is_empty() {
        return None;
    }
    if Path::new(rel).is_absolute() {
        return Some(rel.to_string());
    }
    let ws = workspace?.trim();
    if ws.is_empty() {
        return None;
    }
    Some(Path::new(ws).join(rel).to_string_lossy().to_string())
}

/// 非图片附件的告知段：给模型**原文路径**（文件未被复制，需自行读取）。
///
/// 这段是附件唯一的「内容入口」——模型不看它就不会知道用户给了文件，故措辞把「怎么读」
/// 一并说清（PDF/Office 用 astorm-office 技能、图片用 `read_images`、文本/代码用
/// `file_operation`；工作区外读取可能需用户批准）。
fn external_note(items: &[&AttachmentMeta]) -> String {
    let mut out = String::from(
        "【附件】用户随本条消息提供了以下文件，**未复制进工作区**，需要时请按原文路径读取\
         （pdf/docx/xlsx/pptx 用 astorm-office 技能，图片用 read_images，文本/代码用 file_operation；\
         工作区外的路径读取可能需要用户批准）：",
    );
    for m in items {
        out.push_str(&format!("\n- {}", m.path));
    }
    out
}

/// 校验并处理附件：图片复制进会话工作区并生成片段；其他文件只记原文路径（不读不复制）。
///
/// 逐项失败不影响其他项：失败原因收集在 [`FinalizedAttachments::failures`]。
pub(crate) fn finalize(session_id: &str, sources: &[String]) -> FinalizedAttachments {
    let workspace = crate::tool::resolve_workspace(session_id);
    let dir = workspace.join(".astorm").join(ATTACHMENT_DIR);
    // 附件目录**首次真正写图片时**才建：只有非图片引用的消息不该在工作区留下空目录
    let mut dir_ready = false;

    let mut out = FinalizedAttachments {
        parts: Vec::new(),
        metas: Vec::new(),
        failures: Vec::new(),
    };
    let mut total: u64 = 0;
    let mut images = 0usize;
    let mut externals = 0usize;
    for source in sources {
        let display = source_display_name(source);
        match load_source(source) {
            Ok(Loaded::Image { bytes, format }) => {
                if images >= MAX_ATTACHMENTS {
                    out.failures.push(format!(
                        "{display}：超过单条消息附件上限（{MAX_ATTACHMENTS} 张），已忽略"
                    ));
                    continue;
                }
                let size = bytes.len() as u64;
                total = total.saturating_add(size);
                if total > MAX_ATTACHMENT_TOTAL_BYTES {
                    total -= size;
                    out.failures.push(format!(
                        "{display}：附件总量超过上限（{}），已忽略",
                        human_size(MAX_ATTACHMENT_TOTAL_BYTES)
                    ));
                    continue;
                }
                if !dir_ready {
                    if let Err(e) = std::fs::create_dir_all(&dir) {
                        out.failures.push(format!("附件目录创建失败: {e}"));
                        continue;
                    }
                    dir_ready = true;
                }
                let dest = dir.join(unique_file_name(&display, format.ext()));
                if let Err(e) = std::fs::write(&dest, &bytes) {
                    total -= size;
                    out.failures.push(format!("{display}：写入会话附件目录失败: {e}"));
                    continue;
                }
                out.parts.push(ContentPart::ImageUrl {
                    image_url: ImageUrlContent::new(data_uri(&bytes, format)),
                });
                out.metas.push(AttachmentMeta {
                    path: dest.to_string_lossy().into_owned(),
                    name: display,
                    mime: format.mime().to_string(),
                    size,
                });
                images += 1;
            }
            // 非图片：不读内容、不复制，只把**原文路径**交给模型（agent 按类型自行读取）
            Ok(Loaded::External { size }) => {
                if externals >= MAX_EXTERNAL_FILES {
                    out.failures.push(format!(
                        "{display}：超过单条消息文件数量上限（{MAX_EXTERNAL_FILES} 个），已忽略"
                    ));
                    continue;
                }
                externals += 1;
                out.metas.push(AttachmentMeta {
                    path: source.trim().to_string(),
                    name: display,
                    mime: EXTERNAL_MIME.to_string(),
                    size,
                });
            }
            Err(e) => out.failures.push(format!("{display}：{e}")),
        }
    }
    out
}

/// 冷启动重建：按消息 `metadata.attachments` 重新读盘，重建**图片**片段（缺失/超限跳过）。
///
/// 非图片（外部引用）本就没有片段，静默跳过——它们的内容入口是落库的用户消息文本里那段
/// 「【附件】…原文路径…」，不依赖此函数（也就不该在这里刷「格式不支持」的日志）。
///
/// 返回 `None` 表示无附件或全部不可用（调用方保持纯文本消息）。
pub(crate) fn rebuild_parts(metadata: &Value) -> Option<Vec<ContentPart>> {
    let metas = metas_of(metadata);
    if metas.is_empty() {
        return None;
    }
    let mut parts = Vec::with_capacity(metas.len());
    let mut total: u64 = 0;
    for meta in metas {
        if !is_image_meta(&meta) {
            continue;
        }
        let bytes = match std::fs::read(&meta.path) {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!("[attachment] 重建失败（文件不可读）: {} ({e})", meta.path);
                continue;
            }
        };
        let size = bytes.len() as u64;
        if size > MAX_ATTACHMENT_BYTES {
            tracing::warn!(
                "[attachment] 重建跳过（单张超限 {}）: {}",
                human_size(size),
                meta.path
            );
            continue;
        }
        total = total.saturating_add(size);
        if total > MAX_ATTACHMENT_TOTAL_BYTES {
            tracing::warn!("[attachment] 重建跳过（总量超限）: {}", meta.path);
            continue;
        }
        let Some(format) = detect_format(&bytes) else {
            tracing::warn!("[attachment] 重建跳过（格式不支持）: {}", meta.path);
            continue;
        };
        parts.push(ContentPart::ImageUrl {
            image_url: ImageUrlContent::new(data_uri(&bytes, format)),
        });
    }
    (!parts.is_empty()).then_some(parts)
}

/// 从消息 metadata 解析附件描述列表（缺省 / 非法结构 → 空）。
pub(crate) fn metas_of(metadata: &Value) -> Vec<AttachmentMeta> {
    metadata
        .get("attachments")
        .cloned()
        .and_then(|v| serde_json::from_value::<Vec<AttachmentMeta>>(v).ok())
        .unwrap_or_default()
}

/// 附件来源的两种形态（按文件头魔数判定，不看扩展名）。
enum Loaded {
    /// 已读进内存的图片（后续复制进会话工作区 + 内联 base64 上送）。
    Image { bytes: Vec<u8>, format: ImageFormat },
    /// 其他文件：**不读内容、不复制**，只记大小（agent 按原文路径自行读取）。
    External { size: u64 },
}

/// 读取附件来源（本地绝对路径；非绝对路径视为无效）。
///
/// 只读文件头即可区分两类：是图片 → 按图片口径校验并全量读；否则一律当「外部文件引用」——
/// 不做体积上限（不读内容也就没有 base64 膨胀），文件是否存在/可读由 `metadata` 这一步保证。
fn load_source(source: &str) -> Result<Loaded, String> {
    let path = Path::new(source.trim());
    if !path.is_absolute() {
        return Err("附件路径必须是绝对路径".into());
    }
    let meta = std::fs::metadata(path).map_err(|e| format!("读取失败: {e}"))?;
    if meta.is_dir() {
        return Err("这是目录，不是文件".into());
    }
    let head = read_head(path, HEAD_SNIFF_BYTES)?;
    if detect_format(&head).is_none() {
        return Ok(Loaded::External { size: meta.len() });
    }
    if meta.len() > MAX_ATTACHMENT_BYTES {
        return Err(format!(
            "图片过大（{}，上限 {}）",
            human_size(meta.len()),
            human_size(MAX_ATTACHMENT_BYTES)
        ));
    }
    let bytes = std::fs::read(path).map_err(|e| format!("读取失败: {e}"))?;
    if bytes.len() as u64 > MAX_ATTACHMENT_BYTES {
        return Err(format!(
            "图片过大（{}，上限 {}）",
            human_size(bytes.len() as u64),
            human_size(MAX_ATTACHMENT_BYTES)
        ));
    }
    let format = detect_format(&bytes)
        .ok_or("不支持的图片格式（仅支持 JPEG/PNG/GIF/WebP，按文件内容判定）")?;
    Ok(Loaded::Image { bytes, format })
}

/// 读文件头前 `n` 字节（文件更短则返回已有的部分；空文件 → 空）。
fn read_head(path: &Path, n: usize) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let mut file = std::fs::File::open(path).map_err(|e| format!("读取失败: {e}"))?;
    let mut buf = vec![0u8; n];
    let mut filled = 0usize;
    while filled < n {
        match file.read(&mut buf[filled..]) {
            Ok(0) => break,
            Ok(k) => filled += k,
            Err(e) => return Err(format!("读取失败: {e}")),
        }
    }
    buf.truncate(filled);
    Ok(buf)
}

/// 来源路径 → 展示名（文件名；取不到则原样路径）。
fn source_display_name(source: &str) -> String {
    Path::new(source.trim())
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| source.trim().to_string())
}

/// 目标文件名：`<uuid8>_<清洗后的原名>.<ext>`（原名便于在文件管理器里辨认）。
fn unique_file_name(display_name: &str, ext: &str) -> String {
    let stem = Path::new(display_name)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let cleaned: String = stem
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .take(MAX_STEM_CHARS)
        .collect();
    let cleaned = cleaned.trim_matches('_');
    let id = Uuid::new_v4().simple().to_string();
    let id = &id[..8];
    if cleaned.is_empty() {
        format!("{id}.{ext}")
    } else {
        format!("{id}_{cleaned}.{ext}")
    }
}

/// 字节数 → 人类可读（与 read_images 展示口径一致）。
fn human_size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = 1024.0 * 1024.0;
    let b = bytes as f64;
    if b >= MB {
        format!("{:.1} MB", b / MB)
    } else if b >= KB {
        format!("{:.0} KB", b / KB)
    } else {
        format!("{bytes} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// 最小合法 PNG 头（magic bytes 足够识别）。
    const PNG_HEAD: &[u8] = &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 0];
    const BMP_HEAD: &[u8] = &[0x42, 0x4D, 0x36, 0, 0, 0, 0, 0];

    fn tmp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "astorm-attachment-{tag}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(dir: &Path, name: &str, bytes: &[u8]) -> String {
        let path = dir.join(name);
        std::fs::write(&path, bytes).unwrap();
        path.to_string_lossy().into_owned()
    }

    /// 附件复制进会话工作区并生成元数据；工作区由 App 决定，测试内直接对返回的绝对路径断言。
    #[test]
    fn finalize_copies_into_session_workspace() {
        let src = tmp_dir("finalize");
        let png = write(&src, "截图.png", PNG_HEAD);
        let done = finalize("sess-attach-finalize", &[png.clone()]);

        assert!(done.failures.is_empty(), "failures: {:?}", done.failures);
        assert_eq!(done.parts.len(), 1);
        assert_eq!(done.metas.len(), 1);
        let meta = &done.metas[0];
        assert_eq!(meta.name, "截图.png");
        assert_eq!(meta.mime, "image/png");
        assert_eq!(meta.size, PNG_HEAD.len() as u64);
        // 落盘路径：会话工作区 .astorm/attachment/ 下，且内容与源一致
        assert!(
            meta.path.replace('\\', "/").contains("/.astorm/attachment/"),
            "path: {}",
            meta.path
        );
        assert!(meta.path.ends_with(".png"), "path: {}", meta.path);
        assert_eq!(std::fs::read(&meta.path).unwrap(), PNG_HEAD);
        // data URI 内联
        match &done.parts[0] {
            ContentPart::ImageUrl { image_url } => {
                assert!(image_url.url.starts_with("data:image/png;base64,"))
            }
            other => panic!("应为 image_url: {other:?}"),
        }
    }

    /// 路径非法 / 不存在 / 是目录 → 逐项失败，不影响其他项。
    #[test]
    fn finalize_reports_item_failures_without_failing_others() {
        let src = tmp_dir("failures");
        let png = write(&src, "ok.png", PNG_HEAD);
        let missing = src.join("nope.png").to_string_lossy().into_owned();
        let dir = src.join("adir");
        std::fs::create_dir_all(&dir).unwrap();
        let dir_s = dir.to_string_lossy().into_owned();

        let done = finalize("sess-attach-fail", &[missing, "relative.png".into(), dir_s, png]);
        assert_eq!(done.parts.len(), 1, "仅合法 PNG 成功");
        assert_eq!(done.metas.len(), 1, "失败项不产生元数据");
        assert_eq!(done.failures.len(), 3, "failures: {:?}", done.failures);
        assert!(done.failures[0].contains("读取失败"), "{:?}", done.failures);
        assert!(done.failures[1].contains("绝对路径"), "{:?}", done.failures);
        assert!(done.failures[2].contains("目录"), "{:?}", done.failures);
    }

    /// 非图片文件：**不复制进工作区**，只记原文路径；路径写进用户消息文本供模型读取。
    #[test]
    fn external_file_is_referenced_not_copied() {
        let src = tmp_dir("external");
        let bytes = b"%PDF-1.7\nhello\n";
        let doc = write(&src, "报告.pdf", bytes);
        let sid = "sess-attach-ext";

        let done = finalize(sid, &[doc.clone()]);
        assert!(done.failures.is_empty(), "failures: {:?}", done.failures);
        assert!(done.parts.is_empty(), "非图片不产生图片片段");
        assert_eq!(done.metas.len(), 1);
        let meta = &done.metas[0];
        assert_eq!(meta.path, doc, "保留原始路径（未复制）");
        assert_eq!(meta.name, "报告.pdf");
        assert_eq!(meta.size, bytes.len() as u64);
        assert!(!is_image_meta(meta), "非图片 meta: mime={}", meta.mime);

        // 会话附件目录里不该出现这份内容（同目录可能被并行用例写过图片，故按内容比对）
        let att_dir = crate::tool::resolve_workspace(sid).join(".astorm/attachment");
        if let Ok(entries) = std::fs::read_dir(&att_dir) {
            for e in entries.flatten() {
                let got = std::fs::read(e.path()).unwrap_or_default();
                assert_ne!(got, bytes, "非图片附件被复制了: {:?}", e.path());
            }
        }

        // 用户消息：文本里带原文路径（模型的唯一入口），且无图片片段 → 纯文本消息
        let msg = build_user_message(sid, "看看这个", Some(&[doc.clone()]));
        let content = msg.content.expect("应有文本");
        assert!(content.starts_with("看看这个"), "content: {content}");
        assert!(content.contains(&doc), "content: {content}");
        assert!(content.contains("未复制进工作区"), "content: {content}");
        assert!(msg.content_parts.is_none());
        // 元数据随消息落库（前端据此渲染文件胶囊）
        let metas = metas_of(msg.metadata.as_ref().unwrap());
        assert_eq!(metas.len(), 1);
        assert_eq!(metas[0].path, doc);
    }

    /// 不支持的图片格式（如 BMP）不再判失败：降级为**路径引用**让 agent 自己读。
    #[test]
    fn unsupported_image_format_falls_back_to_reference() {
        let src = tmp_dir("bmp");
        let bmp = write(&src, "图.bmp", BMP_HEAD);
        let done = finalize("sess-attach-bmp", &[bmp.clone()]);
        assert!(done.failures.is_empty(), "failures: {:?}", done.failures);
        assert!(done.parts.is_empty());
        assert_eq!(done.metas.len(), 1);
        assert_eq!(done.metas[0].path, bmp);
    }

    /// 重建：按 metadata.attachments 重读文件；缺失的跳过。
    #[test]
    fn rebuild_parts_reads_from_metadata_and_skips_missing() {
        let src = tmp_dir("rebuild");
        let png = write(&src, "a.png", PNG_HEAD);
        let meta = AttachmentMeta {
            path: png,
            name: "a.png".into(),
            mime: "image/png".into(),
            size: PNG_HEAD.len() as u64,
        };
        let missing = AttachmentMeta {
            path: src.join("gone.png").to_string_lossy().into_owned(),
            name: "gone.png".into(),
            mime: "image/png".into(),
            size: 1,
        };
        let metadata = json!({ "attachments": [meta, missing] });
        let parts = rebuild_parts(&metadata).expect("应重建出图片片段");
        assert_eq!(parts.len(), 1, "缺失文件被跳过");
        // 无附件 / 非法结构 → None
        assert!(rebuild_parts(&json!({})).is_none());
        assert!(rebuild_parts(&json!({ "attachments": "bad" })).is_none());
    }

    /// 消息装配：有图 → multimodal + metadata.attachments；无附件 → 纯文本（行为不变）。
    #[test]
    fn build_user_message_shape() {
        let src = tmp_dir("message");
        let png = write(&src, "b.png", PNG_HEAD);

        let plain = build_user_message("sess-attach-msg", "你好", None);
        assert!(plain.content_parts.is_none());
        assert!(plain.metadata.is_none());

        let with_image =
            build_user_message("sess-attach-msg", "看看这张", Some(&[png]));
        assert_eq!(with_image.role, "user");
        assert_eq!(with_image.content.as_deref(), Some("看看这张"));
        assert_eq!(with_image.content_parts.as_ref().unwrap().len(), 1);
        let metas = metas_of(with_image.metadata.as_ref().unwrap());
        assert_eq!(metas.len(), 1);
        // wire 格式：文本 + 图片两个 block（文本不丢）
        let wire = with_image.content_json().unwrap();
        assert_eq!(wire[0]["type"], json!("text"));
        assert_eq!(wire[1]["type"], json!("image_url"));
    }

    /// 失败项写进文本尾部（模型知道有附件缺失）。
    #[test]
    fn build_user_message_notes_failures() {
        let src = tmp_dir("note");
        let missing = src.join("gone.bmp").to_string_lossy().into_owned();
        let msg = build_user_message("sess-attach-note", "看图", Some(&[missing]));
        let content = msg.content.unwrap();
        assert!(content.starts_with("看图"));
        assert!(content.contains("以下附件未读入"), "content: {content}");
        assert!(msg.content_parts.is_none());
    }

    /// 落库 → 冷启动重建 全链路：附件描述进 metadata，重建后仍挂回 content_parts
    /// （跨轮/重启模型仍能看到用户发的图片）。
    #[test]
    fn record_round_trip_keeps_user_image_parts() {
        let src = tmp_dir("roundtrip");
        let png = write(&src, "c.png", PNG_HEAD);
        let msg = build_user_message("sess-attach-rt", "看图", Some(&[png]));

        let record = crate::agent::history::message_to_record(&msg, "sess-attach-rt", 0);
        assert!(
            record.metadata.as_ref().unwrap().get("attachments").is_some(),
            "附件描述应随消息 metadata 落库"
        );

        let back = crate::agent::history::record_to_message(&record).expect("记录应可还原");
        assert_eq!(back.content.as_deref(), Some("看图"));
        assert_eq!(
            back.content_parts.as_ref().map(|p| p.len()),
            Some(1),
            "重建后仍带图片片段"
        );
    }

    /// 结构化提及：技能/智能体/文件各有明确指令；命令跳过、同 token 去重、文件拼绝对路径。
    #[test]
    fn mentions_render_tool_directives() {
        let mentions = vec![
            MentionRef {
                kind: MentionKind::Skill,
                id: "ai-ppt".into(),
                token: "$ai-ppt".into(),
                label: None,
            },
            MentionRef {
                kind: MentionKind::Agent,
                id: "builtin-explore".into(),
                token: "$agent/builtin-explore".into(),
                label: Some("Explore".into()),
            },
            MentionRef {
                kind: MentionKind::File,
                id: "src/foo.ts".into(),
                token: "@src/foo.ts".into(),
                label: None,
            },
            MentionRef {
                kind: MentionKind::Command,
                id: "goal".into(),
                token: "/goal".into(),
                label: None,
            },
            // 同 token 重复：指令只写一条
            MentionRef {
                kind: MentionKind::Skill,
                id: "ai-ppt".into(),
                token: "$ai-ppt".into(),
                label: None,
            },
        ];
        let base = build_user_message("sess-mention", "帮我看下", None);
        let note = render_refs_note(&mentions, &[], &[], &[], Some("/tmp/ws"));
        let msg = append_note(base, &note);
        let content = msg.content.unwrap();

        assert!(content.starts_with("帮我看下"), "content: {content}");
        assert!(content.contains("【引用指令】"));
        assert!(content.contains("skills 工具"), "content: {content}");
        assert_eq!(content.matches("技能「ai-ppt」").count(), 1, "同 token 去重");
        assert!(content.contains("agent_type=\"builtin-explore\""), "content: {content}");
        let abs = Path::new("/tmp/ws").join("src/foo.ts").to_string_lossy().to_string();
        assert!(content.contains(&abs), "content: {content}");
        assert!(!content.contains("/goal"), "命令不应出现: {content}");
        // metadata 不受提及影响
        assert!(msg.metadata.is_none());
    }

    /// 元素围栏原文随「引用说明」一起拼进喂模型副本；无提及时也生效。
    #[test]
    fn render_refs_note_includes_element_fences() {
        let fences = vec![
            "```element\n{\"tag\":\"a\"}\n```".to_string(),
            "```element\n{\"tag\":\"button\"}\n```".to_string(),
        ];
        let note = render_refs_note(&[], &fences, &[], &[], None);
        assert!(note.contains("```element"), "note: {note}");
        assert_eq!(note.matches("```element").count(), 2, "note: {note}");
        // 空说明原样返回
        assert!(render_refs_note(&[], &[], &[], &[], None).is_empty());
    }

    /// 对话正文引用:渲染为「【引用对话】」块,带来源角色与序号。
    #[test]
    fn render_refs_note_includes_quotes() {
        let quotes = vec![
            QuoteRef {
                text: "过程中修掉的问题".into(),
                seq: Some(430),
                role: Some("assistant".into()),
            },
            QuoteRef {
                text: "用户的一段话".into(),
                seq: Some(12),
                role: Some("user".into()),
            },
        ];
        let note = render_refs_note(&[], &[], &[], &quotes, None);
        assert!(note.contains("【引用对话】"), "note: {note}");
        assert!(note.contains("助手消息 #430"), "note: {note}");
        assert!(note.contains("用户消息 #12"), "note: {note}");
        assert!(note.contains("过程中修掉的问题"), "note: {note}");
    }

    /// 「引用说明」写进 metadata 后可原样读回（冷启动重建据它拼回正文）。
    #[test]
    fn refs_note_round_trips_through_metadata() {
        let mut metadata = None;
        set_refs_note(&mut metadata, "【引用指令】\n- x");
        assert_eq!(refs_note_of(metadata.as_ref()), "【引用指令】\n- x");
        // 空说明不写、读缺省为空
        let mut empty = None;
        set_refs_note(&mut empty, "   ");
        assert!(empty.is_none());
        assert!(refs_note_of(None).is_empty());
    }
}
