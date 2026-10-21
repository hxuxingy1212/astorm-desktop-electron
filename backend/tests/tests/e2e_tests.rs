//! e2e 测试入口：`tests/e2e/` 目录下的测试。
//!
//! 依赖项目根 `.env` 的真实配置（LLM 密钥 / 网络），全部标记 `#[ignore]`，
//! 不参与默认全量测试；手动运行：
//!
//! ```bash
//! cargo test -p astorm-tests --test e2e_tests -- --ignored
//! ```

mod e2e;
