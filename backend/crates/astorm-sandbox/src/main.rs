//! sbox：astorm-sandbox 命令行入口。

fn main() -> std::process::ExitCode {
    // 中文 Windows 控制台默认 GBK，切换 UTF-8 避免输出乱码
    astorm_sandbox::sandbox::enable_utf8_console();
    astorm_sandbox::cli::main()
}