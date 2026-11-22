//! Очистка docfx-разметки Learn перед разбором Markdown:
//! * `[!INCLUDE [x](../includes/y.md)]` → содержимое файла (без его front matter, вложенность ≤ 3), иначе удаляется;
//! * `> [!NOTE]` / `[!TIP]` / `[!IMPORTANT]` / `[!WARNING]` / `[!CAUTION]` → «Примечание:», «Совет:»…;
//! * строки `:::image …:::`, `:::zone …:::`, `::: moniker …`, `:::row:::` и блоки `:::image type="complex"` … `:::image-end:::` — удаляются;
//! * `[!div …]`, `[!VIDEO …]`, HTML-комментарии — удаляются.

use regex::Regex;
use std::path::Path;
use std::sync::LazyLock;

static INCLUDE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\[!INCLUDE\s*\[[^\]]*\]\(\s*<?([^)\s>]+)>?\s*\)\s*\]").unwrap());
static ALERT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\[!(NOTE|TIP|IMPORTANT|WARNING|CAUTION)\]").unwrap());
static DIV_VIDEO: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\[!(div|VIDEO)[^\]]*\]").unwrap());
static COMMENT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<!--.*?-->").unwrap());
static INLINE_TRIPLE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r":::[a-zA-Z][^\n]*?:::").unwrap());

fn alert_label(kind: &str) -> &'static str {
    match kind.to_ascii_uppercase().as_str() {
        "NOTE" => "Примечание:",
        "TIP" => "Совет:",
        "IMPORTANT" => "Важно:",
        "WARNING" => "Предупреждение:",
        _ => "Осторожно:",
    }
}

/// Убрать строки `:::…` и сложные картинки целиком.
fn strip_triple_colon(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_complex = false;
    for line in text.split_inclusive('\n') {
        let t = line.trim_start().trim_start_matches('>').trim_start();
        if in_complex {
            if t.starts_with(":::image-end") {
                in_complex = false;
            }
            continue;
        }
        if t.starts_with(":::") {
            if t.starts_with(":::image") && t.contains("type=\"complex\"") {
                in_complex = !t.contains(":::image-end");
            }
            continue;
        }
        out.push_str(&INLINE_TRIPLE.replace_all(line, ""));
    }
    out
}

/// `resolve(path_from_include)` возвращает текст включаемого файла (или `None`).
pub fn preprocess_with(text: &str, resolve: &mut dyn FnMut(&str, usize) -> Option<String>, depth: usize) -> String {
    let text = COMMENT.replace_all(text, "");
    let text = INCLUDE.replace_all(&text, |c: &regex::Captures| {
        if depth >= 3 {
            return String::new();
        }
        match resolve(&c[1], depth) {
            Some(inc) => {
                let (_, body) = crate::frontmatter::split(&inc);
                format!("\n{}\n", preprocess_with(body, resolve, depth + 1).trim())
            }
            None => String::new(),
        }
    });
    let text = ALERT.replace_all(&text, |c: &regex::Captures| alert_label(&c[1]).to_string());
    let text = DIV_VIDEO.replace_all(&text, "");
    strip_triple_colon(&text)
}

/// Предобработка файла `file` (пути include — относительно его каталога; вложенные — относительно корня `root`).
pub fn preprocess_file(text: &str, file: &Path, root: &Path) -> String {
    let base = file.parent().unwrap_or(root).to_path_buf();
    let mut stack: Vec<std::path::PathBuf> = vec![base];
    let mut resolve = |rel: &str, depth: usize| -> Option<String> {
        let rel = rel.split(['#', '?']).next().unwrap_or(rel);
        let dir = stack.get(depth).cloned().unwrap_or_else(|| stack[0].clone());
        let p = if let Some(abs) = rel.strip_prefix('~').or_else(|| rel.strip_prefix('/')) {
            root.join(abs.trim_start_matches('/'))
        } else {
            dir.join(rel)
        };
        let content = std::fs::read_to_string(&p).ok()?;
        let parent = p.parent().map(Path::to_path_buf).unwrap_or_else(|| root.to_path_buf());
        stack.truncate(depth + 1);
        stack.push(parent);
        Some(content)
    };
    preprocess_with(text, &mut resolve, 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pre(t: &str) -> String {
        preprocess_with(t, &mut |_, _| None, 0)
    }

    #[test]
    fn alerts_become_russian_labels() {
        let out = pre("> [!NOTE]\n> Run as admin.\n\n> [!warning]\n> Danger.\n> [!TIP]\n> [!IMPORTANT]\n> [!CAUTION]");
        assert!(out.contains("> Примечание:\n> Run as admin."));
        assert!(out.contains("> Предупреждение:"));
        assert!(out.contains("Совет:") && out.contains("Важно:") && out.contains("Осторожно:"));
        assert!(!out.contains("[!"));
    }

    #[test]
    fn triple_colon_and_misc_removed() {
        let t = "Intro\n:::image type=\"content\" source=\"a.png\" alt-text=\"x\":::\n::: zone pivot=\"win\"\nInside zone\n::: zone-end\n\
                 :::image type=\"complex\" source=\"b.png\" alt-text=\"y\":::\nLong description\n:::image-end:::\nAfter\n\
                 > [!div class=\"nextstepaction\"]\n> [Next](x.md)\n<!-- hidden\ncomment -->Text :::image source=\"c.png\"::: tail\n[!VIDEO https://x]";
        let out = pre(t);
        assert!(out.contains("Intro") && out.contains("Inside zone") && out.contains("After"));
        assert!(!out.contains(":::") && !out.contains("Long description") && !out.contains("hidden"));
        assert!(!out.contains("[!div") && !out.contains("VIDEO"));
        assert!(out.contains("Text  tail"));
    }

    #[test]
    fn includes_inlined_from_files() {
        let dir = tempfile::tempdir().unwrap();
        let inc = dir.path().join("includes");
        std::fs::create_dir_all(&inc).unwrap();
        std::fs::write(inc.join("a.md"), "---\nms.topic: include\n---\nIncluded A [!INCLUDE [b](b.md)]").unwrap();
        std::fs::write(inc.join("b.md"), "nested B").unwrap();
        let file = dir.path().join("doc.md");
        let t = "Start\n[!INCLUDE [a](includes/a.md)]\n[!INCLUDE[missing](includes/none.md)]\nEnd";
        let out = preprocess_file(t, &file, dir.path());
        assert!(out.contains("Included A"), "{out}");
        assert!(out.contains("nested B"), "вложенный include относительно включаемого файла: {out}");
        assert!(!out.contains("ms.topic") && !out.contains("[!INCLUDE") && !out.contains("missing"));
        assert!(out.contains("End"));
    }

    #[test]
    fn include_depth_is_limited() {
        let mut n = 0;
        let out = preprocess_with("[!INCLUDE [x](x.md)]", &mut |_, _| { n += 1; Some("again [!INCLUDE [x](x.md)]".into()) }, 0);
        assert_eq!(n, 3);
        assert_eq!(out.matches("again").count(), 3);
    }
}
