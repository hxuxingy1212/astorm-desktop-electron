//! `okf-agent` CLI：仓库外的**独立**入口，把同一套 OKF 编译管道当命令行工具用。
//!
//! 应用（Electron）内走 lib + `astorm-core` 门面；本二进制用于：
//! - 无 GUI 环境下的批量编译 / 巡检（CI、脚本）
//! - 直接对 astorm 的会话目录跑真实数据的自测（`parse` 不调用 LLM）
//! - `--dry-run` 审阅将要发送的提示词（无凭据也能验证提示词构造）
//!
//! 应用约定（Bundle 与数据目录的默认位置）**只定义在这个二进制里**，lib 保持与产品无关：
//! Bundle 根默认 `<用户目录>/memory`，用户目录默认 `~/astorm`；
//! 会话目录按平台解析 astorm 数据目录（macOS `~/Library/Application Support/astorm/data/session`）。
//!
//! 凭据来源（后者优先）：`backend/.env` / 进程环境（`LLM_BASE_URL` `LLM_MODEL` `LLM_API_KEY`）
//! / 命令行参数。

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use astorm_memory::config::{MemoryCategory, MemoryConfig};
use astorm_memory::index::MemoryIndex;
use astorm_memory::llm::{ChatLlm, DryRunLlm, MemoryLlm, ScriptedLlm};
use astorm_memory::paths::BundlePaths;
use astorm_memory::pipeline::{self, CompileOptions, SessionInput};
use astorm_memory::types::{CategoryTable, CompileReport, ProgressEvent};
use astorm_memory::validate::validate_bundle;
use astorm_memory::writer;
use clap::{Parser, Subcommand};
use llm_provider::ChatClient;

/// CLI 内部统一用 `String` 错误（lib 亦如此），避免两套错误类型互相转换。
type Result<T> = std::result::Result<T, String>;

/// `bail!` 的 String 版本（用法与 anyhow 一致）。
macro_rules! bail {
    ($($arg:tt)*) => { return Err(format!($($arg)*)) };
}

/// `.ctx(|| "说明")`：给任意可显示错误加上下文并统一成 `String`。
trait Ctx<T> {
    fn ctx(self, msg: impl FnOnce() -> String) -> Result<T>;
}

impl<T, E: std::fmt::Display> Ctx<T> for std::result::Result<T, E> {
    fn ctx(self, msg: impl FnOnce() -> String) -> Result<T> {
        self.map_err(|e| format!("{}: {e}", msg()))
    }
}

impl<T> Ctx<T> for Option<T> {
    fn ctx(self, msg: impl FnOnce() -> String) -> Result<T> {
        self.ok_or_else(msg)
    }
}

#[derive(Parser)]
#[command(
    name = "okf-agent",
    version,
    about = "OKF 长期记忆编译管道：会话 JSONL → OKF v0.2 Bundle",
    long_about = None
)]
struct Cli {
    /// Bundle 根目录（默认 `<用户目录>/memory`，即 `~/astorm/memory`）
    ///
    /// 若目标目录已有内容但不含本管道标记（`.okf-bundle.json`），将**拒绝写入**以免破坏他人数据。
    #[arg(long, global = true, value_name = "DIR")]
    bundle: Option<PathBuf>,

    /// 用户目录（`<用户目录>/memory` 为 Bundle 根；默认 `~/astorm`）
    #[arg(long, global = true, value_name = "DIR")]
    user_dir: Option<PathBuf>,

    /// 会话目录（默认按平台解析 astorm 数据目录下的 `session/`）
    #[arg(long, global = true, value_name = "DIR")]
    sessions_dir: Option<PathBuf>,

    /// 全文索引目录（默认 `<bundle>/.okf-index`）
    #[arg(long, global = true, value_name = "DIR")]
    index_dir: Option<PathBuf>,

    /// 分类表覆盖：JSON 数组（`[{"id","dir","type","hint"}]`）
    #[arg(long, global = true, value_name = "JSON")]
    categories: Option<String>,

    /// 编译模型（默认 `LLM_MODEL`）
    #[arg(long, global = true)]
    model: Option<String>,

    /// 供应商名（仅用于日志显示；CLI 只认 base-url + api-key）
    #[arg(long, global = true)]
    provider: Option<String>,

    /// LLM API 基地址（默认 `LLM_BASE_URL`）
    #[arg(long, global = true, value_name = "URL")]
    base_url: Option<String>,

    /// LLM API 密钥（默认 `LLM_API_KEY`）
    #[arg(long, global = true, value_name = "KEY")]
    api_key: Option<String>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// 初始化 Bundle 骨架（创建 raw/ 与 wiki/ 分类目录）并打印解析出的路径
    Init,
    /// 仅归一化：会话 JSONL → `raw/sessions/*.md`（**不调用 LLM**）
    Parse {
        /// 忽略增量状态，重新处理全部会话
        #[arg(long)]
        force: bool,
    },
    /// 完整管道：归一化 → 提取 → 编译 → 合并 → 写入
    Compile {
        /// 忽略增量状态，强制重编译
        #[arg(long)]
        force: bool,
        /// 不调用 LLM：只跑确定性阶段并打印将要发送的提示词
        #[arg(long)]
        dry_run: bool,
    },
    /// 增量更新（等同 `compile`，语义上只处理指纹变化的会话）
    Update {
        #[arg(long)]
        force: bool,
        #[arg(long)]
        dry_run: bool,
    },
    /// 依据 `wiki/` 重建全文索引（索引是可重建的派生数据）
    Reindex,
    /// 列出概念
    List {
        /// 仅列出该 OKF `type` 的概念
        #[arg(long, value_name = "TYPE")]
        r#type: Option<String>,
        /// 仅列出该分类目录下的概念
        #[arg(long, value_name = "DIR")]
        dir: Option<String>,
    },
    /// 打印概念全文（含 frontmatter 原文）
    Show {
        /// Concept ID（如 `decisions/use-jwt`）
        concept_id: String,
    },
    /// 全文检索概念（BM25 + 中文分词）
    Search {
        query: String,
        /// 返回条数上限
        #[arg(long, default_value_t = 10)]
        limit: usize,
    },
    /// 校验 Bundle 的 OKF 合规性
    Validate,
    /// 打印变更历史（最新在前）
    Log {
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// 列出归一化的来源会话
    Raw,
    /// 状态概览：概念数 / 待编译会话 / 上次编译时间
    Status,
    /// 离线自检：用内建脚本 LLM 跑一遍完整管道（不联网）
    ///
    /// 目的是在没有凭据的环境下验证「解析 → 提取 → 编译 → 合并 → 写入 → 校验」全链路。
    SelfTest {
        /// 自检用的临时 Bundle 目录（默认系统临时目录下的随机目录）
        #[arg(long, value_name = "DIR")]
        scratch: Option<PathBuf>,
    },
}

#[tokio::main]
async fn main() -> ExitCode {
    // 与仓库既有约定一致：优先读 backend/.env（缺失则忽略）
    let _ = dotenvy::dotenv();
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("错误: {e}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<()> {
    let cli = Cli::parse();
    let bundle_root = resolve_bundle_root(&cli)?;
    let paths = BundlePaths::new(&bundle_root);
    let cfg = build_config(&cli)?;
    let categories = CategoryTable::new(cfg.categories.clone());

    match &cli.command {
        Command::Init => {
            paths
                .ensure_dirs(&categories.dirs())
                .ctx(|| format!("初始化 Bundle 失败: {}", paths.root().display()))?;
            print_paths(&cli, &paths);
            println!("\n分类目录:");
            for dir in categories.dirs() {
                println!("  {dir}/");
            }
            println!("\n分类表（{} 条）:", categories.all().len());
            for c in categories.all() {
                println!(
                    "  {:<14} type={:<13} dir={:<14} {}",
                    c.id,
                    c.type_name,
                    if c.dir.is_empty() { "(wiki 根)" } else { c.dir.as_str() },
                    c.hint
                );
            }
            println!("\n已初始化。下一步：okf-agent parse（不调用 LLM）或 okf-agent compile");
            Ok(())
        }

        Command::Parse { force } => {
            let sessions = load_sessions(&cli)?;
            let pending = pipeline::count_pending(&paths, &sessions);
            println!("会话目录: {}", sessions_dir(&cli)?.display());
            println!("扫描到 {} 个会话（待编译 {pending}）", sessions.len());
            let (processed, skipped, warnings) = pipeline::parse_sessions(&paths, &cfg, &sessions, *force)?;
            for w in &warnings {
                println!("  告警: {w}");
            }
            println!("归一化完成：处理 {processed} / 跳过 {skipped}");
            println!("来源目录: {}", paths.raw_sessions_dir().display());
            Ok(())
        }

        Command::Compile { force, dry_run } | Command::Update { force, dry_run } => {
            let sessions = load_sessions(&cli)?;
            let index = open_index(&cli, &paths)?;
            let options = CompileOptions {
                force: *force,
                dry_run: *dry_run,
            };
            let report = if *dry_run {
                println!("模式: dry-run（不调用 LLM，只打印将要发送的提示词）");
                let llm = DryRunLlm::new();
                let report = pipeline::compile_bundle(
                    &paths,
                    &index,
                    &llm,
                    &cfg,
                    &categories,
                    &sessions,
                    &options,
                    &print_progress,
                    &tokio_util::sync::CancellationToken::new(),
                )
                .await?;
                print_dry_run_prompts(&llm);
                report
            } else {
                let llm = build_llm(&cli)?;
                println!("模型: {}", llm.model_label());
                pipeline::compile_bundle(
                    &paths,
                    &index,
                    llm.as_ref(),
                    &cfg,
                    &categories,
                    &sessions,
                    &options,
                    &print_progress,
                    &tokio_util::sync::CancellationToken::new(),
                )
                .await?
            };
            print_report(&paths, &report);
            Ok(())
        }

        Command::Reindex => {
            let index = open_index(&cli, &paths)?;
            let n = index.rebuild(&paths)?;
            println!("索引已重建：{n} 个概念");
            println!("索引目录: {}", index_dir_of(&cli, &paths)?.display());
            Ok(())
        }

        Command::List { r#type, dir } => {
            let Some(index) = open_index_readonly(&cli, &paths)? else {
                println!("索引尚未建立（先运行 compile 或 reindex）");
                return Ok(());
            };
            let all = index.list()?;
            let filtered: Vec<_> = all
                .iter()
                .filter(|c| r#type.as_deref().map(|t| c.type_name == t).unwrap_or(true))
                .filter(|c| dir.as_deref().map(|d| c.dir == d).unwrap_or(true))
                .collect();
            println!("概念 {} 个（总 {}）", filtered.len(), all.len());
            for c in filtered {
                println!(
                    "  {:<40} {:<12} {}",
                    c.concept_id,
                    c.type_name,
                    if c.title.is_empty() { "-" } else { c.title.as_str() }
                );
            }
            Ok(())
        }

        Command::Show { concept_id } => {
            let path = paths.concept_path(concept_id);
            let content = std::fs::read_to_string(&path)
                .ctx(|| format!("概念不存在: {} ({})", concept_id, path.display()))?;
            println!("{content}");
            Ok(())
        }

        Command::Search { query, limit } => {
            let Some(index) = open_index_readonly(&cli, &paths)? else {
                println!("索引尚未建立（先运行 compile 或 reindex）");
                return Ok(());
            };
            let hits = index.search(query, *limit)?;
            println!("命中 {} 条（查询: {query}）", hits.len());
            for hit in hits {
                println!(
                    "  [{:.3}] {:<40} {}  ({})",
                    hit.score.unwrap_or(0.0),
                    hit.concept_id,
                    if hit.title.is_empty() { "-" } else { hit.title.as_str() },
                    hit.matched.join("+")
                );
            }
            Ok(())
        }

        Command::Validate => {
            let report = validate_bundle(&paths);
            println!("检查概念 {} 个", report.checked);
            println!("Error {} / Warning {}", report.errors(), report.warnings());
            for issue in &report.issues {
                let level = match issue.level {
                    astorm_memory::validate::IssueLevel::Error => "ERROR  ",
                    astorm_memory::validate::IssueLevel::Warning => "WARNING",
                };
                println!(
                    "  {level} {} {}",
                    if issue.concept_id.is_empty() { "-" } else { issue.concept_id.as_str() },
                    issue.message
                );
            }
            if report.is_ok() {
                println!("\nBundle 合规（无 Error）。");
            } else {
                bail!("Bundle 不合规：{} 个 Error", report.errors());
            }
            Ok(())
        }

        Command::Log { limit } => {
            let entries = writer::read_log(&paths, *limit);
            println!("变更历史 {} 条（最新在前）", entries.len());
            for e in entries {
                println!("  {} {:<7} {} ← {}", e.at, e.action, e.concept_id, e.resource);
            }
            Ok(())
        }

        Command::Raw => {
            let mut files: Vec<PathBuf> = std::fs::read_dir(paths.raw_sessions_dir())
                .map(|it| it.flatten().map(|e| e.path()).collect())
                .unwrap_or_default();
            files.sort();
            println!("来源会话 {} 个", files.len());
            for f in files {
                let size = std::fs::metadata(&f).map(|m| m.len()).unwrap_or(0);
                println!(
                    "  {:<40} {:>8} B  {}",
                    f.file_name().unwrap_or_default().to_string_lossy(),
                    size,
                    f.display()
                );
            }
            Ok(())
        }

        Command::Status => {
            let sessions = load_sessions(&cli).unwrap_or_default();
            // 只读：不创建索引、不重建、不写任何目录
            let indexed = open_index_readonly(&cli, &paths)?
                .map(|i| i.count().to_string())
                .unwrap_or_else(|| "未建立".to_string());
            let state = astorm_memory::state::SyncState::load(&paths.state_file());
            print_paths(&cli, &paths);
            let ownership = match paths.ownership() {
                astorm_memory::paths::BundleOwnership::Absent => "未初始化",
                astorm_memory::paths::BundleOwnership::Owned => "本管道所有",
                astorm_memory::paths::BundleOwnership::Foreign { .. } => "⚠️ 他人目录（将拒绝写入）",
            };
            println!("\nBundle 归属: {ownership}");
            println!("概念: {} 个（索引 {indexed}）", paths.concept_files().len());
            println!("来源会话: {} 个", std::fs::read_dir(paths.raw_sessions_dir()).map(|i| i.count()).unwrap_or(0));
            println!("已处理会话: {} 个", state.len());
            println!("待编译会话: {} 个", pipeline::count_pending(&paths, &sessions));
            println!(
                "上次编译: {}",
                state.last_sync.as_deref().unwrap_or("（从未）")
            );
            let stats = pipeline::concept_stats(&paths);
            if !stats.is_empty() {
                println!("按 type:");
                let mut pairs: Vec<_> = stats.into_iter().collect();
                pairs.sort();
                for (t, n) in pairs {
                    println!("  {t:<16} {n}");
                }
            }
            Ok(())
        }

        Command::SelfTest { scratch } => run_self_test(scratch.clone(), &cfg).await,
    }
}

// ---------- 路径与配置解析 ----------

/// astorm 数据目录（`ASTORM_DATA_DIR` 优先；否则平台约定位置）。
fn astorm_data_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("ASTORM_DATA_DIR") {
        if !dir.trim().is_empty() {
            return PathBuf::from(dir);
        }
    }
    #[cfg(target_os = "macos")]
    let base = dirs::home_dir().map(|h| h.join("Library/Application Support"));
    #[cfg(target_os = "windows")]
    let base = dirs::config_dir();
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let base = dirs::data_dir();
    base.unwrap_or_default().join("astorm").join("data")
}

fn resolve_bundle_root(cli: &Cli) -> Result<PathBuf> {
    if let Some(dir) = &cli.bundle {
        return Ok(dir.clone());
    }
    let user_dir = match &cli.user_dir {
        Some(dir) => dir.clone(),
        None => dirs::home_dir()
            .ctx(|| "无法解析用户主目录，请用 --user-dir 或 --bundle 指定".to_string())?
            .join("astorm"),
    };
    Ok(user_dir.join("memory"))
}

fn sessions_dir(cli: &Cli) -> Result<PathBuf> {
    match &cli.sessions_dir {
        Some(dir) => Ok(dir.clone()),
        None => Ok(astorm_data_dir().join("session")),
    }
}

/// 索引目录：默认放在 **Bundle 内部**（`<bundle>/.okf-index`）。
///
/// 放内部而非共享的 app data 目录，是因为索引是纯派生数据、与 bundle 同生共死；
/// 且共享路径一旦被别的工具占用，重建时的清空语义会互相破坏（见 `MemoryIndex::new` 的守卫）。
fn index_dir_of(cli: &Cli, paths: &BundlePaths) -> Result<PathBuf> {
    match &cli.index_dir {
        Some(dir) => Ok(dir.clone()),
        None => Ok(paths.root().join(".okf-index")),
    }
}

/// 打开索引用于**写入**（`compile` / `reindex`）：不存在则创建，并可对账重建。
fn open_index(cli: &Cli, paths: &BundlePaths) -> Result<MemoryIndex> {
    let dir = index_dir_of(cli, paths)?;
    let index = MemoryIndex::new(&dir).ctx(|| format!("打开记忆索引失败: {}", dir.display()))?;
    // 对账：索引为空但 wiki 有内容（外部改动 / 首次使用）→ 自动重建
    if index.is_empty() && !paths.concept_files().is_empty() {
        let n = index.rebuild(paths)?;
        eprintln!("[索引对账] wiki 有内容但索引为空，已重建 {n} 个概念");
    }
    Ok(index)
}

/// 打开索引用于**只读命令**（`status` / `list` / `search`）：不存在返回 `None`，
/// **不创建、不重建任何目录**（只读命令不得有副作用）。
fn open_index_readonly(cli: &Cli, paths: &BundlePaths) -> Result<Option<MemoryIndex>> {
    let dir = index_dir_of(cli, paths)?;
    MemoryIndex::open_existing(&dir)
}

fn load_sessions(cli: &Cli) -> Result<Vec<SessionInput>> {
    let dir = sessions_dir(cli)?;
    if !dir.is_dir() {
        bail!(
            "会话目录不存在: {}（用 --sessions-dir 指定，或先运行一次应用）",
            dir.display()
        );
    }
    pipeline::scan_session_dir(&dir)
}

/// 由 CLI 参数 / 环境 / `.env` 构建记忆配置。
fn build_config(cli: &Cli) -> Result<MemoryConfig> {
    let mut cfg = MemoryConfig::default();
    if let Some(json) = &cli.categories {
        let cats: Vec<MemoryCategory> =
            serde_json::from_str(json).ctx(|| "解析 --categories JSON 失败（应为 [{id,dir,type,hint}]）".to_string())?;
        cfg.categories = cats;
    }
    cfg.provider = cli.provider.clone();
    cfg.model = cli.model.clone().or_else(|| env("LLM_MODEL"));
    Ok(cfg)
}

/// 构建生产 LLM（缺凭据时给出可操作的提示）。
fn build_llm(cli: &Cli) -> Result<Box<dyn MemoryLlm>> {
    let base_url = cli.base_url.clone().or_else(|| env("LLM_BASE_URL"));
    let api_key = cli.api_key.clone().or_else(|| env("LLM_API_KEY"));
    let model = cli.model.clone().or_else(|| env("LLM_MODEL"));

    let (Some(base_url), Some(api_key), Some(model)) = (base_url, api_key, model) else {
        bail!(
            "缺少 LLM 凭据：需要 base-url + api-key + model。\n\
             请任选其一：\n\
             1) 在 backend/.env 写入 LLM_BASE_URL / LLM_MODEL / LLM_API_KEY（仓库既有约定）\n\
             2) 设置同名环境变量\n\
             3) 传 --base-url / --api-key / --model\n\
             若只想验证提示词构造，用 --dry-run（不联网）"
        );
    };
    let client = Arc::new(ChatClient::new(base_url, api_key));
    Ok(Box::new(ChatLlm::new(client, model)))
}

fn env(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.trim().is_empty())
}

// ---------- 输出 ----------

fn print_paths(cli: &Cli, paths: &BundlePaths) {
    println!("Bundle 根:  {}", paths.root().display());
    println!("  raw:      {}", paths.raw_sessions_dir().display());
    println!("  wiki:     {}", paths.wiki_dir().display());
    println!("  状态:     {}", paths.state_file().display());
    if let Ok(dir) = sessions_dir(cli) {
        println!("会话目录:   {}", dir.display());
    }
    if let Ok(dir) = index_dir_of(cli, paths) {
        println!("索引目录:   {}", dir.display());
    }
}

fn print_progress(event: ProgressEvent) {
    println!(
        "  [{}] {}/{} {}",
        event.stage.as_str(),
        event.current,
        event.total,
        event.message
    );
}

fn print_report(paths: &BundlePaths, report: &CompileReport) {
    println!("\n编译完成{}", if report.cancelled { "（已取消）" } else { "" });
    println!(
        "  会话: 总 {} / 处理 {} / 跳过 {}",
        report.total_sessions, report.processed_sessions, report.skipped_sessions
    );
    println!(
        "  记忆单元 {} → 概念 {}（新建 {} / 更新 {} / 已覆盖 {}）",
        report.extracted_units,
        report.compiled_concepts,
        report.created.len(),
        report.updated.len(),
        report.skipped.len()
    );
    println!(
        "  LLM 调用 {} 次，token 输入 {} / 输出 {}",
        report.calls, report.prompt_tokens, report.completion_tokens
    );
    for id in &report.created {
        println!("    + {id}");
    }
    for id in &report.updated {
        println!("    ~ {id}");
    }
    for f in &report.failed_sessions {
        println!("    ! 失败 [{}] {}: {}", f.stage, f.session_id, f.message);
    }
    for w in &report.warnings {
        println!("    ? 告警: {w}");
    }
    if !report.cancelled && report.failed_sessions.is_empty() {
        let validation = validate_bundle(paths);
        println!(
            "  校验: 概念 {}，Error {} / Warning {}",
            validation.checked,
            validation.errors(),
            validation.warnings()
        );
        for issue in validation.issues.iter().take(10) {
            println!("    - {}: {}", issue.concept_id, issue.message);
        }
    }
}

/// dry-run：打印将发送给模型的提示词（截断显示，避免刷屏）。
fn print_dry_run_prompts(llm: &DryRunLlm) {
    let prompts = llm.prompts();
    println!("\n将要发送的 LLM 调用 {} 次：", prompts.len());
    for (i, p) in prompts.iter().enumerate() {
        println!("\n--- 调用 {} / 阶段 {:?} ---", i + 1, p.stage);
        println!("[system 前 400 字]\n{}", first_chars(&p.system, 400));
        println!("[user 前 1500 字]\n{}", first_chars(&p.user, 1500));
    }
}

fn first_chars(s: &str, n: usize) -> String {
    let head: String = s.chars().take(n).collect();
    if s.chars().count() > n {
        format!("{head}\n…（共 {} 字符）", s.chars().count())
    } else {
        head
    }
}

// ---------- 离线自检 ----------

/// 用内建脚本 LLM 在**临时 Bundle** 上跑完整管道（不联网、不污染真实 Bundle）。
async fn run_self_test(scratch: Option<PathBuf>, cfg: &MemoryConfig) -> Result<()> {
    let root = scratch.unwrap_or_else(|| {
        std::env::temp_dir().join(format!("okf-agent-selftest-{}", std::process::id()))
    });
    let _ = std::fs::remove_dir_all(&root);
    let paths = BundlePaths::new(&root);
    let idx_dir = root.join(astorm_memory::paths::INDEX_DIR_NAME);
    let index = MemoryIndex::new(&idx_dir)?;
    let categories = CategoryTable::new(cfg.categories.clone());

    println!("自检 Bundle: {}", paths.root().display());

    // 构造一段真实形态的会话（含 system / 工具调用 / 工具结果 / 环境块）
    let jsonl = self_test_session_jsonl();
    let session = SessionInput::new("selftest-sid-0001", jsonl.clone())
        .with_stamp(astorm_memory::state::SourceStamp::new(jsonl.len() as u64, 1_760_000_400))
        .with_title(Some("自检会话".into()));

    let llm = ScriptedLlm::new()
        .with(
            astorm_memory::llm::PromptStage::Extract,
            serde_json::json!({"events": [
                {"event_text": "认证方案选择 JWT 而非 session", "frag_ids": ["m01", "m02"], "event_type": "decision", "confidence": 0.95},
                {"event_text": "用户偏好中文注释", "frag_ids": ["m02"], "event_type": "preference", "confidence": 0.8},
                {"event_text": "后端是 napi dll 需改名 .node", "frag_ids": ["m03"], "event_type": "fact", "confidence": 0.9}
            ]}),
        )
        // 精选（P1.5）：自检直接全部保留（真实模型在这里会去噪 / 合并）
        .with(
            astorm_memory::llm::PromptStage::Curate,
            serde_json::json!({"events": [
                {"event_text": "认证方案选择 JWT 而非 session", "frag_ids": ["m01", "m02"], "event_type": "decision", "confidence": 0.95},
                {"event_text": "用户偏好中文注释", "frag_ids": ["m02"], "event_type": "preference", "confidence": 0.8},
                {"event_text": "后端是 napi dll 需改名 .node", "frag_ids": ["m03"], "event_type": "fact", "confidence": 0.9}
            ]}),
        )
        .with(
            astorm_memory::llm::PromptStage::Compile,
            serde_json::json!({"concepts": [
                {"event_type": "decision", "frag_ids": ["m01", "m02"], "concept_id": "use-jwt",
                 "frontmatter": {"type": "Decision", "title": "认证方案用 JWT", "description": "无状态鉴权选型", "tags": ["auth"]},
                 "body": "选择 JWT 而非 session，理由是支持多端无状态鉴权。"},
                {"event_type": "preference", "frag_ids": ["m02"], "concept_id": "chinese-comments",
                 "frontmatter": {"type": "Preference", "title": "偏好中文注释", "tags": ["style", "i18n"]},
                 "body": "用户偏好中文注释与中文提交信息。"},
                {"event_type": "fact", "frag_ids": ["m03"], "concept_id": "napi-dll",
                 "frontmatter": {"type": "Fact", "title": "后端是 napi dll", "tags": ["backend"]},
                 "body": "后端产物必须重命名为 .node 才能被 createRequire 加载。"}
            ]}),
        );

    // 第一次：完整管道
    println!("\n[1/4] 首次编译");
    let report = pipeline::compile_bundle(
        &paths,
        &index,
        &llm,
        cfg,
        &categories,
        std::slice::from_ref(&session),
        &CompileOptions::default(),
        &print_progress,
        &tokio_util::sync::CancellationToken::new(),
    )
    .await?;
    print_report(&paths, &report);
    if report.created.len() != 3 {
        bail!("自检失败：应新建 3 个概念，实际 {}", report.created.len());
    }

    // 第二次：同输入应全部跳过（增量）
    println!("\n[2/4] 增量复跑（应跳过且零 LLM 调用）");
    let quiet = ScriptedLlm::new(); // 严格模式：任何调用都会报错
    let second = pipeline::compile_bundle(
        &paths,
        &index,
        &quiet,
        cfg,
        &categories,
        std::slice::from_ref(&session),
        &CompileOptions::default(),
        &print_progress,
        &tokio_util::sync::CancellationToken::new(),
    )
    .await?;
    if second.processed_sessions != 0 || second.skipped_sessions != 1 {
        bail!("自检失败：增量应跳过，实际 {second:?}");
    }
    if !quiet.prompts().is_empty() {
        bail!("自检失败：增量跳过时不应调用 LLM");
    }
    println!("  OK：跳过 1 个会话，LLM 调用 0 次");

    // 第三次：强制重跑应幂等（内容未变 → 不新增、不追加）
    println!("\n[3/4] 强制重跑（--force，应幂等）");
    let body_before = std::fs::read_to_string(paths.concept_path("decisions/use-jwt"))
        .ctx(|| "读取自检概念失败".to_string())?;
    let third = pipeline::compile_bundle(
        &paths,
        &index,
        &llm,
        cfg,
        &categories,
        std::slice::from_ref(&session),
        &CompileOptions {
            force: true,
            dry_run: false,
        },
        &print_progress,
        &tokio_util::sync::CancellationToken::new(),
    )
    .await?;
    if !third.created.is_empty() || !third.updated.is_empty() {
        bail!("自检失败：强制重跑不应新建/更新（实际 created={:?} updated={:?}）", third.created, third.updated);
    }
    let body_after = std::fs::read_to_string(paths.concept_path("decisions/use-jwt"))
        .ctx(|| "读取自检概念失败".to_string())?;
    if body_before != body_after {
        bail!("自检失败：强制重跑的 body 发生了变化（不幂等）");
    }
    println!("  OK：0 新建 / 0 更新 / {} 已覆盖，body 未变", third.skipped.len());

    // 第四次：OKF 合规 + 索引可召回 + 中文检索
    println!("\n[4/4] 合规校验与检索");
    let validation = validate_bundle(&paths);
    println!(
        "  校验: 概念 {}，Error {} / Warning {}",
        validation.checked,
        validation.errors(),
        validation.warnings()
    );
    for issue in &validation.issues {
        println!("    - {} {}", issue.concept_id, issue.message);
    }
    if !validation.is_ok() {
        bail!("自检失败：Bundle 不合规（{} 个 Error）", validation.errors());
    }
    if index.count() != 3 {
        bail!("自检失败：索引应有 3 个概念，实际 {}", index.count());
    }
    let hits = index.search("认证", 10)?;
    if hits.len() != 1 || hits[0].concept_id != "decisions/use-jwt" {
        bail!("自检失败：中文检索未命中预期概念: {hits:?}");
    }
    println!("  OK：中文检索「认证」命中 {}", hits[0].concept_id);

    println!("\n自检全部通过。产物目录: {}", paths.root().display());
    println!("（如需保留请勿删除；下个进程 ID 会生成新目录）");
    Ok(())
}

/// 自检用会话（真实形态：system / 环境块 / 工具调用 / 工具结果）。
fn self_test_session_jsonl() -> String {
    let msgs = serde_json::json!([
        {"id": "st:0", "sessionId": "selftest-sid-0001", "role": "system", "content": "你是 astorm……", "toolCalls": null, "seq": 0, "createdAt": 1760000000},
        {"id": "st:1", "sessionId": "selftest-sid-0001", "role": "user", "content": "认证方案怎么选？\n\n【会话环境】\n- 会话模式：execute\n- 工作区：/tmp/selftest\n- 可用工具：bash, read_file", "toolCalls": null, "seq": 1, "createdAt": 1760000100},
        {"id": "st:2", "sessionId": "selftest-sid-0001", "role": "assistant", "content": "建议用 JWT。", "toolCalls": [{"id": "call_1", "type": "function", "function": {"name": "bash", "arguments": "{\"command\":\"grep -rn jwt backend\"}"}}], "seq": 2, "createdAt": 1760000200},
        {"id": "st:tool:call_1", "sessionId": "selftest-sid-0001", "role": "tool", "content": "backend/src/auth.rs:12: JWT ...", "toolCalls": null, "seq": 3, "createdAt": 1760000300, "toolCallId": "call_1", "metadata": {"status": "ok"}},
        {"id": "st:4", "sessionId": "selftest-sid-0001", "role": "user", "content": "那就用 JWT 吧", "toolCalls": null, "seq": 4, "createdAt": 1760000400}
    ]);
    msgs.as_array()
        .unwrap()
        .iter()
        .map(|m| serde_json::to_string(m).unwrap())
        .collect::<Vec<_>>()
        .join("\n")
}
