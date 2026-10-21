//! 命令行字符串拆分工具（用于 --direct 直接执行模式与静态预检的 token 化）。

/// 把一条命令字符串按 shell 规则拆成参数序列。
///
/// 规则（Windows cmd 风格与常见 shell 风格的并集，保守处理）：
/// - 空白（空格/制表符）作为分隔符
/// - 双引号包裹一段，内部空格不拆分；`""` 表示为空参数
/// - 单引号按原样保留（不做 shell 语义，仅阻止拆分）
/// - 反斜杠转义仅对双引号内的 `\"` 生效，其余按字面保留
///
/// 该工具只用于启发式分析与直接执行，不追求与真实 shell 完全一致。
pub fn split_command_line(line: &str) -> Vec<String> {
    let mut args: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut in_double = false;
    let mut in_single = false;
    let mut has_token = false;

    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if !in_single => {
                in_double = !in_double;
                has_token = true;
            }
            '\'' if !in_double => {
                in_single = !in_single;
                has_token = true;
            }
            '\\' if in_double => {
                // 双引号内的 \" 转义
                if let Some(&next) = chars.peek() {
                    if next == '"' || next == '\\' {
                        cur.push(next);
                        chars.next();
                        has_token = true;
                        continue;
                    }
                }
                cur.push('\\');
                has_token = true;
            }
            c if c.is_whitespace() && !in_double && !in_single => {
                if has_token {
                    args.push(std::mem::take(&mut cur));
                    has_token = false;
                }
            }
            other => {
                cur.push(other);
                has_token = true;
            }
        }
    }
    if has_token {
        args.push(cur);
    }
    args
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 简单参数() {
        assert_eq!(split_command_line("a b   c"), vec!["a", "b", "c"]);
    }

    #[test]
    fn 双引号包裹空白() {
        assert_eq!(
            split_command_line("echo \"hello world\" x"),
            vec!["echo", "hello world", "x"]
        );
    }

    #[test]
    fn 单引号不拆分() {
        assert_eq!(split_command_line("echo 'a b'"), vec!["echo", "a b"]);
    }

    #[test]
    fn 空字符串为空参数() {
        assert_eq!(split_command_line("a \"\" b"), vec!["a", "", "b"]);
    }

    #[test]
    fn 引号内转义() {
        assert_eq!(split_command_line("echo \"a\\\"b\""), vec!["echo", "a\"b"]);
    }

    #[test]
    fn 空输入() {
        assert!(split_command_line("").is_empty());
    }

    #[test]
    fn windows_风格重定向保留() {
        assert_eq!(
            split_command_line("cmd /C echo hi > D:\\out.txt"),
            vec!["cmd", "/C", "echo", "hi", ">", "D:\\out.txt"]
        );
    }
}