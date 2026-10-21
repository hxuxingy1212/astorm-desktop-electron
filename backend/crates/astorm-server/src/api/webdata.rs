//! Web 桥的文件类端点：fs 读写 / 附件上传与直读 / KB 文件 / office 预览。
//!
//! 与桌面自定义协议（astorm-attachment:// / astorm-kb://）语义对齐：
//! - 附件直读只允许 staging 目录与会话工作区 `.astorm/attachment/`（同桌面 allow-list）；
//! - KB 文件只允许知识库根目录之内（越界 403）；
//! - office 预览复用随镜像分发的 `json2*` CLI（unpack --inline 单文件 JSON + 缓存）。

use axum::extract::{Path, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use base64::Engine as _;
use serde_json::{json, Value};

use astorm_core::app::App;

use crate::error::{ok, ApiError};
use crate::local;
use crate::state::ServerState;

type R = Result<Json<Value>, ApiError>;

/// 能力探测图：白底黑字 739（验证模型同时具备文本指令遵循与图片识别）
const PROBE_IMAGE_BASE64: &str = "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";
const PROBE_PROMPT: &str = "Read the number in the image. Reply exactly as: PINEAPPLE <number>";

/// 用户目录根（`AppConfig.workspace`；容器内即数据卷 `/data`，下辖 workspace/ knowledge/
/// memory/）。**这是 fs 端点与目录选择器的浏览边界**：令牌持有者=容器属主，但越出卷
/// （/etc/shadow 之类）一律拒绝。
fn user_root() -> Option<std::path::PathBuf> {
    App::try_global().map(|app| app.dir().user_dir.clone())
}

/// 会话工作区根（`<用户目录根>/workspace`；目录选择器的缺省起点）。
fn session_workspace_root() -> Option<std::path::PathBuf> {
    App::try_global().map(|app| app.dir().workspace_dir.clone())
}

fn mime_of(ext: &str) -> &'static str {
    match ext {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "ico" => "image/x-icon",
        "bmp" => "image/bmp",
        "avif" => "image/avif",
        "css" => "text/css",
        "html" => "text/html",
        "js" | "mjs" => "text/javascript",
        "json" => "application/json",
        "txt" | "md" | "log" => "text/plain; charset=utf-8",
        "pdf" => "application/pdf",
        "wasm" => "application/wasm",
        "mp3" | "wav" | "ogg" => "audio/mpeg",
        "mp4" | "webm" => "video/mp4",
        _ => "application/octet-stream",
    }
}

/// sniff 图片魔数（与桌面 attachmentFiles 的按内容识别一致）。
fn sniff_mime(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG") {
        Some("image/png")
    } else if bytes.starts_with(b"\xff\xd8\xff") {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF8") {
        Some("image/gif")
    } else if bytes.len() > 12 && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else if bytes.starts_with(b"%PDF") {
        Some("application/pdf")
    } else {
        None
    }
}

/* ================= fs ================= */

/// fs 端点的路径白名单：只允许**工作区之内**（含附件/知识库根目录由各自端点负责）。
/// 桌面版这些通道本来就限定在工作区文件树里使用；云形态收紧为同一边界
/// （令牌持有者 = 容器属主，但越出工作区读 /etc/shadow 之类的路径直接拒绝）。
fn fs_path_allowed(path: &std::path::Path) -> bool {
    let Some(root) = user_root() else { return false };
    let norm = lexical_normalize(path);
    norm.starts_with(lexical_normalize(&root))
}

/// 词法规范化：解析 `.`/`..`（不追踪符号链接；符号链接由容器文件系统边界兜底）。
fn lexical_normalize(path: &std::path::Path) -> std::path::PathBuf {
    use std::path::Component;
    let mut out = std::path::PathBuf::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// `GET /api/v1/fs/read-base64?path=`：读文件为 base64（图片预览；8MB 上限，stat 先行）。
pub async fn read_base64(
    State(_s): State<ServerState>,
    axum::extract::Query(q): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Json<Value>, ApiError> {
    let path = q.get("path").cloned().unwrap_or_default();
    let p = std::path::PathBuf::from(&path);
    if !fs_path_allowed(&p) {
        return Err(ApiError::bad_request("路径超出工作区范围"));
    }
    let meta = std::fs::metadata(&p).map_err(|e| ApiError::not_found(format!("读取失败: {e}")))?;
    if meta.len() > 8 * 1024 * 1024 {
        return Ok(Json(json!({ "base64": "" })));
    }
    let bytes = std::fs::read(&p).map_err(|e| ApiError::bad_request(format!("读取失败: {e}")))?;
    let mime = sniff_mime(&bytes);
    Ok(Json(json!({
        "base64": base64::engine::general_purpose::STANDARD.encode(&bytes),
        "mime": mime,
    })))
}

/// `GET /api/v1/fs/read-bytes?path=`：读二进制（office 等预览；64MB 上限，stat 先行）。
pub async fn read_bytes(
    State(_s): State<ServerState>,
    axum::extract::Query(q): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Response, ApiError> {
    let path = q.get("path").cloned().unwrap_or_default();
    let p = std::path::PathBuf::from(&path);
    if !fs_path_allowed(&p) {
        return Err(ApiError::bad_request("路径超出工作区范围"));
    }
    let meta = std::fs::metadata(&p).map_err(|_| ApiError::not_found("文件不存在或不可读"))?;
    if meta.len() > 64 * 1024 * 1024 {
        return Ok(StatusCode::PAYLOAD_TOO_LARGE.into_response());
    }
    let bytes = std::fs::read(&p).map_err(|_| ApiError::not_found("文件不存在或不可读"))?;
    let ext = path.rsplit('.').next().unwrap_or("").to_lowercase();
    Ok((
        [(header::CONTENT_TYPE, mime_of(&ext))],
        bytes,
    )
        .into_response())
}

/// `POST /api/v1/fs/write`：写文本文件（markdown 保存；限定工作区）。
pub async fn write_text(
    State(_s): State<ServerState>,
    Json(body): Json<Value>,
) -> R {
    let path = body.get("path").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let content = body.get("content").and_then(|v| v.as_str()).unwrap_or("");
    let p = std::path::PathBuf::from(&path);
    if !fs_path_allowed(&p) {
        return Err(ApiError::bad_request("路径超出工作区范围"));
    }
    std::fs::write(&p, content).map_err(|e| ApiError::bad_request(format!("写入失败: {e}")))?;
    Ok(ok(json!({ "ok": true })))
}

/* ================= 目录选择器（web 端替代原生目录对话框） ================= */

/// 数据根（容器内 = 挂载卷）：目录选择器只允许在卷内浏览（越界拒绝，可手输任意绝对路径）。
fn data_root() -> std::path::PathBuf {
    App::try_global()
        .map(|app| app.dir().app_dir.clone())
        .unwrap_or_else(|| std::path::PathBuf::from("/data"))
}

/// 选择器路径白名单：数据根之内（工作空间/知识库/记忆都在其中）。
fn picker_path_allowed(path: &std::path::Path) -> bool {
    lexical_normalize(path).starts_with(lexical_normalize(&data_root()))
}

/// `GET /api/v1/fs/list-dirs?path=`：单层列目录（仅目录，供选择器）。
/// 缺省起点 = 工作空间根（`<数据根>/workspace`）。返回 `{path, parent, root, dirs:[{name,path}]}`。
pub async fn list_dirs(
    State(_s): State<ServerState>,
    axum::extract::Query(q): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> R {
    let root = data_root();
    let raw = q.get("path").cloned().unwrap_or_default();
    let start = if raw.trim().is_empty() {
        // 缺省起点 = 会话工作区根（`<用户目录根>/workspace`，项目目录多在此）
        session_workspace_root()
            .filter(|p| p.is_dir())
            .unwrap_or_else(|| root.clone())
    } else {
        std::path::PathBuf::from(&raw)
    };
    let start = lexical_normalize(&start);
    if !picker_path_allowed(&start) {
        return Err(ApiError::bad_request("路径超出可浏览范围（数据根之外）"));
    }
    let mut dirs: Vec<Value> = Vec::new();
    let rd = std::fs::read_dir(&start).map_err(|e| ApiError::bad_request(format!("读取目录失败: {e}")))?;
    for entry in rd.flatten() {
        let Ok(ft) = entry.file_type() else { continue };
        if !ft.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue; // 隐藏目录不进选择器（.astorm 等内部目录）
        }
        dirs.push(json!({ "name": name, "path": entry.path().to_string_lossy() }));
    }
    dirs.sort_by(|a, b| {
        a.get("name").and_then(|v| v.as_str()).unwrap_or("")
            .cmp(b.get("name").and_then(|v| v.as_str()).unwrap_or(""))
    });
    let parent = start.parent().filter(|p| picker_path_allowed(p)).map(|p| p.to_string_lossy().to_string());
    Ok(ok(json!({
        "path": start.to_string_lossy(),
        "parent": parent,
        "root": root.to_string_lossy(),
        "dirs": dirs,
    })))
}

/// `POST /api/v1/fs/mkdir {parent, name}`：在选择器里新建文件夹（数据根内，单层名）。
pub async fn make_dir(
    State(_s): State<ServerState>,
    Json(body): Json<Value>,
) -> R {
    let parent = body.get("parent").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let name = body.get("name").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    if name.is_empty() || name.contains('/') || name.contains('\\') || name.starts_with('.') {
        return Err(ApiError::bad_request("文件夹名不合法"));
    }
    let p = lexical_normalize(&std::path::PathBuf::from(&parent)).join(&name);
    if !picker_path_allowed(&p) {
        return Err(ApiError::bad_request("路径超出可浏览范围（数据根之外）"));
    }
    std::fs::create_dir_all(&p).map_err(|e| ApiError::bad_request(format!("新建文件夹失败: {e}")))?;
    Ok(ok(json!({ "path": p.to_string_lossy() })))
}

/* ================= 附件 ================= */

/// 上传体：multipart 太重，直接收 JSON（base64）——前端 File.arrayBuffer → base64。
#[derive(serde::Deserialize)]
pub struct UploadBody {
    pub name: String,
    #[serde(default)]
    pub base64: String,
}

/// 上传上限（与桌面 attachmentFiles.ts:24-30 对齐：单图 20MB）。
const UPLOAD_MAX_BYTES: usize = 20 * 1024 * 1024;
/// staging 清扫周期与文件 TTL（桌面 24h）。
const STAGING_TTL: std::time::Duration = std::time::Duration::from_secs(24 * 3600);

/// `POST /api/v1/attachments`：上传附件到服务端 staging，返回 MessageAttachment 形状。
pub async fn upload_attachment(
    State(state): State<ServerState>,
    Json(body): Json<UploadBody>,
) -> R {
    // base64 体积先查（解码前拒绝，避免大包打爆内存）
    if body.base64.len() > UPLOAD_MAX_BYTES / 3 * 4 + 4 {
        return Err(ApiError::bad_request("附件超过 20MB 上限"));
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(body.base64.as_bytes())
        .map_err(|e| ApiError::bad_request(format!("base64 解码失败: {e}")))?;
    if bytes.len() > UPLOAD_MAX_BYTES {
        return Err(ApiError::bad_request("附件超过 20MB 上限"));
    }
    sweep_staging(state.local.attachment_dir());;
    // 文件名清洗：只保留文件名部分 + 白名单字符
    let safe_name: String = body
        .name
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("file")
        .chars()
        .filter(|c| c.is_alphanumeric() || "-_.".contains(*c))
        .collect();
    let safe_name = if safe_name.is_empty() { "file".to_string() } else { safe_name };
    let dir = state.local.attachment_dir();
    // 唯一化：时间戳前缀
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let target = dir.join(format!("{ts}-{safe_name}"));
    std::fs::write(&target, &bytes).map_err(|e| ApiError::internal(format!("staging 写入失败: {e}")))?;
    let mime = sniff_mime(&bytes)
        .map(str::to_string)
        .unwrap_or_else(|| {
            let ext = safe_name.rsplit('.').next().unwrap_or("").to_lowercase();
            mime_of(&ext).to_string()
        });
    Ok(ok(json!({
        "path": target.to_string_lossy(),
        "name": safe_name,
        "size": bytes.len(),
        "mime": mime,
    })))
}

/// 附件路径 allow-list（与桌面一致）：staging 目录 或 会话工作区 `.astorm/attachment/`。
/// `path` 必须先做**词法规范化**（解析 `.`/`..`）——`Path::starts_with` 是按组件比较，
/// 不解析相对段，`staging/../..` 这类路径会绕过前缀检查造成任意文件读（桌面在
/// 检查前同样 normalize，attachmentFiles.ts:271）。
fn attachment_allowed(path: &std::path::Path, staging: &std::path::Path) -> bool {
    let norm = lexical_normalize(path);
    let norm_str = norm.to_string_lossy().replace('\\', "/");
    if norm_str.contains("/.astorm/attachment/") {
        return true;
    }
    norm.starts_with(lexical_normalize(staging))
}

/// `GET /api/v1/attachment/file/*path`：附件直读（web 版 astorm-attachment://）。
pub async fn serve_attachment(
    State(state): State<ServerState>,
    Path(path): Path<String>,
) -> Response {
    // path 为各段 encodeURIComponent 后的绝对路径（与桌面 toUrl 同款编码）；
    // 渲染层 toUrl 的 filter(Boolean) 会丢掉开头的空段 → 重建绝对路径的分隔符
    // （桌面 attachmentFiles.ts:268-270 同款处理，否则 staging 前缀检查永远失败）
    let decoded = path
        .split('/')
        .filter(|s| !s.is_empty())
        .map(|seg| percent_decode(seg))
        .collect::<Vec<_>>()
        .join("/");
    let mut target = std::path::PathBuf::from(&decoded);
    if !target.is_absolute() {
        target = std::path::Path::new("/").join(target);
    }
    let staging = state.local.attachment_dir();
    if !attachment_allowed(&target, &staging) {
        return (StatusCode::FORBIDDEN, "attachment path not allowed").into_response();
    }
    match std::fs::read(&target) {
        Ok(bytes) => {
            let mime = sniff_mime(&bytes)
                .map(str::to_string)
                .unwrap_or_else(|| "application/octet-stream".to_string());
            ([(header::CONTENT_TYPE, mime)], bytes).into_response()
        }
        Err(_) => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

/* ================= 知识库文件 ================= */

/// `GET /api/v1/kb/file/*path`：KB 文件直读（web 版 astorm-kb://doc/<relpath>；
/// 路径为知识库根目录下的相对路径）。
pub async fn serve_kb_file(Path(path): Path<String>) -> Response {
    let Some(root) = App::try_global().map(|app| app.knowledge().root()) else {
        return (StatusCode::NOT_FOUND, "knowledge base unavailable").into_response();
    };
    let segs: Vec<std::path::PathBuf> = path
        .split('/')
        .filter(|s| !s.is_empty())
        .map(percent_decode)
        .map(std::path::PathBuf::from)
        .collect();
    if segs.is_empty() {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    }
    let mut target = root.clone();
    for seg in &segs {
        target = target.join(seg);
    }
    let canon = target.canonicalize().unwrap_or(target.clone());
    // 目标 canonicalize 后可能是 `\\?\C:\...`（Windows）或 symlink 解析后的真实路径；
    // root 也必须用同一形态比较，否则所有请求都会被判 403。
    let root_canon = root.canonicalize().unwrap_or_else(|_| root.clone());
    if !canon.starts_with(&root_canon) {
        return (StatusCode::FORBIDDEN, "forbidden").into_response();
    }
    match std::fs::read(&canon) {
        Ok(bytes) => {
            let ext = canon
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_lowercase();
            (
                [(
                    header::CONTENT_TYPE,
                    mime_of(&ext),
                )],
                bytes,
            )
                .into_response()
        }
        Err(_) => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

/* ================= office 预览 ================= */

fn office_bin(kind: &str) -> Option<std::path::PathBuf> {
    let dir = std::env::var("ASTORM_OFFICE_BIN_DIR").ok()?;
    let exe = if cfg!(windows) { format!("json2{kind}.exe") } else { format!("json2{kind}") };
    let p = std::path::PathBuf::from(dir).join(exe);
    p.is_file().then_some(p)
}

/// `POST /api/v1/office/preview`：`unpack --inline` 单文件 JSON（带缓存，同桌面）。
pub async fn office_preview(
    State(state): State<ServerState>,
    Json(body): Json<Value>,
) -> R {
    let path = body.get("path").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let kind = match path.rsplit('.').next().unwrap_or("").to_lowercase().as_str() {
        "pdf" => "pdf",
        "docx" | "docm" => "docx",
        "xlsx" | "xlsm" => "xlsx",
        "pptx" | "pptm" | "ppsx" | "pps" => "pptx",
        _ => {
            return Ok(ok(json!({
                "ok": false,
                "message": "该格式不支持 office 高保真预览(支持 docx/docm、xlsx/xlsm、pptx/pptm/ppsx/pps、pdf)"
            })))
        }
    };
    let Some(bin) = office_bin(kind) else {
        return Ok(ok(json!({
            "ok": false,
            "message": "未找到 office CLI（镜像需放入 Linux 版五件套，见 docker/office-bin/README）"
        })));
    };
    // 缓存：path+mtime+size 哈希
    let meta = std::fs::metadata(&path).map_err(|e| ApiError::bad_request(format!("读取文件失败: {e}")))?;
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let key = format!("{path:?}-{mtime}-{}", meta.len());
    let cache_dir = App::try_global()
        .map(|app| app.dir().data_dir.join("office-preview"))
        .unwrap_or_else(std::env::temp_dir);
    let _ = std::fs::create_dir_all(&cache_dir);
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    key.hash(&mut hasher);
    let hash = format!("{:016x}", hasher.finish());
    let cache_file = cache_dir.join(format!("{hash}.json"));
    if let Ok(text) = std::fs::read_to_string(&cache_file) {
        if let Ok(data) = serde_json::from_str::<Value>(&text) {
            return Ok(ok(json!({ "ok": true, "kind": kind, "data": data })));
        }
    }
    // 解包到**唯一**临时目录：同一文件的并发预览不能共用/互删工作目录。
    let unique = format!(
        "office-preview-{hash}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    );
    let work = std::env::temp_dir().join(unique);
    let _ = std::fs::create_dir_all(&work);
    let out = work.join("inline.json");
    let result = tokio::process::Command::new(&bin)
        .args(["unpack", &path, "--inline", "-o"])
        .arg(&out)
        .output()
        .await;
    let resp = match result {
        Ok(o) if o.status.success() && out.is_file() => {
            match std::fs::read_to_string(&out).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()) {
                Some(data) => {
                    let _ = std::fs::write(&cache_file, data.to_string());
                    json!({ "ok": true, "kind": kind, "data": data })
                }
                None => json!({ "ok": false, "message": "解包产物解析失败" }),
            }
        }
        Ok(o) => json!({
            "ok": false,
            "message": format!("解包失败: {}", String::from_utf8_lossy(&o.stderr)),
        }),
        Err(e) => json!({ "ok": false, "message": format!("启动 CLI 失败: {e}") }),
    };
    let _ = std::fs::remove_dir_all(&work);
    Ok(ok(resp))
}

/// staging 清扫：删除超过 TTL 的上传副本（桌面 24h TTL 同款；仅在处理上传时触发，
/// 无后台任务——单用户量级足够）。
fn sweep_staging(dir: std::path::PathBuf) {
    let Ok(entries) = std::fs::read_dir(&dir) else { return };
    let now = std::time::SystemTime::now();
    for e in entries.flatten() {
        let Ok(meta) = e.metadata() else { continue };
        if !meta.is_file() {
            continue;
        }
        if let Ok(m) = meta.modified() {
            if now.duration_since(m).map(|d| d > STAGING_TTL).unwrap_or(false) {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
}

/// FNV-1a 64 位（跨进程稳定，无需外部 crate）。
fn fnv1a(data: &[u8]) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in data {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}")
}

/// 最简百分号解码（ encodeURIComponent 产物；+ 不转空格，与 JS 一致）。
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
            if let Some(v) = hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `GET /web-bridge.js`：无缓存返回桥脚本（挂在静态托管之前；迭代期浏览器缓存
/// 旧桥会让新旧契约混跑，实测踩坑）。
pub async fn web_bridge_js(State(state): State<ServerState>) -> Response {
    let Some(dir) = &state.web_dir else {
        return (StatusCode::NOT_FOUND, "web not mounted").into_response();
    };
    match tokio::fs::read(dir.join("web-bridge.js")).await {
        Ok(bytes) => (
            [
                (header::CONTENT_TYPE, "text/javascript; charset=utf-8"),
                (header::CACHE_CONTROL, "no-cache, no-store, must-revalidate"),
            ],
            bytes,
        )
            .into_response(),
        Err(_) => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

/* ================= 供应商配置（providers.json，明文 + ${ENV} 占位） ================= */

/// `GET /api/v1/providers`：`{providers: [...]}`（与桌面 providers:list 同形状）。
pub async fn providers_list(State(state): State<ServerState>) -> R {
    // providers.json（Web 设置页写的真源）优先；空/缺失回退运行配置（env 种子），
    // 否则登录前的一次空 persist 会让模型页永远空白（实测踩坑）
    let cfg = state.app.config().clone();
    Ok(ok(local::effective_providers(&state.local, &cfg)))
}

/// `POST /api/v1/providers/sync`：整体保存 + 重建 AppConfig 热更。
pub async fn providers_sync(
    State(state): State<ServerState>,
    Json(body): Json<Value>,
) -> R {
    let providers = body.get("providers").cloned().unwrap_or(json!([]));
    state.local.write("providers.json", &json!({ "providers": providers }))?;
    local::sync_config_to_core(&state.local)?;
    Ok(ok(json!({ "ok": true })))
}

/// `POST /api/v1/providers/test`：文本+图片能力探测（chat/completions 图文请求）。
pub async fn providers_test(
    State(state): State<ServerState>,
    Json(body): Json<Value>,
) -> R {
    let name = body.get("name").and_then(|v| v.as_str()).unwrap_or("");
    let model = body.get("model").and_then(|v| v.as_str()).unwrap_or("");
    // 供应商来源 = 已保存的 providers.json；UI DTO 的 models 是 {name,order}，
    // 必须先走与 sync 相同的转换补 id（AppConfig 需要 {id,name}）——否则反序列化
    // 失败 → 一律误报「供应商 X 不存在」（Web 设置页「检测」按钮实测踩坑）。
    let mut saved = state.local.read("providers.json");
    local::expand_providers(&mut saved);
    let mut list: Vec<astorm_core::app::config::LlmProvider> = de_providers(&saved);
    // 允许测试尚未保存的供应商（body 内联定义；apiKey 缺省用已存同名的）
    if let Some(p) = body.get("provider") {
        let mut wrapped = json!({ "providers": [p.clone()] });
        local::expand_providers(&mut wrapped);
        let one = wrapped
            .get("providers")
            .and_then(|v| v.as_array())
            .and_then(|a| a.first())
            .cloned()
            .unwrap_or(Value::Null);
        if let Ok(mut cfg) = serde_json::from_value::<astorm_core::app::config::LlmProvider>(one) {
            if cfg.api_key.is_empty() {
                cfg.api_key = list
                    .iter()
                    .find(|x| x.name == name)
                    .map(|x| x.api_key.clone())
                    .unwrap_or_default();
            }
            list.insert(0, cfg);
        }
    }
    let Some(p) = list.iter().find(|p| p.name == name) else {
        return Ok(ok(json!({ "ok": false, "message": format!("供应商 {name} 不存在") })));
    };
    let model = if model.is_empty() {
        p.models.first().map(|m| m.name.clone()).unwrap_or_default()
    } else {
        model.to_string()
    };
    let url = format!("{}/chat/completions", p.base_url.trim_end_matches('/'));
    let client = reqwest_client();
    let image = format!("data:image/png;base64,{PROBE_IMAGE_BASE64}");
    let resp = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", p.api_key))
        .json(&json!({
            "model": model,
            "messages": [{
                "role": "user",
                "content": [
                    { "type": "text", "text": PROBE_PROMPT },
                    { "type": "image_url", "image_url": { "url": image } }
                ]
            }],
            "max_tokens": 48,
        }))
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await;
    match resp {
        Ok(r) => {
            let status = r.status();
            let text = r.text().await.unwrap_or_default();
            if status.is_success() {
                let answer = serde_json::from_str::<Value>(&text)
                    .ok()
                    .map(|v| chat_completion_text(&v))
                    .unwrap_or_default()
                    .to_lowercase();
                if answer.contains("pineapple") && answer.contains("739") {
                    Ok(ok(json!({ "ok": true, "message": "文本与图片识别正常" })))
                } else {
                    Ok(ok(json!({ "ok": false, "message": "vision_unsupported" })))
                }
            } else {
                let lower = text.to_lowercase();
                let unsupported = [400u16, 415, 422].contains(&status.as_u16())
                    && (lower.contains("image")
                        || lower.contains("vision")
                        || lower.contains("unsupported")
                        || lower.contains("content"));
                Ok(ok(if unsupported {
                    json!({ "ok": false, "message": "vision_unsupported" })
                } else {
                    json!({ "ok": false, "message": format!("HTTP {}: {}", status, text) })
                }))
            }
        }
        Err(e) => Ok(ok(json!({ "ok": false, "message": format!("请求失败: {e}") }))),
    }
}

/// 从 chat/completions 响应取助手文本（字符串或 content parts）。
fn chat_completion_text(v: &Value) -> String {
    match v.pointer("/choices/0/message/content") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(parts)) => parts
            .iter()
            .filter_map(|p| p.get("text").and_then(|t| t.as_str()))
            .collect::<Vec<_>>()
            .join(" "),
        _ => String::new(),
    }
}

/// providers.json → AppConfig 的 LlmProvider 列表（复用 local::expand_providers 的映射）。
fn de_providers(v: &Value) -> Vec<astorm_core::app::config::LlmProvider> {
    serde_json::from_value(v.clone())
        .map(|mut wrapper: astorm_core::app::config::LlmConfig| {
            astorm_core::app::config::LlmConfig::default();
            wrapper.providers
        })
        .unwrap_or_default()
}

/// 极简 reqwest 客户端（rustls，与 workspace 特性一致）。
fn reqwest_client() -> reqwest::Client {
    reqwest::Client::builder()
        .build()
        .expect("构建 HTTP 客户端失败")
}
