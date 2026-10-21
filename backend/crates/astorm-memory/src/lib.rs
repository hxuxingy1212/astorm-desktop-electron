//! astorm-memory：OKF v0.2 长期记忆编译管道（**独立 crate，不依赖 astorm-core**）。
//!
//! 把会话 JSONL 提炼、编译、合并为符合 OKF v0.2 规范的知识库 Bundle。三层架构遵循
//! 「`raw/` 不可变来源真相 → LLM 编译器 → `wiki/` 编译产物」：**编译器只从 `raw/` 读取，
//! 不从已有 wiki 文本编译**，以防知识漂移。
//!
//! 两种使用方式：
//! - **lib**：应用集成（`astorm-core` 门面把应用设定解析为本 crate 的配置与输入后调用）
//! - **bin**：`okf-agent` CLI（独立于 Electron 应用，可对任意会话目录运行）
//!
//! ```text
//! JSONL 文件
//!   → [Parser] 归一化为 Markdown，写入 raw/sessions/
//!   → [Extractor] LLM 提取原子记忆 → Vec<MemoryUnit>
//!   → [Compiler] LLM 组装为 OKF Concept → ConceptDocument
//!   → [Merger] 与已有 Concept 语义比对 → Upsert 决策
//!   → [Writer] 写入 wiki/，重建 index.md，追加 log.md
//! ```
//!
//! 全管道仅 **3 个 LLM 调用点**（提取 / 编译 / 合并判定），其余阶段纯确定性；因此可复现、
//! 可离线单测（注入 [`llm::MemoryLlm`] 的脚本实现），且支持整体取消。


/// 骨架占位：编译期自检 lib ↔ llm-provider 链接（真实模块随后迁入）。
pub fn crate_ready() -> bool {
    true
}

pub mod concept;
pub mod config;
pub mod paths;
pub mod types;
pub mod state;
pub mod writer;
pub mod validate;
pub mod index;
pub mod parser;
pub mod pipeline;
pub mod llm;
pub mod prompt;
pub mod extract;
pub mod compile;
pub mod merge;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crate_builds() {
        assert!(crate_ready());
    }
}
