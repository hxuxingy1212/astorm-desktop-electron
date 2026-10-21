//! 记忆管道的**配置契约**（本 crate 拥有；应用侧 `astorm-core` 直接引用并作为 `AppConfig.memory`）。
//!
//! 分类体系是**配置驱动**的：OKF 的 `type` 是描述性而非枚举性的（消费者必须容忍未知类型），
//! 因此分类不做硬编码白名单，而是配置数据 —— 新增分类只需改 `memory.categories`。
//!
//! 前端 JSON 结构（camelCase）：
//! ```json
//! { "memory": { "enabled": true, "autoCompile": false, "categories": [
//!     { "id": "decision", "dir": "decisions", "type": "Decision", "hint": "一次性的选择及其理由" }
//! ] } }
//! ```

use serde::{Deserialize, Serialize};

/// 记忆概念**分类**：决定提取阶段的 `event_type` 取值集、编译阶段的目录映射与 OKF `type` 值。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryCategory {
    /// 稳定标识：提取阶段 LLM 输出的 `event_type` 取值（小写、无空格；规范化后写入）。
    pub id: String,
    /// 目标目录名（相对 `wiki/`；空串表示直接放 `wiki/` 根，不做二级目录）。
    #[serde(default)]
    pub dir: String,
    /// 写入 Concept frontmatter 的 OKF `type` 值（空则按 `id` 首字母大写推导）。
    #[serde(rename = "type", default)]
    pub type_name: String,
    /// 语义说明：拼进提取 / 编译提示词的取值集，供模型判断该归入哪一类。
    #[serde(default)]
    pub hint: String,
}

/// 长期记忆（OKF 编译管道）配置。
///
/// 编译触发方式为**手动为主**：`auto_compile` 默认关闭，避免静默消耗 API 额度；
/// 记忆页用「待编译 N 个会话」提示（基于源文件 size/mtime 快路径比对，不花 token）。
#[derive(Debug, Clone, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct MemoryConfig {
    /// 是否启用记忆功能（关闭时不打开索引、不读盘、不编译）。
    pub enabled: bool,
    /// 会话结束后是否自动跑一次增量编译（默认关：避免静默消耗 API 额度）。
    pub auto_compile: bool,
    /// 编译所用供应商名（未配置 → 默认供应商）。
    pub provider: Option<String>,
    /// 编译所用模型（模型**引用 id**；未配置 → 默认模型）。
    pub model: Option<String>,
    /// 概念分类表（默认见 [`MemoryConfig::default_categories`]；为空或全非法时回退默认表）。
    pub categories: Vec<MemoryCategory>,
    /// 单次编译调用携带的记忆单元数上限（限制单次输出规模）。
    pub max_units_per_compile_call: usize,
    /// 单次提取调用携带的对话正文字符预算（超出则按消息边界分块多次提取）。
    pub raw_char_budget: usize,
    /// 归一化时单个工具结果的字符截断上限（工具输出是最大噪声源）。
    pub tool_result_chars: usize,
    /// 合并判定时的候选概念数上限（BM25 召回 top N）。
    pub judge_candidates: usize,
    /// 提取阶段的二遍自查（把已提取结果连同原文再喂一次，补漏；成本约 2×）。
    pub extract_self_critique: bool,
    /// **精选**（第二遍 LLM）：把抽出的候选单元筛成真正耐久的知识，并合并同主题项。
    /// 真实测试证明不做这步会严重过抽（2 个会话产出 126 条，其中 43 条是同一第三方库的碎片）。
    pub curate: bool,
    /// 精选调用的单元批大小。
    pub curate_units_per_call: usize,
    /// 丢弃 `confidence` 低于此值的单元（0 表示不过滤）。
    pub min_confidence: f32,
    /// 单会话最终产出单元数上限（0 表示不限）；精选之外的兜底。
    pub max_units_per_session: usize,
    /// 合并候选的**主题重叠度**下限（0 = 不过滤）。
    ///
    /// 用「标题+描述+标签」的词重叠（Jaccard）而非裸 BM25 分数：BM25 分数不可跨文档校准，
    /// 而重叠度对「只因都提到同一个项目名」天然不敏感 —— 真实数据上正是这个原因导致
    /// 「所有提到 agent-browser 的碎片全被合并进同一个概念」（29 个 tags / 9 个追加段）。
    pub candidate_min_overlap: f32,
    /// 单个概念的 tags 上限（合并时按「原有优先」保留，0 = 不限）。
    pub max_tags: usize,
    /// 单个概念的**追加段**上限；超出后把历史追加汇总为单一「补充」段（0 = 不限）。
    pub max_append_sections: usize,
    /// 正文末尾「相关」小节里的**概念互链**条数（0 = 不生成该小节）。
    ///
    /// 链接是**确定性**生成的（检索索引 + 确认目标文件存在），不依赖模型自造：
    /// 编译是单次调用、无工具，模型看不到 Bundle 里有哪些概念，它写的跨概念链接必然是猜的。
    pub related_concepts: usize,

    // ---- 以下两项是**会话侧注入**（决定模型在会话里能看到多少记忆），不是编译预算 ----
    /// 会话中是否注入「与当前用户消息相关的记忆」（见 `MemoryImpl::digest_for`）。
    ///
    /// 只影响模型看到什么，不改变编译产出。注入文本追加在**用户消息末尾**并以
    /// `【相关记忆】` 标记结尾，归一化时整体剥离 —— 因此**不会回流成新的记忆**。
    pub inject_memories: bool,
    /// 每次注入的相关记忆条数上限（0 = 不注入；命中不足时按实际条数）。
    pub injected_concepts: usize,
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            auto_compile: false,
            provider: None,
            model: None,
            categories: Self::default_categories(),
            max_units_per_compile_call: 20,
            raw_char_budget: 24_000,
            tool_result_chars: 1_200,
            judge_candidates: 8,
            extract_self_critique: false,
            curate: true,
            curate_units_per_call: 60,
            min_confidence: 0.6,
            max_units_per_session: 40,
            candidate_min_overlap: 0.25,
            max_tags: 8,
            max_append_sections: 3,
            related_concepts: 3,
            inject_memories: true,
            injected_concepts: 5,
        }
    }
}

impl MemoryConfig {
    /// 默认分类表：**顺序即引导强度**，信号强的分类排在 `fact` 之前。
    ///
    /// `hint` 会被注入**两处** —— 提取阶段的取值集（[`crate::prompt::extract_system`]）
    /// 与编译阶段的目录映射表（[`crate::types::CategoryTable::prompt_lines`]）——
    /// 因此它是配置里唯一直接作用于分类准确率的面。
    ///
    /// 真实数据审计（64 个概念）证明：把「操作步骤 / 环境限制 / 目录约定」笼统归入
    /// `fact` 会让它变成垃圾桶（占 55%），故拆出 `procedure` / `constraint` /
    /// `convention`，并把 `fact` 的 hint 收紧为「不属于以上分类」。
    /// 已删除的 `status`（临时状态、进度与待办）与「耐久记忆」预算口径自相矛盾：
    /// 这些内容会过期，现由提取提示词的硬门槛显式拒收。
    pub fn default_categories() -> Vec<MemoryCategory> {
        [
            ("decision", "decisions", "Decision", "一次性的选择及其理由"),
            ("preference", "preferences", "Preference", "用户稳定的倾向与习惯"),
            ("instruction", "instructions", "Instruction", "用户要求长期遵守的规则"),
            (
                "convention",
                "conventions",
                "Convention",
                "项目约定：目录/命名/流程/工具链的固定做法",
            ),
            (
                "procedure",
                "procedures",
                "Procedure",
                "可复用的操作步骤、命令序列或排查流程",
            ),
            (
                "constraint",
                "constraints",
                "Constraint",
                "环境或外部强加的硬限制：平台、版本、不可改的接口",
            ),
            ("fact", "concepts", "Fact", "项目里客观成立且不属于以上分类的知识"),
            ("relationship", "entities", "Entity", "命名实体及其属性与关联"),
            ("other", "concepts", "Other", "无法归入以上分类的其他记忆"),
        ]
        .into_iter()
        .map(|(id, dir, type_name, hint)| MemoryCategory {
            id: id.into(),
            dir: dir.into(),
            type_name: type_name.into(),
            hint: hint.into(),
        })
        .collect()
    }

    /// 生效的分类表：规范化配置中的分类，非法/为空时回退默认表。
    ///
    /// 规范化规则：
    /// - `id` 去空白并小写；为空的分类丢弃
    /// - `dir` 去首尾 `/`、`\` → `/`；丢弃 `.` / `..` 段；**只保留第一段**（OKF 概念为单层 slug）；
    ///   结果为空 ⇒ 概念直接放 `wiki/` 根
    /// - `type_name` 为空 ⇒ 按 `id` 首字母大写推导
    /// - 按 `id` 去重（保留先出现者）
    pub fn effective_categories(&self) -> Vec<MemoryCategory> {
        let mut out: Vec<MemoryCategory> = Vec::new();
        for raw in &self.categories {
            let id = raw.id.trim().to_lowercase();
            if id.is_empty() || out.iter().any(|c| c.id == id) {
                continue;
            }
            let dir = normalize_category_dir(&raw.dir);
            let type_name = {
                let t = raw.type_name.trim();
                if t.is_empty() {
                    capitalize(&id)
                } else {
                    t.to_string()
                }
            };
            out.push(MemoryCategory {
                id,
                dir,
                type_name,
                hint: raw.hint.trim().to_string(),
            });
        }
        if out.is_empty() {
            Self::default_categories()
        } else {
            out
        }
    }
}

/// 分类目录名规范化：统一分隔符、丢弃 `.` / `..` 段、只保留第一段。
fn normalize_category_dir(dir: &str) -> String {
    dir.replace('\\', "/")
        .split('/')
        .map(str::trim)
        .find(|seg| !seg.is_empty() && *seg != "." && *seg != "..")
        .unwrap_or("")
        .to_string()
}

/// 首字母大写（`fact` → `Fact`；非 ASCII 首字符原样保留）。
fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_map_nine_categories_to_eight_dirs() {
        let cfg = MemoryConfig::default();
        let cats = cfg.effective_categories();
        assert_eq!(cats.len(), 9);
        let mut dirs: Vec<String> = cats.iter().map(|c| c.dir.clone()).collect();
        dirs.sort();
        dirs.dedup();
        assert_eq!(
            dirs,
            vec![
                "concepts",
                "constraints",
                "conventions",
                "decisions",
                "entities",
                "instructions",
                "preferences",
                "procedures"
            ]
        );
        // fact / other 共用 concepts（8 个目录承载 9 个分类）
        for id in ["fact", "other"] {
            let c = cats.iter().find(|c| c.id == id).unwrap();
            assert_eq!(c.dir, "concepts", "{id} 应落在 concepts/");
        }
        // relationship 的 type 改为 Entity（原名与「实体」语义不符）
        let rel = cats.iter().find(|c| c.id == "relationship").unwrap();
        assert_eq!(rel.type_name, "Entity");
        // status 已删除：临时状态/进度/待办由提取提示词硬门槛拒收
        assert!(
            cats.iter().all(|c| c.id != "status"),
            "status 不再是默认分类"
        );
    }

    /// 新增三条分类必须带 hint —— hint 是提取/编译两侧唯一的分类引导面。
    #[test]
    fn new_categories_carry_hints() {
        let cats = MemoryConfig::default().effective_categories();
        for id in ["convention", "procedure", "constraint"] {
            let c = cats.iter().find(|c| c.id == id).expect("应存在新分类");
            assert!(!c.hint.is_empty(), "{id} 缺 hint");
            assert!(!c.type_name.is_empty(), "{id} 缺 type");
        }
        // 顺序要求：信号强的分类排在 fact 之前，否则 fact 继续吸走它们
        let pos = |id: &str| cats.iter().position(|c| c.id == id).unwrap();
        for id in ["convention", "procedure", "constraint"] {
            assert!(pos(id) < pos("fact"), "{id} 应排在 fact 之前");
        }
    }

    #[test]
    fn ids_are_lowercased_and_deduped() {
        let cfg = MemoryConfig {
            categories: vec![
                MemoryCategory {
                    id: "  Decision ".into(),
                    dir: "decisions".into(),
                    type_name: "Decision".into(),
                    hint: String::new(),
                },
                MemoryCategory {
                    id: "decision".into(),
                    dir: "other-dir".into(),
                    type_name: "Dup".into(),
                    hint: String::new(),
                },
            ],
            ..MemoryConfig::default()
        };
        let cats = cfg.effective_categories();
        assert_eq!(cats.len(), 1, "重复 id 只保留先出现者");
        assert_eq!(cats[0].id, "decision");
        assert_eq!(cats[0].dir, "decisions");
    }

    #[test]
    fn dir_is_normalized_to_single_segment() {
        let cases = [
            ("a/b/c", "a"),
            ("/leading/", "leading"),
            ("..", ""),
            (".", ""),
            ("", ""),
            ("c:\\win\\path", "c:"),
            ("  spaced  ", "spaced"),
        ];
        for (input, expected) in cases {
            let cfg = MemoryConfig {
                categories: vec![MemoryCategory {
                    id: "x".into(),
                    dir: input.into(),
                    type_name: "X".into(),
                    hint: String::new(),
                }],
                ..MemoryConfig::default()
            };
            assert_eq!(cfg.effective_categories()[0].dir, expected, "input={input:?}");
        }
    }

    #[test]
    fn empty_type_is_derived_from_id() {
        let cfg = MemoryConfig {
            categories: vec![MemoryCategory {
                id: "runbook".into(),
                dir: "runbooks".into(),
                type_name: "   ".into(),
                hint: String::new(),
            }],
            ..MemoryConfig::default()
        };
        assert_eq!(cfg.effective_categories()[0].type_name, "Runbook");
    }

    #[test]
    fn empty_or_all_invalid_config_falls_back_to_defaults() {
        for categories in [Vec::new(), vec![MemoryCategory {
            id: "   ".into(),
            dir: "x".into(),
            type_name: "X".into(),
            hint: String::new(),
        }]] {
            let cfg = MemoryConfig {
                categories,
                ..MemoryConfig::default()
            };
            assert_eq!(
                cfg.effective_categories().len(),
                MemoryConfig::default_categories().len(),
                "非法配置应回退默认表"
            );
        }
    }

    #[test]
    fn hint_is_trimmed() {
        let cfg = MemoryConfig {
            categories: vec![MemoryCategory {
                id: "fact".into(),
                dir: "concepts".into(),
                type_name: "Fact".into(),
                hint: "  客观知识  ".into(),
            }],
            ..MemoryConfig::default()
        };
        assert_eq!(cfg.effective_categories()[0].hint, "客观知识");
    }

    #[test]
    fn deserializes_frontend_json_shape() {
        let json = r#"{
            "enabled": false,
            "autoCompile": true,
            "model": "deepseek-chat",
            "extractSelfCritique": true,
            "categories": [
                { "id": "note", "dir": "notes", "type": "Note", "hint": "随手记" }
            ]
        }"#;
        let cfg: MemoryConfig = serde_json::from_str(json).unwrap();
        assert!(!cfg.enabled);
        assert!(cfg.auto_compile);
        assert_eq!(cfg.model.as_deref(), Some("deepseek-chat"));
        assert!(cfg.extract_self_critique);
        assert_eq!(cfg.categories.len(), 1);
        assert_eq!(cfg.categories[0].type_name, "Note");
        // 未给出的字段走默认
        assert_eq!(cfg.judge_candidates, 8);
    }

    /// 缺失 / 空对象都要能解析（前端老版本配置不含 memory 段）。
    #[test]
    fn deserializes_when_section_absent_or_empty() {
        let cfg: MemoryConfig = serde_json::from_str("{}").unwrap();
        assert!(cfg.enabled, "默认启用");
        assert!(!cfg.auto_compile, "默认不自动增量");
        assert_eq!(cfg.categories.len(), 9);
    }

    #[test]
    fn serializes_type_as_type_key() {
        let cat = MemoryCategory {
            id: "fact".into(),
            dir: "concepts".into(),
            type_name: "Fact".into(),
            hint: "h".into(),
        };
        let json = serde_json::to_string(&cat).unwrap();
        assert!(json.contains(r#""type":"Fact""#), "type 必须以 type 为键: {json}");
        assert!(!json.contains("typeName"), "{json}");
    }
}
