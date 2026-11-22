//! Очистка вывода ConPTY от VT/ANSI-последовательностей: цвета, заголовок окна (OSC), перемещения курсора.

/// Текст без escape-последовательностей, строки через `\n`, хвостовые пробелы срезаны.
/// `\r` без `\n` — возврат каретки: следующий текст перезаписывает строку (индикаторы прогресса).
/// `ESC[nC` (ConPTY так сжимает пробелы) → n пробелов; `ESC[row;colH` — перенос строки, если строка не пуста.
pub fn strip_vt(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut line = String::new();
    let mut pending_cr = false;
    let mut chars = input.chars().peekable();

    let flush = |out: &mut String, line: &mut String| {
        out.push_str(line.trim_end());
        out.push('\n');
        line.clear();
    };

    while let Some(c) = chars.next() {
        match c {
            '\x1b' => match chars.peek().copied() {
                Some('[') => {
                    chars.next();
                    let mut params = String::new();
                    let mut fin = None;
                    for d in chars.by_ref() {
                        if ('\x40'..='\x7e').contains(&d) {
                            fin = Some(d);
                            break;
                        }
                        params.push(d);
                    }
                    match fin {
                        Some('C') => {
                            let n: usize = params.parse().unwrap_or(1).min(500);
                            line.extend(std::iter::repeat_n(' ', n));
                        }
                        Some('H') | Some('f') if !line.trim().is_empty() => flush(&mut out, &mut line),
                        Some('E') if !line.trim().is_empty() => flush(&mut out, &mut line),
                        _ => {}
                    }
                }
                Some(']') | Some('P') | Some('X') | Some('^') | Some('_') => {
                    // OSC/DCS/…: до BEL или ST (ESC \)
                    chars.next();
                    while let Some(d) = chars.next() {
                        if d == '\x07' {
                            break;
                        }
                        if d == '\x1b' {
                            if chars.peek() == Some(&'\\') {
                                chars.next();
                            }
                            break;
                        }
                    }
                }
                Some(_) => {
                    chars.next();
                }
                None => {}
            },
            '\r' => {
                if chars.peek() != Some(&'\n') {
                    pending_cr = true;
                }
            }
            '\n' => {
                pending_cr = false;
                flush(&mut out, &mut line);
            }
            '\x08' => {
                line.pop();
            }
            '\t' => line.push('\t'),
            c if c.is_control() => {}
            c => {
                if pending_cr {
                    line.clear();
                    pending_cr = false;
                }
                line.push(c);
            }
        }
    }
    out.push_str(line.trim_end());
    // Хвостовые пустые строки не нужны.
    let trimmed = out.trim_end_matches('\n').len();
    out.truncate(trimmed);
    out
}

/// Последние `n` строк.
pub fn tail_lines(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_colors_title_and_cursor() {
        let raw = "\x1b[?25l\x1b[2J\x1b[m\x1b[H\x1b]0;C:\\WINDOWS\\system32\\cmd.exe\x07\x1b[?25hпривет\r\n\x1b[32mok\x1b[0m\r\n";
        assert_eq!(strip_vt(raw), "привет\nok");
    }

    #[test]
    fn spaces_and_carriage_return() {
        assert_eq!(strip_vt("a\x1b[3Cb"), "a   b");
        assert_eq!(strip_vt("10%\r50%\r100%\r\ndone"), "100%\ndone");
        assert_eq!(strip_vt("abc\x08\x08x"), "ax");
        assert_eq!(strip_vt("line1\x1b[2;1Hline2"), "line1\nline2");
        assert_eq!(strip_vt("x\x1b]0;title\x1b\\y"), "xy");
    }

    #[test]
    fn tail() {
        assert_eq!(tail_lines("a\nb\nc", 2), "b\nc");
        assert_eq!(tail_lines("a", 5), "a");
    }
}
