//! git 门面:基于系统 git CLI(porcelain 解析),文件树「查看变更」等功能使用。
//! 注意:Windows 下执行 git 必须带 CREATE_NO_WINDOW,否则每次调用都会弹出控制台窗口。

use std::process::Command;

use astorm_common::git::{DiffFileEntry, Git, GitEnvInfo, GitFileChange, GitFileDiff, GitStatus};

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

/// Windows 下不创建控制台窗口(静默执行子进程)。
#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// 组装一次 git 调用(带静默标志)。
fn git_command(dir: &str) -> Command {
    let mut cmd = Command::new("git");
    // 中文/非 ASCII 路径默认被 quote 成八进制转义，后续按路径取 diff 会失配
    cmd.args(["-c", "core.quotePath=false", "-C"]).arg(dir);
    #[cfg(target_os = "windows")]
    cmd.creation_flags(CREATE_NO_WINDOW);
    cmd
}

/// 执行一次 git 子进程并取 stdout 文本(非 0 退出 → Err(stderr))。
fn git_stdout(dir: &str, args: &[&str]) -> Result<String, String> {
    let out = git_command(dir)
        .args(args)
        .output()
        .map_err(|e| format!("git 不可用: {e}"))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        // 非仓库：`git diff` 会向 stderr 打出整段 `--no-index` usage（几百行），经 IPC 抛回
        // 前端既刷屏又无意义 → 归一成稳定短错误，前端据此显示「不是 Git 仓库」空态。
        if stderr.to_ascii_lowercase().contains("not a git repository") {
            return Err("notRepo".to_string());
        }
        return Err(stderr.trim().to_string());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// `git diff` 的公共参数:子命令 + 范围(`--cached` 看索引,否则看工作树)+ 关重命名。
///
/// 带 `--no-renames` 是有意的:重命名退化为「删 + 增」两条,避免解析
/// `R100\told\tnew` 的双路径形式(审查页签按单路径展示,重命名检测收益低、解析成本高)。
fn diff_scope_args(staged: bool) -> Vec<&'static str> {
    let mut v = vec!["diff"];
    if staged {
        v.push("--cached");
    }
    v.push("--no-renames");
    v
}

/// 文本行数(空串 = 0;末尾换行不额外算一行)。
fn text_lines(s: &str) -> u32 {
    if s.is_empty() {
        return 0;
    }
    s.lines().count() as u32
}

/// 内容是否像二进制(NUL 字节,与 git 的启发式一致)。
fn looks_binary(text: &str) -> bool {
    text.contains('\0')
}

/// 未跟踪文件列表(`git ls-files --others --exclude-standard`)。
fn untracked_files(dir: &str) -> Result<Vec<String>, String> {
    let text = git_stdout(dir, &["ls-files", "--others", "--exclude-standard"])?;
    Ok(text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect())
}

/// 读工作树文件内容(相对仓库根的路径);失败回落空串。
fn read_worktree(dir: &str, path: &str) -> String {
    std::fs::read_to_string(std::path::Path::new(dir).join(path)).unwrap_or_default()
}

/// 解析 `git diff --numstat`:每行 `新增\t删除\t路径`;二进制为 `-\t-`(记 0/0)。
fn parse_numstat(text: &str) -> std::collections::HashMap<String, (u32, u32)> {
    let mut counts = std::collections::HashMap::new();
    for line in text.lines() {
        let mut it = line.splitn(3, '\t');
        let (Some(add), Some(del), Some(path)) = (it.next(), it.next(), it.next()) else {
            continue;
        };
        let path = path.trim();
        if path.is_empty() {
            continue;
        }
        // 非数字(二进制 `-`)→ 0
        let add = add.trim().parse::<u32>().unwrap_or(0);
        let del = del.trim().parse::<u32>().unwrap_or(0);
        counts.insert(path.to_string(), (add, del));
    }
    counts
}

/// 解析 `git diff --name-status`:每行 `状态\t路径`,状态取首字母(A/M/D)。空行跳过。
fn parse_name_status(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for line in text.lines() {
        let mut it = line.splitn(2, '\t');
        let (Some(status), Some(path)) = (it.next(), it.next()) else {
            continue;
        };
        let path = path.trim();
        if path.is_empty() {
            continue;
        }
        out.push((
            status.trim().chars().next().unwrap_or('M').to_string(),
            path.to_string(),
        ));
    }
    out
}

/// git 门面。
pub struct GitImpl;

impl Git for GitImpl {
    /// 初始化仓库：`git init -q`（静默，成功无输出）。
    ///
    /// 供文件树 / 代码审查页签空状态的「创建 Git 仓库」按钮使用；已初始化时 git 也会
    /// 返回成功（reinit 无害），故不预检是否已是仓库。
    fn git_init(&self, dir: String) -> Result<String, String> {
        let out = git_command(&dir)
            .args(["init", "-q"])
            .output()
            .map_err(|e| format!("git 不可用: {e}"))?;
        if !out.status.success() {
            return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
        }
        Ok(format!("已在 {dir} 初始化 Git 仓库"))
    }

    /// 工作区状态(staged / unstaged):`git status --porcelain` 逐行解析。
    /// 状态字:已暂存取 X,未暂存取 Y;未跟踪(? ?)归入 unstaged,状态字 '?'。
    /// 路径保持 porcelain 原样(相对仓库根,正斜杠),diff 留空(按需再由 get_git_file_diff 取)。
    fn git_status(&self, dir: String) -> Result<GitStatus, String> {
        let out = git_command(&dir)
            .args(["status", "--porcelain"])
            .output()
            .map_err(|e| format!("git 不可用: {e}"))?;
        if !out.status.success() {
            return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
        }
        let text = String::from_utf8_lossy(&out.stdout);
        let mut staged = Vec::new();
        let mut unstaged = Vec::new();
        for line in text.lines() {
            let bytes = line.as_bytes();
            if bytes.len() < 3 {
                continue;
            }
            let x = bytes[0] as char;
            let y = bytes[1] as char;
            let path = line[3..].to_string();
            if x != ' ' && x != '?' {
                staged.push(GitFileChange {
                    id: path.clone(),
                    path: path.clone(),
                    status: x.to_string(),
                    diff: String::new(),
                });
            }
            if y != ' ' && y != '?' {
                unstaged.push(GitFileChange {
                    id: path.clone(),
                    path: path.clone(),
                    status: y.to_string(),
                    diff: String::new(),
                });
            }
            // 未跟踪(??)
            if x == '?' || y == '?' {
                unstaged.push(GitFileChange {
                    id: path.clone(),
                    path,
                    status: "?".to_string(),
                    diff: String::new(),
                });
            }
        }
        Ok(GitStatus { staged, unstaged })
    }

    fn git_stage(&self, _dir: String, _files: Vec<String>) -> Result<String, String> {
        Err(super::not_impl("git", "git_stage"))
    }

    fn git_unstage(&self, _dir: String, _files: Vec<String>) -> Result<String, String> {
        Err(super::not_impl("git", "git_unstage"))
    }

    fn git_commit(&self, _dir: String, _message: String, _files: Vec<String>) -> Result<String, String> {
        Err(super::not_impl("git", "git_commit"))
    }

    fn git_push(&self, _dir: String) -> Result<String, String> {
        Err(super::not_impl("git", "git_push"))
    }

    fn check_git_env(&self, _dir: String) -> Result<GitEnvInfo, String> {
        Err(super::not_impl("git", "check_git_env"))
    }

    /// 当前分支名:系统提示词「会话环境」块每轮实时取用(替代会话记录里的建会话快照)。
    ///
    /// 未出生分支(`--abbrev-ref HEAD` 在无提交的仓库里返回 `HEAD`)与分离 HEAD 都返回
    /// 字面量 `HEAD`,并非真分支名 → 归一成空串,由调用方回落「未知」。
    fn git_current_branch(&self, dir: String) -> Result<String, String> {
        let out = git_stdout(&dir, &["rev-parse", "--abbrev-ref", "HEAD"])?;
        let name = out.trim();
        if name.is_empty() || name == "HEAD" {
            return Ok(String::new());
        }
        Ok(name.to_string())
    }

    /// 差异摘要(代码审查页签的文件列表):`--numstat` 取增删计数,`--name-status` 取状态字,
    /// 按路径合并;未跟踪文件(staged=false 时)追加为状态 `?`、新增行数 = 文件行数。
    ///
    /// 一律带 `--no-renames`:避免 `R100\told\tnew` 双路径解析,重命名退化为「删 + 增」两条。
    /// 二进制文件 numstat 为 `-\t-`(计数记 0)。
    fn get_git_diff_summary(&self, dir: String, staged: bool) -> Result<Vec<DiffFileEntry>, String> {
        let mut numstat_args = diff_scope_args(staged);
        numstat_args.push("--numstat");
        let numstat = git_stdout(&dir, &numstat_args)?;

        let mut name_args = diff_scope_args(staged);
        name_args.push("--name-status");
        let name_status = git_stdout(&dir, &name_args)?;

        // 计数表:path → (additions, deletions)
        let counts = parse_numstat(&numstat);

        let mut entries: Vec<DiffFileEntry> = Vec::new();
        for (status, path) in parse_name_status(&name_status) {
            let (additions, deletions) = counts.get(&path).copied().unwrap_or((0, 0));
            entries.push(DiffFileEntry {
                path,
                status,
                additions,
                deletions,
            });
        }

        // 未跟踪(仅未暂存档):git diff 看不到,状态 `?`,新增行数 = 工作树文件行数
        if !staged {
            for path in untracked_files(&dir)? {
                if entries.iter().any(|e| e.path == path) {
                    continue;
                }
                let content = read_worktree(&dir, &path);
                entries.push(DiffFileEntry {
                    path,
                    status: "?".to_string(),
                    additions: text_lines(&content),
                    deletions: 0,
                });
            }
        }

        entries.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(entries)
    }

    /// 单个文件差异全文 + 新侧总行数。
    ///
    /// 未跟踪文件(`git diff` 无输出但文件存在)合成为「每行前加 `+`」的伪 diff,
    /// 使审查页签对未跟踪文件也能展示内容。
    fn get_git_file_diff(
        &self,
        dir: String,
        path: String,
        staged: bool,
    ) -> Result<GitFileDiff, String> {
        let mut args = diff_scope_args(staged);
        args.extend(["--", path.as_str()]);
        let diff = git_stdout(&dir, &args)?;

        // 新侧总行数:staged 取索引 blob(`git show :<path>`),unstaged 取工作树文件
        let new_content = if staged {
            git_stdout(&dir, &["show", &format!(":{path}")]).unwrap_or_default()
        } else {
            read_worktree(&dir, &path)
        };
        let binary = looks_binary(&new_content) || diff.contains("Binary files");

        if !diff.trim().is_empty() {
            return Ok(GitFileDiff {
                diff,
                new_lines: if binary { 0 } else { text_lines(&new_content) },
            });
        }

        // `git diff` 无输出:未跟踪文件(或已暂存档里不存在)——合成为全新增
        if staged || new_content.is_empty() {
            return Ok(GitFileDiff {
                diff,
                new_lines: if binary { 0 } else { text_lines(&new_content) },
            });
        }
        if binary {
            return Ok(GitFileDiff {
                diff: format!("Binary files a/{path} and b/{path} differ\n"),
                new_lines: 0,
            });
        }
        let body: String = new_content
            .lines()
            .map(|l| format!("+{l}\n"))
            .collect::<Vec<_>>()
            .join("");
        Ok(GitFileDiff {
            diff: format!("--- /dev/null\n+++ b/{path}\n{body}"),
            new_lines: text_lines(&new_content),
        })
    }

    /// 被 git 忽略的路径列表:`git status --ignored --porcelain` 的 `!! ` 行。
    /// 目录会带尾部 `/`(如 node_modules/),统一去掉;非仓库/无忽略时为空。
    fn git_ignored(&self, dir: String) -> Result<Vec<String>, String> {
        let out = git_command(&dir)
            .args(["status", "--ignored", "--porcelain"])
            .output()
            .map_err(|e| format!("git 不可用: {e}"))?;
        if !out.status.success() {
            return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
        }
        let text = String::from_utf8_lossy(&out.stdout);
        let mut ignored = Vec::new();
        for line in text.lines() {
            if line.starts_with("!! ") {
                let mut p = line[3..].to_string();
                while p.ends_with('/') {
                    p.pop();
                }
                if !p.is_empty() {
                    ignored.push(p);
                }
            }
        }
        Ok(ignored)
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numstat_parses_counts_and_binary() {
        let text = "6\t0\tbackend/crates/a.rs\n12\t3\tfrontend/src/b.ts\n-\t-\tassets/logo.png\n";
        let m = parse_numstat(text);
        assert_eq!(m.get("backend/crates/a.rs"), Some(&(6, 0)));
        assert_eq!(m.get("frontend/src/b.ts"), Some(&(12, 3)));
        // 二进制 `-\t-` → 0/0
        assert_eq!(m.get("assets/logo.png"), Some(&(0, 0)));
    }

    #[test]
    fn numstat_skips_malformed_lines() {
        let m = parse_numstat("garbage\n\n5\t1\t\n");
        assert!(m.is_empty(), "无路径的行必须跳过: {m:?}");
    }

    #[test]
    fn name_status_takes_first_letter() {
        // --no-renames 下只有 A/M/D，但兼容带分数后缀的 R100/C75
        let text = "M\tbackend/a.rs\nA\tnew.txt\nD\told.txt\nR100\tx.rs\n";
        let got = parse_name_status(text);
        assert_eq!(
            got,
            vec![
                ("M".to_string(), "backend/a.rs".to_string()),
                ("A".to_string(), "new.txt".to_string()),
                ("D".to_string(), "old.txt".to_string()),
                ("R".to_string(), "x.rs".to_string()),
            ]
        );
    }

    #[test]
    fn text_lines_counts_without_trailing_blank() {
        assert_eq!(text_lines(""), 0);
        assert_eq!(text_lines("a"), 1);
        assert_eq!(text_lines("a\n"), 1);
        assert_eq!(text_lines("a\nb\n"), 2);
    }
}

#[cfg(test)]
mod repo_tests {
    use super::*;

    /// 在临时目录跑 git(带 -c 身份与禁签名,避免依赖全局配置)。
    fn run(dir: &std::path::Path, args: &[&str]) -> bool {
        let mut cmd = Command::new("git");
        cmd.arg("-C").arg(dir);
        cmd.args(["-c", "user.email=t@t", "-c", "user.name=t", "-c", "commit.gpgsign=false"]);
        cmd.args(args);
        cmd.output().map(|o| o.status.success()).unwrap_or(false)
    }

    /// 端到端:真实 git 仓库上验证摘要(含未跟踪)与单文件 diff(含未跟踪合成)。
    /// git 不可用的环境直接跳过(不算失败)。
    #[test]
    fn summary_and_file_diff_on_real_repo() {
        if Command::new("git").arg("--version").output().is_err() {
            eprintln!("跳过:环境无 git");
            return;
        }
        let dir = std::env::temp_dir().join(format!("astorm_git_e2e_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("建临时目录");
        assert!(run(&dir, &["init", "-q"]), "git init 失败");
        std::fs::write(dir.join("a.txt"), "one\ntwo\n").unwrap();
        assert!(run(&dir, &["add", "-A"]), "git add 失败");
        assert!(run(&dir, &["commit", "-q", "-m", "init"]), "git commit 失败");

        // 改动已跟踪文件 + 新增未跟踪文件
        std::fs::write(dir.join("a.txt"), "one\ntwo\nthree\n").unwrap();
        std::fs::write(dir.join("b.txt"), "x\ny\nz\n").unwrap();

        let git = GitImpl;
        let dir_s = dir.to_string_lossy().into_owned();

        // 未暂存摘要:修改的 a.txt(additions=1) + 未跟踪的 b.txt(status '?', additions=3)
        let list = git.get_git_diff_summary(dir_s.clone(), false).expect("摘要失败");
        let a = list.iter().find(|e| e.path == "a.txt").expect("缺 a.txt");
        assert_eq!((a.status.as_str(), a.additions, a.deletions), ("M", 1, 0));
        let b = list.iter().find(|e| e.path == "b.txt").expect("缺 b.txt");
        assert_eq!((b.status.as_str(), b.additions, b.deletions), ("?", 3, 0));

        // 已跟踪文件的 diff:含新增行,新侧 3 行
        let da = git.get_git_file_diff(dir_s.clone(), "a.txt".into(), false).expect("diff 失败");
        assert!(da.diff.contains("+three"), "diff 应含新增行: {}", da.diff);
        assert_eq!(da.new_lines, 3);

        // 未跟踪文件:合成「全新增」diff——正文每行前缀 +（`+++` 是文件头,不算内容行）
        let db = git.get_git_file_diff(dir_s.clone(), "b.txt".into(), false).expect("diff 失败");
        let added = db
            .diff
            .lines()
            .filter(|l| l.starts_with('+') && !l.starts_with("+++"))
            .count();
        assert_eq!(added, 3, "合成 diff 应为 3 条新增行:\n{}", db.diff);
        assert_eq!(db.new_lines, 3);

        // 全部暂存后:未暂存摘要清空,已暂存摘要里 b.txt 状态为 A
        assert!(run(&dir, &["add", "-A"]), "git add -A 失败");
        let unstaged = git.get_git_diff_summary(dir_s.clone(), false).expect("摘要失败");
        assert!(unstaged.is_empty(), "暂存后未暂存应为空: {unstaged:?}");
        let staged = git.get_git_diff_summary(dir_s.clone(), true).expect("摘要失败");
        let b2 = staged.iter().find(|e| e.path == "b.txt").expect("已暂存缺 b.txt");
        assert_eq!(b2.status, "A");
        let db2 = git
            .get_git_file_diff(dir_s.clone(), "b.txt".into(), true)
            .expect("已暂存 diff 失败");
        assert_eq!(db2.new_lines, 3);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 实时分支探测:普通分支名 / 分离 HEAD 归一为空串 / 非仓库报错。
    #[test]
    fn current_branch_on_real_repo() {
        if Command::new("git").arg("--version").output().is_err() {
            eprintln!("跳过:环境无 git");
            return;
        }
        let dir = std::env::temp_dir().join(format!("astorm_git_branch_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("建临时目录");
        let git = GitImpl;
        let dir_s = dir.to_string_lossy().into_owned();

        // 非仓库 → Err(提示词那边据此回落「未知」)
        assert!(git.git_current_branch(dir_s.clone()).is_err(), "非仓库应报错");

        assert!(run(&dir, &["init", "-q"]), "git init 失败");
        std::fs::write(dir.join("a.txt"), "one\n").unwrap();
        assert!(run(&dir, &["add", "-A"]), "git add 失败");
        assert!(run(&dir, &["commit", "-q", "-m", "init"]), "git commit 失败");
        // 显式建分支:不依赖机器的 init.defaultBranch(main / master 视配置而定)
        assert!(run(&dir, &["checkout", "-q", "-b", "probe"]), "checkout -b 失败");
        assert_eq!(git.git_current_branch(dir_s.clone()).unwrap(), "probe");

        // 分离 HEAD:git 返回字面量 "HEAD"(并非分支名)→ 归一成空串
        assert!(run(&dir, &["checkout", "-q", "--detach"]), "detach 失败");
        assert_eq!(git.git_current_branch(dir_s).unwrap(), "");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
