//! 终端输出清理：去掉 ANSI 颜色/控制序列和回车，便于在气泡中显示。

/// 移除常见 ANSI CSI / OSC 转义序列和 \r，保留普通 UTF-8 文本。
pub fn clean_line(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' {
            match chars.peek() {
                Some('[') => {
                    // CSI: ESC [ ... final byte (0x40..=0x7E)
                    chars.next();
                    for c in chars.by_ref() {
                        if c.is_ascii() && (0x40..=0x7E).contains(&(c as u8)) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    // OSC: ESC ] ... BEL（常见于终端标题/超链接）
                    chars.next();
                    for c in chars.by_ref() {
                        if c == '\u{7}' {
                            break;
                        }
                    }
                }
                _ => {}
            }
        } else if ch != '\r' {
            out.push(ch);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_csi_color_codes() {
        assert_eq!(clean_line("\u{1b}[1;32m✓ done\u{1b}[0m"), "✓ done");
    }

    #[test]
    fn strips_carriage_return() {
        assert_eq!(clean_line("progress 10%\r"), "progress 10%");
    }

    #[test]
    fn keeps_plain_text() {
        assert_eq!(clean_line("Packages: +14"), "Packages: +14");
    }
}
