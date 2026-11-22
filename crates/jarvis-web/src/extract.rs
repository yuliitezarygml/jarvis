//! Извлечение содержимого: кодировка (`encoding_rs`), основной текст (readability, `dom_smoothie`),
//! HTML → Markdown (`htmd`), PDF → текст (`pdf-extract`).

use dom_query::Document;
use encoding_rs::{Encoding, UTF_8, WINDOWS_1251};

use crate::text::{collapse_ws, tidy};

/// Меньше этого после readability — считаем, что статью выделить не удалось, берём весь `<body>`.
const MIN_READABLE_CHARS: usize = 200;

/// Теги, которые в Markdown для модели не нужны (картинки, формы, медиа, скрипты).
const SKIP_TAGS: &[&str] = &[
    "head", "script", "style", "noscript", "template", "img", "picture", "svg", "canvas", "video", "audio",
    "iframe", "object", "embed", "form", "button", "input", "select", "textarea", "dialog",
];

/// Мусор вокруг основного содержимого — вырезается в запасном пути (когда readability не справилась).
const BOILERPLATE: &str = "script, style, noscript, template, nav, header, footer, aside, form, svg, iframe, \
    [role=navigation], [role=banner], [role=contentinfo], [aria-hidden=true], .cookie, .cookies, #cookie-banner";

#[derive(Debug, Clone, PartialEq)]
pub struct Extracted {
    pub title: String,
    pub markdown: String,
    /// true — текст выделен readability-алгоритмом; false — запасной путь (весь body без навигации).
    pub readable: bool,
}

/// HTML-страница → заголовок и Markdown основного содержимого. `url` — для абсолютных ссылок.
pub fn html_to_markdown(html: &str, url: Option<&str>) -> Extracted {
    let doc_url = url.filter(|u| url::Url::parse(u).is_ok());
    let fallback_title = page_title(html);

    let cfg = dom_smoothie::Config { max_elements_to_parse: 0, ..Default::default() };
    if let Ok(mut r) = dom_smoothie::Readability::new(html, doc_url, Some(cfg))
        && let Ok(article) = r.parse()
    {
        let md = to_markdown(&article.content);
        if md.chars().count() >= MIN_READABLE_CHARS {
            let title = collapse_ws(&article.title);
            return Extracted {
                title: if title.is_empty() { fallback_title } else { title },
                markdown: md,
                readable: true,
            };
        }
    }

    let doc = Document::from(html);
    doc.select(BOILERPLATE).remove();
    let main = ["main", "article", "[role=main]", "body"]
        .iter()
        .map(|s| doc.select(s).first())
        .find(|s| s.exists());
    let inner = match main {
        Some(m) => m.html().to_string(),
        None => doc.html().to_string(),
    };
    Extracted { title: fallback_title, markdown: to_markdown(&inner), readable: false }
}

/// HTML → Markdown без картинок/форм/скриптов.
pub fn to_markdown(html: &str) -> String {
    let options = htmd::options::Options {
        bullet_list_marker: htmd::options::BulletListMarker::Dash,
        hr_style: htmd::options::HrStyle::Dashes,
        ul_bullet_spacing: 1,
        ol_number_spacing: 1,
        ..Default::default()
    };
    let conv = htmd::HtmlToMarkdown::builder().options(options).skip_tags(SKIP_TAGS.to_vec()).build();
    tidy(&conv.convert(html).unwrap_or_default())
}

/// `<title>`, затем `og:title`, затем первый `<h1>`.
pub fn page_title(html: &str) -> String {
    let doc = Document::from(html);
    let title = collapse_ws(&doc.select("head title, title").first().text());
    if !title.is_empty() {
        return title;
    }
    if let Some(og) = doc.select(r#"meta[property="og:title"]"#).first().attr("content") {
        let og = collapse_ws(&og);
        if !og.is_empty() {
            return og;
        }
    }
    collapse_ws(&doc.select("h1").first().text())
}

/// PDF → текст. `pdf-extract` на битых файлах может паниковать — перехватываем.
pub fn pdf_to_text(bytes: &[u8]) -> Result<String, String> {
    match std::panic::catch_unwind(|| pdf_extract::extract_text_from_mem(bytes)) {
        Ok(Ok(s)) => Ok(tidy(&s)),
        Ok(Err(e)) => Err(e.to_string()),
        Err(_) => Err("сбой разбора PDF".into()),
    }
}

/// Декодировать текст: BOM → `charset` из Content-Type → `<meta charset>` (для HTML) → UTF-8,
/// а если UTF-8 явно не подходит — windows-1251 (умолчание WHATWG для русской локали).
pub fn decode_text(bytes: &[u8], content_type: Option<&str>, is_html: bool) -> String {
    if let Some((enc, bom_len)) = Encoding::for_bom(bytes) {
        return enc.decode_without_bom_handling(&bytes[bom_len..]).0.into_owned();
    }
    let header = content_type.and_then(charset_param);
    let meta = if is_html && header.is_none() { meta_charset(bytes) } else { None };
    if let Some(label) = header.or(meta) {
        if let Some(enc) = Encoding::for_label(label.trim().as_bytes()) {
            // `utf-16`/`replacement` без BOM у ASCII-совместимого документа означает UTF-8 (WHATWG).
            return enc.output_encoding().decode_without_bom_handling(bytes).0.into_owned();
        }
    }
    let (s, had_errors) = UTF_8.decode_without_bom_handling(bytes);
    if !had_errors {
        return s.into_owned();
    }
    let bad = s.chars().filter(|&c| c == '\u{FFFD}').count();
    if bad * 100 > s.chars().count().max(1) {
        WINDOWS_1251.decode_without_bom_handling(bytes).0.into_owned()
    } else {
        s.into_owned()
    }
}

/// `text/html; charset="windows-1251"` → `windows-1251`.
fn charset_param(ct: &str) -> Option<String> {
    ct.split(';').skip(1).find_map(|p| {
        let (k, v) = p.split_once('=')?;
        k.trim().eq_ignore_ascii_case("charset").then(|| v.trim().trim_matches(['"', '\'']).to_string())
    })
}

/// Предварительный просмотр первых 4 КБ: `<meta charset=…>` или `<meta http-equiv … content="…; charset=…">`.
fn meta_charset(bytes: &[u8]) -> Option<String> {
    let head = bytes[..bytes.len().min(4096)].to_ascii_lowercase();
    let find = |hay: &[u8], needle: &[u8]| hay.windows(needle.len()).position(|w| w == needle);
    let mut i = 0;
    while let Some(pos) = find(&head[i..], b"<meta") {
        let start = i + pos;
        let end = find(&head[start..], b">").map_or(head.len(), |e| start + e);
        let tag = &head[start..end];
        if let Some(c) = find(tag, b"charset") {
            let mut j = c + "charset".len();
            while j < tag.len() && tag[j].is_ascii_whitespace() {
                j += 1;
            }
            if j < tag.len() && tag[j] == b'=' {
                j += 1;
                while j < tag.len() && (tag[j].is_ascii_whitespace() || tag[j] == b'"' || tag[j] == b'\'') {
                    j += 1;
                }
                let val: Vec<u8> = tag[j..]
                    .iter()
                    .copied()
                    .take_while(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b':' | b'.'))
                    .collect();
                if !val.is_empty() {
                    return String::from_utf8(val).ok();
                }
            }
        }
        i = end.max(start + 1);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn charset_sources() {
        assert_eq!(charset_param("text/html; Charset=\"windows-1251\"").as_deref(), Some("windows-1251"));
        assert_eq!(charset_param("text/html").as_deref(), None);
        assert_eq!(meta_charset(b"<html><head><META CHARSET='koi8-r'>").as_deref(), Some("koi8-r"));
        assert_eq!(
            meta_charset(br#"<meta http-equiv="Content-Type" content="text/html; charset=windows-1251">"#).as_deref(),
            Some("windows-1251")
        );
        assert_eq!(meta_charset(b"<meta name=viewport content=x>").as_deref(), None);
    }

    #[test]
    fn decodes_cp1251() {
        let (bytes, _, _) = WINDOWS_1251.encode("<p>Привет, сэр</p>");
        // Без указания кодировки: UTF-8 не подходит → windows-1251.
        assert_eq!(decode_text(&bytes, None, true), "<p>Привет, сэр</p>");
        // Из заголовка.
        assert_eq!(decode_text(&bytes, Some("text/html; charset=windows-1251"), true), "<p>Привет, сэр</p>");
        // UTF-8 с BOM.
        let mut bom = vec![0xEF, 0xBB, 0xBF];
        bom.extend_from_slice("Джарвис".as_bytes());
        assert_eq!(decode_text(&bom, Some("text/plain; charset=windows-1251"), false), "Джарвис");
    }

    #[test]
    fn markdown_skips_images_and_scripts() {
        let md = to_markdown("<h2>Заголовок</h2><p>Текст <a href='https://a.b/'>ссылка</a></p><img src=x.png><script>evil()</script><ul><li>раз</li></ul>");
        assert!(md.contains("## Заголовок"));
        assert!(md.contains("[ссылка](https://a.b/)"));
        assert!(md.contains("- раз"));
        assert!(!md.contains("x.png") && !md.contains("evil"));
    }
}
