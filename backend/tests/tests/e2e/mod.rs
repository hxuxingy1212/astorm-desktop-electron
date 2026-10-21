//! e2e 测试模块：基于 `.env` 真实配置（LLM 密钥 / 网络），默认 `#[ignore]` 跳过。
//!
//! 手动运行：`cargo test -p astorm-tests --test e2e_tests -- --ignored`
//! 前置：项目根 `.env` 配置 `LLM_BASE_URL` / `LLM_MODEL` / `LLM_API_KEY`。
//!
//! 新增 e2e 测试：在本目录新建文件 + 此处声明 `pub mod <name>;`，
//! 测试函数必须标记 `#[ignore]`，不参与默认全量测试。

pub mod react_loop;
