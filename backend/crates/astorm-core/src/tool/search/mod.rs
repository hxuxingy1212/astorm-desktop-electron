//! 搜索工具集（独立目录）。
//!
//! 对外只暴露统一的 [`SearchTool`]（工具名 `search`，见 [`search_tool`]）；
//! `websearch` / `webfetch` / `github_trending` / `arxiv_search` / `history_search`
//! 为该目录内部实现，仅经 `search` 的 `tool` 参数分发调用，不单独注册到 `ToolRegistry`。

pub mod arxiv_search;
pub mod github_trending;
pub mod history_search;
pub mod renderer;
pub mod search_tool;
pub mod webfetch;
pub mod websearch;

pub use renderer::WebRender;
pub use search_tool::SearchTool;