//! 技能系统单测：SKILL.md frontmatter 解析 + Tantivy 索引搜索 / 导入（本地离线）。

use std::path::Path;

use crate::skill::index::SkillIndex;
use crate::skill::SkillDoc;

/// 构造合法 SKILL.md 内容。
fn skill_md(name: &str, desc: &str, body: &str) -> String {
    format!(
        "---\nname: {name}\ndescription: {desc}\nversion: 1.0.0\nmetadata:\n  category: coding\n  tags: rust, async\n---\n\n{body}"
    )
}

fn tmp_dir(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("astorm_skill_{tag}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

#[test]
fn parse_valid_frontmatter() {
    let md = skill_md("fmt-helper", "帮我格式化代码", "# Fmt\n自动格式化");
    let doc = SkillIndex::parse_skill_content(&md, Path::new("/tmp/fmt/SKILL.md")).unwrap();
    assert_eq!(doc.name, "fmt-helper");
    assert_eq!(doc.description, "帮我格式化代码");
    assert_eq!(doc.prompt, "# Fmt\n自动格式化");
    assert_eq!(doc.category, "coding");
    assert_eq!(doc.tags, vec!["rust", "async"]);
    assert_eq!(doc.version, "1.0.0");
    assert_eq!(doc.source, "file");
    assert!(doc.enabled);
}

#[test]
fn parse_skill_errors() {
    // 缺少 frontmatter
    let err = SkillIndex::parse_skill_content("# No Frontmatter", Path::new("x/SKILL.md"))
        .unwrap_err();
    assert!(err.contains("缺少 frontmatter"), "缺 frontmatter 应报错: {err}");
    // frontmatter 未闭合
    let err = SkillIndex::parse_skill_content("---\nname: no-close", Path::new("x/SKILL.md"))
        .unwrap_err();
    assert!(err.contains("未闭合"), "未闭合应报错: {err}");
    // YAML 解析失败
    let err = SkillIndex::parse_skill_content("---\nname: [broken\n---\nbody", Path::new("x/SKILL.md"))
        .unwrap_err();
    assert!(err.contains("frontmatter 解析失败"), "畸形 yaml 应报错: {err}");
}

#[test]
fn index_and_search() {
    let base = tmp_dir("index");
    let idx_dir = base.join("index");
    let user_dir = base.join("skills");
    let mut index = SkillIndex::new(&idx_dir, &user_dir).unwrap();

    let doc = SkillDoc {
        name: "rust-helper".into(),
        description: "rust async runtime concurrency 帮助".into(),
        prompt: "用 rust async runtime 编写并发程序，帮助程序员提效".into(),
        category: "coding".into(),
        tags: vec!["rust".into()],
        version: "1.0.0".into(),
        author: "tester".into(),
        source: "file".into(),
        source_path: "rust-helper/SKILL.md".into(),
        enabled: true,
        updated_at: 1,
    };
    index.index_skill(&doc).unwrap();
    index.commit().unwrap();

    // 名称 / 描述检索命中
    let hits = index.search("runtime", 5).unwrap();
    assert_eq!(hits.len(), 1, "应命中: {hits:?}");
    assert_eq!(hits[0].name, "rust-helper");
    assert!(hits[0].score.unwrap() > 0.0, "应带相关度得分");

    // 正文检索（search_content）
    let hits = index.search_content("程序员", 5).unwrap();
    assert_eq!(hits.len(), 1, "正文应命中: {hits:?}");

    // 按名称读全文
    let got = index.get_by_name("rust-helper").unwrap().expect("应存在");
    assert_eq!(got.prompt, "用 rust async runtime 编写并发程序，帮助程序员提效");
    assert_eq!(got.tags, vec!["rust"]);

    // list_by_names：存在 + 缺失混合
    let summaries = index
        .list_by_names(&["rust-helper".into(), "nope".into()])
        .unwrap();
    assert_eq!(summaries.len(), 1);
    assert_eq!(summaries[0].name, "rust-helper");

    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn import_from_path_indexes_skill() {
    let base = tmp_dir("import");
    let idx_dir = base.join("index");
    let user_dir = base.join("skills");
    let mut index = SkillIndex::new(&idx_dir, &user_dir).unwrap();

    let src = base.join("my-skill");
    std::fs::create_dir_all(&src).unwrap();
    let src_file = src.join("SKILL.md");
    std::fs::write(&src_file, skill_md("imported-skill", "被导入的技能", "do stuff")).unwrap();

    let doc = index.import_from_path(&src_file).unwrap();
    assert_eq!(doc.source, "user");
    index.commit().unwrap();
    assert!(index.get_by_name("imported-skill").unwrap().is_some());
    // 已复制到用户技能目录
    assert!(user_dir.join("imported-skill").join("SKILL.md").is_file());

    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn import_from_path_copies_asset_dirs() {
    let base = tmp_dir("import_assets");
    let idx_dir = base.join("index");
    let user_dir = base.join("skills");
    let mut index = SkillIndex::new(&idx_dir, &user_dir).unwrap();

    let src = base.join("rich-skill");
    std::fs::create_dir_all(src.join("references")).unwrap();
    std::fs::create_dir_all(src.join("scripts")).unwrap();
    std::fs::create_dir_all(src.join("unused")).unwrap();
    std::fs::write(src.join("SKILL.md"), skill_md("rich-skill", "带附属目录的技能", "body")).unwrap();
    std::fs::write(src.join("references").join("api.md"), "# API").unwrap();
    std::fs::write(src.join("scripts").join("run.py"), "print(1)").unwrap();

    let src_file = src.join("SKILL.md");
    index.import_from_path(&src_file).unwrap();
    index.commit().unwrap();

    let dest = user_dir.join("rich-skill");
    assert!(dest.join("SKILL.md").is_file());
    assert!(dest.join("references").join("api.md").is_file(), "附属 references/ 应复制");
    assert!(dest.join("scripts").join("run.py").is_file(), "附属 scripts/ 应复制");
    assert!(!dest.join("unused").exists(), "白名单外目录不应复制");

    // 重复导入幂等：目标目录先清空，不残留已删除的旧文件
    std::fs::write(dest.join("references").join("stale.md"), "old").unwrap();
    index.import_from_path(&src_file).unwrap();
    index.commit().unwrap();
    assert!(!dest.join("references").join("stale.md").exists(), "重复导入应清空旧目标目录");
    assert!(dest.join("references").join("api.md").is_file(), "重新导入后附属文件仍在");

    let _ = std::fs::remove_dir_all(&base);
}