//! `read_images` 工具：读取本地 / 远程图片，作为 user 多模态消息交给模型「看」（图片理解）。
//!
//! 依据 DeepSeek 图片理解文档（OpenAI 兼容 `/chat/completions`）：
//! - 图片只能出现在 **user** 消息，block 形如 `{"type":"image_url","image_url":{"url":…,"detail":…}}`
//! - 支持 JPEG / PNG / GIF / WebP（**按文件内容判定**，不看扩展名与声明 MIME）
//! - 三种输入方式：base64 data URI（本地文件走这条）、外链 URL（模型端自动下载，≤8192 字符）、
//!   Files API `file_id`（本工具不使用）
//! - `detail` 可取 low / high / original / auto；low 会缩到 512×512 更省 token
//!
//! 实现要点：
//! - 工具结果本身只能是文本 `tool` 消息，故图片经 [`ToolContext::inject_message`] 注入为
//!   **user 多模态消息**，由引擎在本轮 `tool_result` 之后追加（仅本轮循环可见）
//! - 本地路径策略与 `file_operation` 的读操作一致：Auto / 已授权直接读；Plan 空间内读放行；
//!   空间外读返回审批信封（用户同意后系统自动重放，重放仍走本工具，故注入照常生效）
//! - 远程 URL 默认**原样透传**（省带宽、不受请求体上限约束）；URL 超长或 `inline=true`
//!   时服务端下载后内联 base64，并做格式 / 体积校验
//! - 规模上限：单次 ≤20 张、单张 ≤20MB、单次内联总量 ≤30MB（base64 膨胀后 ≈40MB < 48MB 请求体）

use std::path::{Path, PathBuf};
use std::time::Duration;

use async_trait::async_trait;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use llm_provider::{ChatMessage, ContentPart, ImageUrlContent};
use serde::Deserialize;
use serde_json::json;

use crate::tool::denial::{Denial, DenialCategory, DenialSource};
use crate::tool::{expand_session_vars, resolve_workspace};

/// 单次调用图片数量上限（DeepSeek 上限 600，这里按实用规模收紧）。
const MAX_IMAGES: usize = 20;
/// 单张图片原始字节上限（DeepSeek 单图上限 32MiB，这里更保守）。
const MAX_IMAGE_BYTES: u64 = 20 * 1024 * 1024;
/// 单次内联（base64）图片的**原始字节**总量上限：膨胀 ≈4/3 → ≈40MiB，为 48MiB 请求体留余量。
const MAX_TOTAL_BYTES: u64 = 30 * 1024 * 1024;
/// 外链 URL 透传的字符上限（DeepSeek 文档）。
const MAX_URL_CHARS: usize = 8192;
/// 远程下载超时（DeepSeek 要求 60s 内完成下载）。
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(60);
/// `detail` 合法取值。
const DETAILS: &[&str] = &["auto", "low", "high", "original"];

/// `read_images` 工具。
pub struct ReadImagesTool;

/// 工具参数。
#[derive(Debug, Deserialize)]
struct ReadImagesArgs {
    /// 图片列表：每项为本地路径 / http(s) URL / data URI。
    images: Vec<String>,
    /// 处理精度：auto（默认）/ low / high / original。
    #[serde(default)]
    detail: Option<String>,
    /// 远程 URL 是否下载后内联 base64（默认 false：原样透传由模型端下载）。
    #[serde(default)]
    inline: Option<bool>,
}

/// 支持的图片格式（按 magic bytes 判定）。
///
/// `pub(crate)`：用户消息附件（`agent::attachment`）复用同一套格式判定与 data URI 组装，
/// 避免两处各写一份 magic bytes 表。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ImageFormat {
    Jpeg,
    Png,
    Gif,
    Webp,
}

impl ImageFormat {
    pub(crate) fn mime(self) -> &'static str {
        match self {
            ImageFormat::Jpeg => "image/jpeg",
            ImageFormat::Png => "image/png",
            ImageFormat::Gif => "image/gif",
            ImageFormat::Webp => "image/webp",
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            ImageFormat::Jpeg => "JPEG",
            ImageFormat::Png => "PNG",
            ImageFormat::Gif => "GIF",
            ImageFormat::Webp => "WebP",
        }
    }

    /// 落盘扩展名（附件复制进工作区时用格式判定结果定扩展名，而非沿用原扩展名）。
    pub(crate) fn ext(self) -> &'static str {
        match self {
            ImageFormat::Jpeg => "jpg",
            ImageFormat::Png => "png",
            ImageFormat::Gif => "gif",
            ImageFormat::Webp => "webp",
        }
    }
}

/// 图片输入来源。
#[derive(Debug, Clone)]
enum Source {
    /// 外链 URL / `data:` URI（原样透传，或按 `inline` 下载内联）。
    Url(String),
    /// 本地文件路径（已展开会话变量、相对工作区锚定）。
    Local(PathBuf),
}

/// 单张已就绪的图片。
#[derive(Debug)]
struct LoadedImage {
    /// wire 格式的图片引用。
    part: ImageUrlContent,
    /// 内联原始字节数（外链透传记 0，不计入内联总量）。
    bytes: u64,
    /// 摘要行（供工具结果文本展示）。
    desc: String,
}

#[async_trait]
impl crate::tool::Tool for ReadImagesTool {
    fn name(&self) -> &str {
        "read_images"
    }

    fn description(&self) -> &str {
        "读取本地或远程图片并交给模型查看（图片理解，需当前模型支持视觉）。支持 JPEG/PNG/GIF/WebP；images 每项为本地文件路径（相对会话工作区或绝对路径，支持 $SESSION_WORKSPACE）或 http(s) URL / data URI。本地文件读取后以 base64 内联；远程 URL 默认原样透传给模型端下载（可传 inline=true 改为服务端下载后内联）。图片会作为随后的用户消息附上，成功后请直接查看图片内容。单次最多 20 张，不支持的格式与超限项会逐条报错。"
    }

    /// 审批拦截结果（纯 JSON）→ 中断载荷（与 file_operation 同构）。
    fn interrupt_payload(&self, result: &str) -> Option<serde_json::Value> {
        crate::tool::policy::interrupt_request_from_result(result)
    }

    /// 硬拒绝（用户拒绝 / 被拒重放）：错误原文即拒绝信封 → 分类元数据。
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
                "images": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "图片列表，每项为本地路径（相对会话工作区或绝对路径，支持 $SESSION_WORKSPACE）、http(s) URL 或 data:image/... URI"
                },
                "detail": {
                    "type": "string",
                    "enum": DETAILS,
                    "description": "图片处理精度：auto（默认）/ low（缩到 512×512，更快更省 token）/ high / original"
                },
                "inline": {
                    "type": "boolean",
                    "description": "远程 URL 是否下载后内联为 base64（默认 false：原样透传由模型端下载）"
                }
            },
            "required": ["images"]
        })
    }

    async fn execute(&self, params: serde_json::Value, ctx: &crate::tool::ToolContext) -> Result<String, String> {
        let args: ReadImagesArgs =
            serde_json::from_value(params).map_err(|e| format!("read_images 参数解析失败: {e}"))?;

        // ── 参数校验 ────────────────────────────────────────────
        if args.images.is_empty() {
            return Err("images 不能为空：请给出至少一个本地路径或 URL".into());
        }
        if args.images.len() > MAX_IMAGES {
            return Err(format!(
                "一次最多读取 {MAX_IMAGES} 张图片（收到 {} 张），请分批调用",
                args.images.len()
            ));
        }
        let detail = match args.detail.as_deref().map(str::trim).filter(|d| !d.is_empty()) {
            Some(d) if DETAILS.contains(&d) => Some(d.to_string()),
            Some(d) => {
                return Err(format!(
                    "detail 取值非法: {d}（可选 {}）",
                    DETAILS.join(" / ")
                ));
            }
            None => None,
        };

        // ── 来源分类（本地 / URL）+ 本地路径策略（与 file_operation 读一致）──
        let workspace = resolve_workspace(&ctx.session_id);
        let sources: Vec<Source> = args
            .images
            .iter()
            .map(|raw| classify(raw, &ctx.session_id, &workspace))
            .collect();
        let blocked: Vec<PathBuf> = {
            let locals = sources.iter().filter_map(|s| match s {
                Source::Local(p) => Some(p.clone()),
                Source::Url(_) => None,
            });
            if crate::tool::policy::auto_guard(&ctx.session_id) {
                Vec::new()
            } else {
                locals
                    .filter(|p| !local_allowed(&workspace, p, &ctx.session_id))
                    .collect()
            }
        };
        if !blocked.is_empty() {
            // 重放闸门 key：整次调用一条命令（用户同意一次即放行这批路径）
            let command = format!("read_images {}", join_paths(&blocked));
            if crate::tool::policy::consume_allowed_rerun(&ctx.session_id, &command) {
                // 已授权 → 放行
            } else if crate::tool::policy::is_denied(&ctx.session_id, &command) {
                return Err(format!(
                    "该图片读取已被用户拒绝：{command}\n请改换思路，不要重复读取同一批被拒路径。"
                ));
            } else {
                return Ok(ask_read(&workspace, &blocked, ctx));
            }
        }

        // ── 逐项加载 ────────────────────────────────────────────
        let inline = args.inline.unwrap_or(false);
        let mut client: Option<reqwest::Client> = None;
        let mut loaded: Vec<LoadedImage> = Vec::new();
        let mut failures: Vec<String> = Vec::new();
        let mut total_bytes: u64 = 0;

        for (raw, source) in args.images.iter().zip(sources.iter()) {
            let item = match source {
                Source::Local(path) => load_local(path),
                Source::Url(url) if url.starts_with("data:") => Ok(LoadedImage {
                    part: ImageUrlContent::new(url.clone()).with_detail(detail.clone()),
                    bytes: 0,
                    desc: format!("内联 data URI（{}）", url.chars().take(48).collect::<String>()),
                }),
                Source::Url(url) if !inline && url.chars().count() <= MAX_URL_CHARS => {
                    // 外链透传：由模型端下载（省带宽、不占请求体）
                    Ok(LoadedImage {
                        part: ImageUrlContent::new(url.clone()).with_detail(detail.clone()),
                        bytes: 0,
                        desc: format!("远程 {url}（外链透传，由模型端下载）"),
                    })
                }
                Source::Url(url) => {
                    if client.is_none() {
                        client = Some(http_client()?);
                    }
                    let http = client.as_ref().expect("已初始化");
                    download_image(http, url, detail.as_deref()).await
                }
            };
            match item {
                Ok(image) => {
                    if image.bytes > 0 {
                        total_bytes = total_bytes.saturating_add(image.bytes);
                        if total_bytes > MAX_TOTAL_BYTES {
                            failures.push(format!(
                                "{raw}：本次调用内联图片总量超过上限（{}），请减少数量或改用外链透传",
                                human_size(MAX_TOTAL_BYTES)
                            ));
                            total_bytes -= image.bytes;
                            continue;
                        }
                    }
                    loaded.push(image);
                }
                Err(e) => failures.push(format!("{raw}：{e}")),
            }
        }

        if loaded.is_empty() {
            return Err(format!("没有可用的图片。\n{}", failures.join("\n")));
        }

        // ── 注入 user 多模态消息（引擎在本轮 tool_result 之后追加）+ 返回摘要文本 ──
        let summary = build_summary(&loaded);
        let parts: Vec<ContentPart> = loaded
            .iter()
            .map(|i| ContentPart::ImageUrl {
                image_url: i.part.clone(),
            })
            .collect();
        // content 文本会由 ChatMessage::content_json 并为首个 text block（图片前）
        let mut message = ChatMessage::multimodal(parts);
        message.content = Some(summary.clone());
        ctx.inject_message(message);

        // 工具结果文本：成功摘要 + 逐项失败明细（失败项不进注入消息，模型从 tool 消息即可看到）
        if failures.is_empty() {
            return Ok(summary);
        }
        let mut result = summary;
        result.push_str("\n\n以下项未读取：");
        for failure in &failures {
            result.push_str(&format!("\n- {failure}"));
        }
        Ok(result)
    }
}

/// 工具名（重放 / 审批登记用）。
const TOOL_NAME: &str = "read_images";

/// 分类来源：URL（http(s) / data:）原样透传处理，其余展开会话变量后按本地路径锚定工作区。
fn classify(raw: &str, session_id: &str, workspace: &Path) -> Source {
    let trimmed = raw.trim();
    let lower = trimmed.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") || lower.starts_with("data:") {
        return Source::Url(trimmed.to_string());
    }
    let expanded = expand_session_vars(trimmed, session_id, workspace);
    let path = Path::new(&expanded);
    Source::Local(if path.is_absolute() {
        path.to_path_buf()
    } else {
        workspace.join(path)
    })
}

/// 本地读是否已授权（工作区内 / 安全配置白名单 / 会话白名单）。
fn local_allowed(workspace: &Path, path: &Path, session_id: &str) -> bool {
    crate::tool::policy::path_in_workspace(workspace, path)
        || crate::tool::policy::path_allowed_by_security(path, false)
        || crate::tool::policy::path_allowed_by_session(session_id, path, false)
}

/// 空间外读的审批信封（登记待决议 + 审计，与 `file_operation` 的 `ask_file` 同构）。
fn ask_read(workspace: &Path, blocked: &[PathBuf], ctx: &crate::tool::ToolContext) -> String {
    let targets: Vec<String> = blocked
        .iter()
        .map(|p| p.strip_prefix(workspace).unwrap_or(p).display().to_string())
        .collect();
    let command = format!("{TOOL_NAME} {}", join_paths(blocked));
    let denial = Denial::new(
        DenialCategory::FileRead,
        DenialSource::StaticPrecheck,
        format!("读取会话空间外图片：{}", targets.join("、")),
    )
    .with_targets(targets)
    .with_command(command.clone());
    crate::tool::policy::audit(
        &ctx.session_id,
        crate::tool::policy::audit_kind_of(DenialCategory::FileRead),
        &denial.detail,
        crate::tool::audit::AuditApproval::None,
        Some(command.as_str()),
        Some(DenialCategory::FileRead),
        Some(denial.source),
    );
    crate::tool::policy::needs_approval(crate::tool::policy::ApprovalReq {
        denial,
        command: &command,
        session_id: &ctx.session_id,
        tool_name: TOOL_NAME,
        tool_call_id: &ctx.tool_call_id,
    })
}

/// 路径列表 → 逗号分隔（重放闸门 key 的稳定形态）。
fn join_paths(paths: &[PathBuf]) -> String {
    paths
        .iter()
        .map(|p| p.display().to_string())
        .collect::<Vec<_>>()
        .join(",")
}

/// 读取本地图片：体积 → magic bytes → base64 data URI。
fn load_local(path: &Path) -> Result<LoadedImage, String> {
    let meta = std::fs::metadata(path).map_err(|e| format!("读取失败: {e}"))?;
    if meta.is_dir() {
        return Err("这是一个目录，不是图片文件".into());
    }
    if meta.len() > MAX_IMAGE_BYTES {
        return Err(format!(
            "图片过大（{}，上限 {}）",
            human_size(meta.len()),
            human_size(MAX_IMAGE_BYTES)
        ));
    }
    let bytes = std::fs::read(path).map_err(|e| format!("读取失败: {e}"))?;
    if bytes.len() as u64 > MAX_IMAGE_BYTES {
        return Err(format!(
            "图片过大（{}，上限 {}）",
            human_size(bytes.len() as u64),
            human_size(MAX_IMAGE_BYTES)
        ));
    }
    let format = detect_format(&bytes)
        .ok_or("不支持的图片格式（仅支持 JPEG/PNG/GIF/WebP，按文件内容判定）")?;
    let raw_len = bytes.len() as u64;
    Ok(LoadedImage {
        part: ImageUrlContent::new(data_uri(&bytes, format)),
        bytes: raw_len,
        desc: format!(
            "本地 {}（{}，{}）",
            path.display(),
            format.label(),
            human_size(raw_len)
        ),
    })
}

/// 下载远程图片并内联（`inline=true` 或 URL 超长时）。
async fn download_image(
    client: &reqwest::Client,
    url: &str,
    detail: Option<&str>,
) -> Result<LoadedImage, String> {
    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("下载失败: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("下载失败: HTTP {}", resp.status().as_u16()));
    }
    if let Some(len) = resp.content_length()
        && len > MAX_IMAGE_BYTES
    {
        return Err(format!(
            "图片过大（{}，上限 {}）",
            human_size(len),
            human_size(MAX_IMAGE_BYTES)
        ));
    }
    let bytes = resp.bytes().await.map_err(|e| format!("下载内容读取失败: {e}"))?;
    if bytes.len() as u64 > MAX_IMAGE_BYTES {
        return Err(format!(
            "图片过大（{}，上限 {}）",
            human_size(bytes.len() as u64),
            human_size(MAX_IMAGE_BYTES)
        ));
    }
    let format = detect_format(&bytes)
        .ok_or("不支持的图片格式（仅支持 JPEG/PNG/GIF/WebP，按文件内容判定）")?;
    let raw_len = bytes.len() as u64;
    Ok(LoadedImage {
        part: ImageUrlContent::new(data_uri(&bytes, format))
            .with_detail(detail.map(str::to_string)),
        bytes: raw_len,
        desc: format!(
            "远程 {url}（已下载内联，{}，{}）",
            format.label(),
            human_size(raw_len)
        ),
    })
}

/// HTTP 客户端（仅下载内联时需要；连接复用）。
fn http_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(DOWNLOAD_TIMEOUT)
        .user_agent("astorm-read-images/1.0")
        .build()
        .map_err(|e| format!("HTTP 客户端初始化失败: {e}"))
}

/// 按 magic bytes 识别图片格式（与扩展名 / 声明 MIME 无关，与 DeepSeek 判定一致）。
pub(crate) fn detect_format(bytes: &[u8]) -> Option<ImageFormat> {
    if bytes.len() >= 3 && bytes[0] == 0xFF && bytes[1] == 0xD8 && bytes[2] == 0xFF {
        return Some(ImageFormat::Jpeg);
    }
    if bytes.len() >= 8 && bytes[..8] == [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A] {
        return Some(ImageFormat::Png);
    }
    if bytes.len() >= 6 && (&bytes[..6] == b"GIF87a" || &bytes[..6] == b"GIF89a") {
        return Some(ImageFormat::Gif);
    }
    if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return Some(ImageFormat::Webp);
    }
    None
}

/// 原始字节 → `data:<mime>;base64,<...>`（OpenAI 兼容内联图片格式；附件装配同样复用）。
pub(crate) fn data_uri(bytes: &[u8], format: ImageFormat) -> String {
    format!("data:{};base64,{}", format.mime(), BASE64.encode(bytes))
}

/// 已读取图片的摘要（注入消息与工具结果共用的正文）。
fn build_summary(loaded: &[LoadedImage]) -> String {
    let mut out = format!(
        "已读取 {} 张图片（图片已附在本轮对话的用户消息中，请直接查看）：",
        loaded.len()
    );
    for image in loaded {
        out.push_str(&format!("\n- {}", image.desc));
    }
    out
}

/// 字节数 → 人类可读（KB / MB）。
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
    use react_loop::{Tool, ToolContext};
    use tokio_util::sync::CancellationToken;

    /// 最小合法 PNG 头（magic bytes 足够识别格式，不需要真实像素数据）。
    const PNG_HEAD: &[u8] = &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 0];
    const BMP_HEAD: &[u8] = &[0x42, 0x4D, 0x36, 0, 0, 0, 0, 0];

    fn ctx() -> ToolContext {
        ToolContext::new(CancellationToken::new()).with_session_id("sess-read-images")
    }

    fn tmp_file(tag: &str, name: &str, bytes: &[u8]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "astorm-read-images-{tag}-{}",
            std::process::id()
        ));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    }

    #[test]
    fn detect_format_by_magic_bytes() {
        assert_eq!(detect_format(PNG_HEAD), Some(ImageFormat::Png));
        assert_eq!(
            detect_format(&[0xFF, 0xD8, 0xFF, 0xE0]),
            Some(ImageFormat::Jpeg)
        );
        assert_eq!(detect_format(b"GIF89a...."), Some(ImageFormat::Gif));
        let mut webp = Vec::from(*b"RIFF");
        webp.extend_from_slice(&[0, 0, 0, 0]);
        webp.extend_from_slice(b"WEBP");
        assert_eq!(detect_format(&webp), Some(ImageFormat::Webp));
        // 非支持格式：BMP / TIFF / PDF / 纯文本
        assert_eq!(detect_format(BMP_HEAD), None);
        assert_eq!(detect_format(&[0x49, 0x49, 0x2A, 0x00]), None);
        assert_eq!(detect_format(b"%PDF-1.7"), None);
        assert_eq!(detect_format(b"hello"), None);
    }

    #[test]
    fn data_uri_carries_mime_and_base64() {
        let uri = data_uri(PNG_HEAD, ImageFormat::Png);
        assert!(uri.starts_with("data:image/png;base64,"), "uri: {uri}");
        let payload = uri.trim_start_matches("data:image/png;base64,");
        assert_eq!(BASE64.decode(payload).unwrap(), PNG_HEAD);
    }

    #[test]
    fn load_local_returns_data_uri_and_rejects_unsupported() {
        let png = tmp_file("png", "pic.png", PNG_HEAD);
        let image = load_local(&png).expect("PNG 应可读");
        assert!(image.part.url.starts_with("data:image/png;base64,"));
        assert_eq!(image.part.detail, None);
        assert_eq!(image.bytes, PNG_HEAD.len() as u64);
        assert!(image.desc.contains("PNG"));

        // 扩展名是图片但内容不是受支持格式 → 拒绝（不认扩展名）
        let bmp = tmp_file("bmp", "pic.bmp", BMP_HEAD);
        let err = load_local(&bmp).unwrap_err();
        assert!(err.contains("不支持的图片格式"), "err: {err}");

        let missing = std::env::temp_dir().join("astorm-read-images-missing.png");
        assert!(load_local(&missing).unwrap_err().contains("读取失败"));
    }

    #[test]
    fn human_size_scales() {
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(2048), "2 KB");
        assert_eq!(human_size(2 * 1024 * 1024), "2.0 MB");
    }

    #[tokio::test]
    async fn url_passthrough_injects_user_image_message() {
        let ctx = ctx();
        let out = ReadImagesTool
            .execute(
                json!({ "images": ["https://example.com/a.jpg"], "detail": "low" }),
                &ctx,
            )
            .await
            .expect("外链应透传成功");
        assert!(out.contains("已读取 1 张图片"), "out: {out}");
        assert!(out.contains("外链透传"), "out: {out}");

        let injected = ctx.take_injections();
        assert_eq!(injected.len(), 1);
        let msg = &injected[0];
        assert_eq!(msg.role, "user");
        assert!(msg.content.as_deref().unwrap_or_default().contains("已读取 1 张图片"));
        let parts = msg.content_parts.as_ref().expect("应带图片 parts");
        assert_eq!(parts.len(), 1);
        match &parts[0] {
            ContentPart::ImageUrl { image_url } => {
                assert_eq!(image_url.url, "https://example.com/a.jpg");
                assert_eq!(image_url.detail.as_deref(), Some("low"));
            }
            other => panic!("应为 image_url part: {other:?}"),
        }
        // wire 格式：content 为数组（文本 + 图片），文本不丢
        let wire = msg.content_json().unwrap();
        assert!(wire.is_array());
        assert_eq!(wire[0]["type"], json!("text"));
        assert_eq!(wire[1]["type"], json!("image_url"));
        assert_eq!(wire[1]["image_url"]["detail"], json!("low"));
    }

    #[tokio::test]
    async fn args_are_validated() {
        let ctx = ctx();
        let tool = ReadImagesTool;
        assert!(tool.execute(json!({ "images": [] }), &ctx).await.unwrap_err().contains("不能为空"));
        let many: Vec<String> = (0..=MAX_IMAGES).map(|i| format!("https://x/{i}.png")).collect();
        assert!(
            tool.execute(json!({ "images": many }), &ctx)
                .await
                .unwrap_err()
                .contains("最多读取")
        );
        assert!(
            tool.execute(json!({ "images": ["https://x/a.png"], "detail": "huge" }), &ctx)
                .await
                .unwrap_err()
                .contains("detail 取值非法")
        );
    }

    /// 默认（无会话记录 → Plan）+ 空间外绝对路径 → 审批信封，且不注入图片。
    #[tokio::test]
    async fn out_of_workspace_local_path_returns_approval_envelope() {
        let ctx = ctx();
        let png = tmp_file("approval", "pic.png", PNG_HEAD);
        let out = ReadImagesTool
            .execute(json!({ "images": [png.display().to_string()] }), &ctx)
            .await
            .expect("空间外读应返回审批信封而非报错");
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["interrupt"], json!(true));
        assert_eq!(v["interrupt_type"], json!("approval"));
        assert_eq!(v["approval"]["category"], json!("file_read"));
        assert!(v["approval"]["command"].as_str().unwrap().starts_with("read_images "));
        assert!(ctx.take_injections().is_empty(), "待审批不应注入图片");
    }

    /// 下载类失败：URL 超长（>8192 字符）触发服务端下载，目标不可达 → 逐项失败、整体报错。
    #[tokio::test]
    async fn long_url_is_downloaded_and_reports_failure_without_network() {
        let ctx = ctx();
        let url = format!("https://127.0.0.1:9/{}", "a".repeat(MAX_URL_CHARS));
        let err = ReadImagesTool
            .execute(json!({ "images": [url] }), &ctx)
            .await
            .unwrap_err();
        assert!(err.contains("没有可用的图片"), "err: {err}");
        assert!(err.contains("下载失败"), "err: {err}");
        assert!(ctx.take_injections().is_empty(), "全部失败不应注入图片");
    }
}
