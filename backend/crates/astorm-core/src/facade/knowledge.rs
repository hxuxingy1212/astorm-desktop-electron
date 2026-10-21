//! 知识库门面：真实文件系统 CRUD + 元数据清单（`.manifest.json`）。
//!
//! 存储布局（`<app_dir>/knowledge/`）：
//! - 文件夹 = 目录；文档 = 文件（缺扩展名自动补 `.md`）
//! - 展示元数据（图标/颜色/标签）集中在根目录 `.manifest.json`，
//!   以相对路径（`/` 分隔）为键；重命名/删除时同步迁移/清理（含后代）。

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use astorm_common::knowledge::{KbCreateRequest, KbItem, KbMetaUpdate, Knowledge};
use serde::{Deserialize, Serialize};

use crate::app::paths::AppDir;

/// 元数据清单文件名（仅存在于知识库根；遍历时跳过）。
const MANIFEST_FILE: &str = ".manifest.json";
/// 文档默认扩展名（无扩展名的新建文档自动补全）。
const DEFAULT_DOC_EXT: &str = "md";
/// 单文档读取上限（1MB，与文件门面一致）。
const MAX_CONTENT_BYTES: u64 = 1024 * 1024;
/// 预览字节读取上限（50MB;PDF/Office/媒体等二进制文档）。
const MAX_PREVIEW_BYTES: u64 = 50 * 1024 * 1024;
/// 颜色默认 key。
const DEFAULT_COLOR: &str = "blue";
/// 导入文档的默认颜色 key（与文件夹的蓝色区分）。
const IMPORT_DOC_COLOR: &str = "teal";

/// 展示元数据清单（相对路径 → 元数据；BTreeMap 保证落盘稳定排序）。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct Manifest {
    #[serde(default = "default_version")]
    version: u32,
    #[serde(default)]
    items: BTreeMap<String, ManifestItem>,
}

fn default_version() -> u32 {
    1
}

/// 单条目元数据。
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ManifestItem {
    #[serde(default)]
    color: String,
    #[serde(default)]
    tags: Vec<String>,
}

/// 知识库门面（根 = 用户目录 knowledge/,**每次操作现算**——跟随系统设置,改动即时生效;
/// 清单即读即写,无缓存一致性负担）。
pub struct KnowledgeImpl;

impl KnowledgeImpl {
    /// 构建门面（零状态;根目录在首次 list/create 时按需创建并播种演示树）。
    pub fn new() -> Self {
        Self
    }

    /// 当前知识库根目录：`<用户目录>/knowledge`（用户目录 = `AppConfig.workspace`,默认 `~/astorm`）。
    pub fn root(&self) -> PathBuf {
        crate::app::App::try_global()
            .map(|app| AppDir::new(app.config()).knowledge_dir)
            .unwrap_or_else(|| {
                dirs::home_dir()
                    .unwrap_or_default()
                    .join("astorm")
                    .join("knowledge")
            })
    }

    /// 确保根目录存在;首次创建时写入演示树（真实文件 + 清单）。
    fn ensure_root(&self) -> Result<PathBuf, String> {
        let root = self.root();
        if !root.exists() {
            fs::create_dir_all(&root).map_err(|e| format!("创建知识库根目录失败({}): {e}", root.display()))?;
            self.bootstrap_demo(&root);
        }
        Ok(root)
    }

    // ---------- 内部工具 ----------

    /// 相对路径归一：反斜杠 → 正斜杠、去空白、拒绝空段 / `.` / `..`。
    fn normalize_rel(rel: &str) -> Result<String, String> {
        let trimmed = rel.trim().trim_start_matches(['/', '\\']);
        if trimmed.is_empty() {
            return Ok(String::new());
        }
        let mut out: Vec<String> = Vec::new();
        for seg in trimmed.split(['/', '\\']) {
            let seg = seg.trim();
            if seg.is_empty() || seg == "." {
                continue;
            }
            if seg == ".." {
                return Err("路径不允许包含 `..`".into());
            }
            out.push(seg.to_string());
        }
        Ok(out.join("/"))
    }

    /// 校验新建/重命名的名称（禁路径分隔符、隐藏名、控制字符与保留名）。
    fn validate_name(name: &str) -> Result<(), String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("名称不能为空".into());
        }
        if name.contains('/') || name.contains('\\') {
            return Err("名称不能包含路径分隔符".into());
        }
        if name.starts_with('.') {
            return Err("名称不能以点开头".into());
        }
        if name == MANIFEST_FILE {
            return Err("名称与系统清单文件冲突".into());
        }
        if name.chars().any(|c| c.is_control()) {
            return Err("名称不能包含控制字符".into());
        }
        Ok(())
    }

    /// 相对路径 → 绝对路径（不校验存在性）。
    fn abs_of(&self, rel: &str) -> PathBuf {
        let mut p = self.root();
        for seg in rel.split('/') {
            if !seg.is_empty() {
                p.push(seg);
            }
        }
        p
    }

    /// 阻断知识库内部符号链接/junction 把读写带到根目录之外。
    fn ensure_inside_root(&self, abs: &std::path::Path) -> Result<(), String> {
        if crate::tool::policy::path_in_workspace(&self.root(), abs) {
            Ok(())
        } else {
            Err("路径越出知识库根目录（符号链接/junction）".into())
        }
    }

    /// 读取清单（缺失/损坏 → 空清单）。
    fn load_manifest(&self) -> Manifest {
        match fs::read(self.root().join(MANIFEST_FILE)) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_default(),
            Err(_) => Manifest::default(),
        }
    }

    /// 写入清单。
    fn save_manifest(&self, m: &Manifest) -> Result<(), String> {
        let bytes = serde_json::to_vec_pretty(m).map_err(|e| format!("序列化清单失败: {e}"))?;
        astorm_common::fsx::write_atomic(&self.root().join(MANIFEST_FILE), &bytes)
            .map_err(|e| format!("写入清单失败: {e}"))
    }

    /// 收集 rel 自身与全部后代的清单键（删除清理用）。
    fn descendant_keys(&self, rel: &str) -> Vec<String> {
        let mut keys = vec![rel.to_string()];
        collect_rel_children(&self.abs_of(rel), rel, &mut keys);
        keys
    }

    /// 首次初始化：写入演示树（真实文件夹 + Markdown 文档 + 元数据清单）。
    fn bootstrap_demo(&self, root: &Path) {
        let root = root.to_path_buf();
        let mut manifest = self.load_manifest();
        // 注册元数据（BTreeMap 持有,最后统一落盘）
        let mut meta = |items: &mut BTreeMap<String, ManifestItem>, rel: &str, color: &str, tags: &[&str]| {
            items.insert(
                rel.to_string(),
                ManifestItem {
                    color: color.into(),
                    tags: tags.iter().map(|s| s.to_string()).collect(),
                },
            );
        };

        // ── 文件夹（真实目录）──
        for (rel, color, tags) in [
            ("项目文档", "blue", vec!["项目", "进行中"]),
            ("项目文档/需求文档", "cyan", vec![]),
            ("项目文档/技术方案", "purple", vec![]),
            ("项目文档/技术方案/前端架构", "cyan", vec![]),
            ("项目文档/技术方案/后端架构", "indigo", vec![]),
            ("学习笔记", "orange", vec!["学习"]),
            ("学习笔记/前端", "blue", vec![]),
            ("学习笔记/前端/Vue3", "teal", vec![]),
            ("学习笔记/算法", "purple", vec![]),
            ("研究资料", "purple", vec!["研究", "收藏"]),
            ("研究资料/论文收藏", "indigo", vec![]),
            ("研究资料/论文收藏/Transformer 系列", "blue", vec![]),
            ("灵感收集", "pink", vec!["灵感"]),
            ("灵感收集/产品点子", "yellow", vec![]),
            ("灵感收集/产品点子/AI 工具", "purple", vec![]),
        ] {
            meta(&mut manifest.items, rel, color, &tags);
            if let Err(e) = fs::create_dir_all(self.abs_of(rel)) {
                tracing::warn!("[knowledge] 创建演示目录失败 {rel}: {e}");
            }
        }

        // ── 文档（真实 Markdown 文件）──
        for (rel, color, tags, body) in [
            ("快速开始.md", "gray", vec!["指南"], "本知识库为真实文件系统：文件夹即目录、文档即 Markdown 文件，可直接在文件管理器中查看编辑。"),
            ("会议纪要.md", "teal", vec!["会议"], "每周例会纪要集中存放于此。"),
            ("项目文档/需求文档/PRD 摘要.md", "gray", vec![], "产品需求摘要。"),
            ("项目文档/需求文档/用户调研.md", "teal", vec![], "用户访谈与问卷结论。"),
            ("项目文档/技术方案/架构总览.md", "gray", vec![], "整体架构说明。"),
            ("项目文档/技术方案/API 契约.md", "blue", vec![], "前后端接口契约。"),
            ("项目文档/技术方案/前端架构/组件设计规范.md", "cyan", vec![], "组件拆分与命名规范。"),
            ("项目文档/技术方案/前端架构/状态管理选型.md", "gray", vec![], "Pinia / 组合式函数对比。"),
            ("项目文档/技术方案/后端架构/服务拆分说明.md", "gray", vec![], "crate 划分与依赖方向。"),
            ("项目文档/技术方案/后端架构/网关与鉴权.md", "green", vec![], "IPC 层鉴权策略。"),
            ("项目文档/发布计划.md", "orange", vec![], "版本发布节奏。"),
            ("学习笔记/前端/Vue3/响应式原理.md", "teal", vec![], "ref / reactive / effect。"),
            ("学习笔记/前端/Vue3/组合式函数模式.md", "blue", vec![], "useXxx 组合式函数设计。"),
            ("学习笔记/前端/TypeScript 技巧.md", "blue", vec![], "类型体操与工程实践。"),
            ("学习笔记/算法/动态规划入门.md", "gray", vec![], "状态定义与转移方程。"),
            ("研究资料/论文收藏/Transformer 系列/Self-Attention 详解.md", "gray", vec![], "注意力机制推导。"),
            ("研究资料/论文收藏/Transformer 系列/位置编码对比.md", "teal", vec![], "绝对/相对/旋转位置编码。"),
            ("研究资料/数据集清单.md", "teal", vec![], "常用数据集索引。"),
            ("灵感收集/产品点子/AI 工具/语音转写助手.md", "gray", vec![], "点子草稿。"),
            ("灵感收集/产品点子/AI 工具/会议摘要机器人.md", "purple", vec![], "点子草稿。"),
            ("灵感收集/设计灵感.md", "cyan", vec![], "视觉参考收集。"),
        ] {
            meta(&mut manifest.items, rel, color, &tags);
            let abs = self.abs_of(rel);
            if let Some(parent) = abs.parent() {
                let _ = fs::create_dir_all(parent);
            }
            if !abs.exists() {
                let title = rel.rsplit('/').next().unwrap_or(rel).trim_end_matches(".md");
                let _ = astorm_common::fsx::write_atomic(
                    &abs,
                    format!("# {title}\n\n{body}\n").as_bytes(),
                );
            }
        }

        if let Err(e) = self.save_manifest(&manifest) {
            tracing::warn!("[knowledge] 保存演示清单失败: {e}");
        }
    }
}

impl Knowledge for KnowledgeImpl {
    fn list(&self) -> Result<Vec<KbItem>, String> {
        let root = self.ensure_root()?;
        let manifest = self.load_manifest();
        let mut items: Vec<KbItem> = Vec::new();
        walk_items(&root, "", &manifest, &mut items)?;
        // 文件夹在前，再按名称排序（与文件门面一致）
        items.sort_by(|a, b| match (a.node_type.as_str(), b.node_type.as_str()) {
            ("folder", "document") => std::cmp::Ordering::Less,
            ("document", "folder") => std::cmp::Ordering::Greater,
            _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
        });
        Ok(items)
    }

    fn create(&self, request: KbCreateRequest) -> Result<KbItem, String> {
        let parent = Self::normalize_rel(&request.parent)?;
        let name = request.name.trim();
        Self::validate_name(name)?;
        if request.kind != "folder" && request.kind != "document" {
            return Err(format!("未知条目类型: {}", request.kind));
        }
        let is_folder = request.kind == "folder";

        // 文档缺扩展名时自动补 .md
        let fs_name = if !is_folder && Path::new(name).extension().is_none() {
            format!("{name}.{DEFAULT_DOC_EXT}")
        } else {
            name.to_string()
        };
        let rel = if parent.is_empty() {
            fs_name.clone()
        } else {
            format!("{parent}/{fs_name}")
        };
        let abs = self.abs_of(&rel);
        self.ensure_inside_root(&abs)?;
        if abs.exists() {
            return Err(format!("条目已存在: {rel}"));
        }

        if is_folder {
            fs::create_dir_all(&abs).map_err(|e| format!("创建文件夹失败: {e}"))?;
        } else {
            if let Some(parent_abs) = abs.parent() {
                fs::create_dir_all(parent_abs).map_err(|e| format!("创建父目录失败: {e}"))?;
            }
            let content = request
                .content
                .clone()
                .unwrap_or_else(|| format!("# {name}\n\n"));
            astorm_common::fsx::write_atomic(&abs, content.as_bytes())
                .map_err(|e| format!("创建文档失败: {e}"))?;
        }

        let mut manifest = self.load_manifest();
        manifest.items.insert(
            rel.clone(),
            ManifestItem {
                color: request.color.clone().unwrap_or_else(|| DEFAULT_COLOR.into()),
                tags: request.tags.clone().unwrap_or_default(),
            },
        );
        self.save_manifest(&manifest)?;
        build_item_from(&abs, &rel, &manifest)
    }

    fn rename(&self, path: String, new_name: String) -> Result<KbItem, String> {
        let rel = Self::normalize_rel(&path)?;
        if rel.is_empty() {
            return Err("不能重命名根目录".into());
        }
        Self::validate_name(&new_name)?;
        let abs = self.abs_of(&rel);
        self.ensure_inside_root(&abs)?;
        if !abs.exists() {
            return Err(format!("条目不存在: {rel}"));
        }
        let parent = match rel.rfind('/') {
            Some(idx) => rel[..idx].to_string(),
            None => String::new(),
        };
        let new_rel = if parent.is_empty() {
            new_name.clone()
        } else {
            format!("{parent}/{new_name}")
        };
        let new_abs = self.abs_of(&new_rel);
        self.ensure_inside_root(&new_abs)?;
        if new_abs.exists() {
            return Err(format!("目标已存在: {new_rel}"));
        }
        fs::rename(&abs, &new_abs).map_err(|e| format!("重命名失败: {e}"))?;

        // 清单迁移：先收集 (旧键, 新键, 值)，再统一删旧写新（文件夹含全部后代键）
        let mut manifest = self.load_manifest();
        let prefix = format!("{rel}/");
        let new_prefix = format!("{new_rel}/");
        let moves: Vec<(String, String, ManifestItem)> = manifest
            .items
            .iter()
            .filter(|(k, _)| k.as_str() == rel || k.starts_with(&prefix))
            .map(|(k, v)| {
                let new_key = if k.as_str() == rel {
                    new_rel.clone()
                } else {
                    format!("{new_prefix}{}", &k[prefix.len()..])
                };
                (k.clone(), new_key, v.clone())
            })
            .collect();
        for (old, _, _) in &moves {
            manifest.items.remove(old);
        }
        for (_, new, v) in &moves {
            manifest.items.insert(new.clone(), v.clone());
        }
        self.save_manifest(&manifest)?;
        build_item_from(&new_abs, &new_rel, &manifest)
    }

    fn delete(&self, path: String) -> Result<(), String> {
        let rel = Self::normalize_rel(&path)?;
        if rel.is_empty() {
            return Err("不能删除根目录".into());
        }
        let abs = self.abs_of(&rel);
        self.ensure_inside_root(&abs)?;
        if !abs.exists() {
            return Ok(()); // 幂等
        }
        if abs.is_dir() {
            fs::remove_dir_all(&abs).map_err(|e| format!("删除文件夹失败: {e}"))?;
        } else {
            fs::remove_file(&abs).map_err(|e| format!("删除文档失败: {e}"))?;
        }
        // 清理自身 + 后代元数据
        let mut manifest = self.load_manifest();
        for k in self.descendant_keys(&rel) {
            manifest.items.remove(&k);
        }
        self.save_manifest(&manifest)?;
        Ok(())
    }

    fn update_meta(&self, meta: KbMetaUpdate) -> Result<KbItem, String> {
        let rel = Self::normalize_rel(&meta.path)?;
        if rel.is_empty() {
            return Err("根目录不支持元数据".into());
        }
        let abs = self.abs_of(&rel);
        self.ensure_inside_root(&abs)?;
        if !abs.exists() {
            return Err(format!("条目不存在: {rel}"));
        }
        let mut manifest = self.load_manifest();
        let entry = manifest.items.entry(rel.clone()).or_insert_with(|| ManifestItem {
            color: String::new(),
            tags: Vec::new(),
        });
        if let Some(color) = meta.color.clone() {
            entry.color = color;
        }
        if let Some(tags) = meta.tags.clone() {
            entry.tags = tags;
        }
        self.save_manifest(&manifest)?;
        build_item_from(&abs, &rel, &manifest)
    }

    fn read(&self, path: String) -> Result<String, String> {
        let rel = Self::normalize_rel(&path)?;
        let abs = self.abs_of(&rel);
        self.ensure_inside_root(&abs)?;
        let meta = fs::metadata(&abs).map_err(|e| format!("读取元数据失败: {e}"))?;
        if !meta.is_file() {
            return Err("不是文档".into());
        }
        if meta.len() > MAX_CONTENT_BYTES {
            return Err("文档超过 1MB,暂不支持读取".into());
        }
        let bytes = fs::read(&abs).map_err(|e| format!("读取文档失败: {e}"))?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }

    fn read_bytes(&self, path: String) -> Result<Vec<u8>, String> {
        let rel = Self::normalize_rel(&path)?;
        let abs = self.abs_of(&rel);
        self.ensure_inside_root(&abs)?;
        let meta = fs::metadata(&abs).map_err(|e| format!("读取元数据失败: {e}"))?;
        if !meta.is_file() {
            return Err("不是文档".into());
        }
        if meta.len() > MAX_PREVIEW_BYTES {
            return Err("文档超过 50MB,暂不支持预览".into());
        }
        fs::read(&abs).map_err(|e| format!("读取文档失败: {e}"))
    }

    fn write(&self, path: String, content: String) -> Result<(), String> {
        let rel = Self::normalize_rel(&path)?;
        if rel.is_empty() {
            return Err("路径为空".into());
        }
        let abs = self.abs_of(&rel);
        self.ensure_inside_root(&abs)?;
        if !abs.is_file() {
            return Err(format!("目标不是文档: {rel}"));
        }
        astorm_common::fsx::write_atomic(&abs, content.as_bytes())
            .map_err(|e| format!("写入文档失败: {e}"))
    }

    fn import(&self, parent: String, sources: Vec<String>) -> Result<Vec<KbItem>, String> {
        let parent = Self::normalize_rel(&parent)?;
        let parent_abs = self.abs_of(&parent);
        self.ensure_inside_root(&parent_abs)?;
        if !parent_abs.is_dir() {
            return Err(format!("父目录不存在: {parent}"));
        }
        let mut manifest = self.load_manifest();
        let mut imported: Vec<KbItem> = Vec::new();
        for src in sources {
            let src = PathBuf::from(src.trim());
            if src.as_os_str().is_empty() {
                continue;
            }
            let Some(src_name) = src.file_name() else {
                continue;
            };
            let name = src_name.to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue; // 跳过隐藏文件
            }
            // 重名自动加序号:「名字 2.ext」「名字 3.ext」…
            let fs_name = free_name(&parent_abs, &name)?;
            let dest = parent_abs.join(&fs_name);
            let rel = if parent.is_empty() {
                fs_name.clone()
            } else {
                format!("{parent}/{fs_name}")
            };
            if src.is_dir() {
                copy_dir_recursive(&src, &dest)?;
                manifest.items.insert(
                    rel.clone(),
                    ManifestItem {
                        color: DEFAULT_COLOR.into(),
                        tags: Vec::new(),
                    },
                );
            } else if src.is_file() {
                fs::copy(&src, &dest).map_err(|e| format!("拷贝文件失败 {name}: {e}"))?;
                manifest.items.insert(
                    rel.clone(),
                    ManifestItem {
                        color: IMPORT_DOC_COLOR.into(),
                        tags: Vec::new(),
                    },
                );
            } else {
                continue;
            }
            imported.push(build_item_from(&dest, &rel, &manifest)?);
        }
        if !imported.is_empty() {
            self.save_manifest(&manifest)?;
        }
        Ok(imported)
    }
}

/// 目录内冲突时生成可用名:「name.ext」→「name 2.ext」→「name 3.ext」…
fn free_name(dir: &Path, name: &str) -> Result<String, String> {
    if !dir.join(name).exists() {
        return Ok(name.to_string());
    }
    let stem = Path::new(name)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| name.to_string());
    let ext = Path::new(name)
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    for n in 2..1000 {
        let candidate = format!("{stem} {n}{ext}");
        if !dir.join(&candidate).exists() {
            return Ok(candidate);
        }
    }
    Err(format!("名称冲突且无法自动去重: {name}"))
}

/// 递归拷贝目录（跳过点文件/点目录;无权限条目仅告警不中断）。
fn copy_dir_recursive(src: &Path, dest: &Path) -> Result<(), String> {
    fs::create_dir_all(dest).map_err(|e| format!("创建目录失败 {dest:?}: {e}"))?;
    let entries = fs::read_dir(src).map_err(|e| format!("读取目录失败 {src:?}: {e}"))?;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let from = entry.path();
        let to = dest.join(&name);
        let ft = entry.file_type().map_err(|e| format!("读取类型失败 {name}: {e}"))?;
        if ft.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else if ft.is_file() {
            fs::copy(&from, &to).map_err(|e| format!("拷贝文件失败 {name}: {e}"))?;
        }
    }
    Ok(())
}

/// 递归收集 abs 下所有条目的相对路径键（追加到 out；不含 rel 自身）。
fn collect_rel_children(abs: &Path, rel: &str, out: &mut Vec<String>) {
    let Ok(entries) = fs::read_dir(abs) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let child_rel = format!("{rel}/{name}");
        out.push(child_rel.clone());
        let child_abs = abs.join(&name);
        if child_abs.is_dir() {
            collect_rel_children(&child_abs, &child_rel, out);
        }
    }
}

/// 递归遍历构建条目列表（跳过点文件/点目录，含清单本身）。
fn walk_items(dir: &Path, prefix: &str, manifest: &Manifest, out: &mut Vec<KbItem>) -> Result<(), String> {
    let entries = fs::read_dir(dir).map_err(|e| format!("读取知识库目录失败: {e}"))?;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let abs = entry.path();
        let rel = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}/{name}")
        };
        if abs.is_dir() {
            walk_items(&abs, &rel, manifest, out)?;
        }
        out.push(build_item_from(abs.as_path(), &rel, manifest)?);
    }
    Ok(())
}

/// 由绝对路径 + 相对路径 + 清单构建条目视图。
fn build_item_from(abs: &Path, rel: &str, manifest: &Manifest) -> Result<KbItem, String> {
    let meta = fs::metadata(abs).map_err(|e| format!("读取元数据失败 {rel}: {e}"))?;
    let name = abs
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let is_dir = meta.is_dir();
    let (created_at, updated_at) = times_ms(&meta);
    let entry = manifest.items.get(rel);
    let parent_id = match rel.rfind('/') {
        Some(idx) => Some(rel[..idx].to_string()),
        None => None,
    };
    Ok(KbItem {
        id: rel.to_string(),
        node_type: if is_dir { "folder" } else { "document" }.into(),
        name,
        parent_id,
        color: entry
            .map(|e| e.color.clone())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| DEFAULT_COLOR.into()),
        tags: entry.map(|e| e.tags.clone()).unwrap_or_default(),
        size: if is_dir { 0 } else { meta.len() as i64 },
        created_at,
        updated_at,
        abs_path: abs.to_string_lossy().into_owned(),
    })
}

/// 文件时间 → (created_ms, updated_ms)；任一侧失败回退 0。
fn times_ms(meta: &fs::Metadata) -> (i64, i64) {
    let to_ms = |t: std::io::Result<std::time::SystemTime>| -> i64 {
        t.ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0)
    };
    (to_ms(meta.created()), to_ms(meta.modified()))
}
