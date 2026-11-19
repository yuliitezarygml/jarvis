use super::words;
use dom_query::Document;
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

fn sentence(out: &mut String) {
    let len = out.trim_end().len();
    out.truncate(len);
    if !out.is_empty() && !out.ends_with(['.', '!', '?', '…', ':', ';', ',']) {
        out.push('.');
    }
    out.push(' ');
}

fn row_sentence(out: &mut String, headers: &[String], cells: &[String]) {
    for (index, value) in cells.iter().enumerate() {
        if index > 0 {
            out.push_str(", ")
        }
        if let Some(label) = headers.get(index).filter(|s| !s.is_empty()) {
            out.push_str(label);
            out.push_str(": ");
        }
        out.push_str(value);
    }
    sentence(out);
}

pub(super) fn html(src: &str) -> String {
    let doc = Document::fragment(src);
    doc.select("script,style,template,head").remove();
    for code in doc.select("code").iter() {
        code.set_text(&words::literal(&code.text()));
    }
    for table in doc.select("table").iter().rev() {
        let mut spoken = String::new();
        let mut headers = Vec::new();
        for row in table.select("tr").iter() {
            let cells: Vec<_> = row
                .children()
                .filter("th,td")
                .iter()
                .map(|cell| cell.formatted_text().trim().to_owned())
                .collect();
            if headers.is_empty() && !row.select("th").is_empty() && row.select("td").is_empty() {
                headers = cells
            } else {
                row_sentence(&mut spoken, &headers, &cells)
            }
        }
        if spoken.is_empty() {
            spoken = headers.join(", ")
        }
        table.set_text(&spoken);
    }
    words::prose(&doc.formatted_text())
}

// Normalize backslash-delimited LaTeX before CommonMark consumes its escapes.
// Code spans, literal blocks, and existing dollar math keep their original bytes.
fn tex_delimiters(src: &str) -> String {
    if !src.contains("\\(") && !src.contains("\\[") {
        return src.into();
    }
    let mut protected = Vec::new();
    for (event, range) in Parser::new_ext(src, Options::ENABLE_MATH).into_offset_iter() {
        if matches!(
            event,
            Event::Code(_)
                | Event::Start(Tag::CodeBlock(_))
                | Event::InlineMath(_)
                | Event::DisplayMath(_)
        ) {
            protected.push(range);
        }
    }
    let mut pos = 0;
    let mut result = String::new();
    for range in protected {
        if range.start < pos {
            continue;
        }
        result.push_str(&words::legacy_tex(&src[pos..range.start]));
        result.push_str(&src[range.clone()]);
        pos = range.end;
    }
    result.push_str(&words::legacy_tex(&src[pos..]));
    result
}

pub(super) fn render(src: &str) -> String {
    let src = tex_delimiters(src);
    let mut out = String::new();
    let mut text = String::new();
    let mut html_block = None::<String>;
    let mut code = false;
    let mut headers = Vec::new();
    let mut row = Vec::new();
    let mut head = false;
    let mut in_cell = false;
    let mut cell = String::new();
    let mut hidden_tag = None::<String>;
    let mut rows = 0;
    for (event, range) in Parser::new_ext(
        &src,
        Options::ENABLE_TABLES | Options::ENABLE_MATH | Options::ENABLE_STRIKETHROUGH,
    )
    .into_offset_iter()
    {
        let dest = if in_cell { &mut cell } else { &mut out };
        match event {
            Event::Start(Tag::HtmlBlock) => html_block = Some(String::new()),
            Event::Html(h) if html_block.is_some() => html_block.as_mut().unwrap().push_str(&h),
            Event::End(TagEnd::HtmlBlock) => {
                dest.push_str(&html(&html_block.take().unwrap_or_default()));
                sentence(dest);
            }
            Event::InlineHtml(tag) => {
                let lower = tag.to_ascii_lowercase();
                if let Some(hidden) = &hidden_tag {
                    if lower.starts_with(&format!("</{hidden}>")) {
                        hidden_tag = None;
                    }
                } else if let Some(name) = ["script", "style", "template"].iter().find(|name| {
                    lower.starts_with(&format!("<{name}>"))
                        || lower.starts_with(&format!("<{name} "))
                }) {
                    hidden_tag = Some(name.to_string());
                } else if lower == "<code>" || lower.starts_with("<code ") {
                    dest.push_str(&words::prose(&text));
                    text.clear();
                    code = true;
                } else if lower == "</code>" {
                    code = false
                } else if lower.starts_with("<br") {
                    text.push(' ')
                }
            }
            _ if hidden_tag.is_some() => (),
            Event::Text(s) if !code => text.push_str(&s),
            Event::SoftBreak | Event::HardBreak => text.push(' '),
            Event::Start(Tag::TableHead) => head = true,
            Event::Start(Tag::TableCell) => {
                in_cell = true;
                cell.clear();
            }
            Event::Start(Tag::CodeBlock(_)) => code = true,
            // CommonMark parses the middle of 2*3*4 as emphasis. Restore its
            // delimiters only when this span occurs between numeric operands.
            Event::Start(Tag::Emphasis) | Event::End(TagEnd::Emphasis)
                if src[range.clone()].starts_with('*')
                    && src[..range.start].ends_with(|c: char| c.is_ascii_digit())
                    && src[range.end..].starts_with(|c: char| c.is_ascii_digit()) =>
            {
                text.push('×')
            }
            Event::Start(_) => (),
            Event::End(
                TagEnd::Strong
                | TagEnd::Emphasis
                | TagEnd::Link
                | TagEnd::Image
                | TagEnd::Strikethrough,
            ) => (),
            other => {
                dest.push_str(&words::prose(&text));
                text.clear();
                match other {
                    Event::Code(s) | Event::Text(s) => dest.push_str(&words::literal(&s)),
                    Event::InlineMath(s) | Event::DisplayMath(s) => {
                        dest.push_str(&words::math_tex(&s))
                    }
                    Event::End(TagEnd::CodeBlock) => {
                        code = false;
                        sentence(dest);
                    }
                    Event::End(TagEnd::TableCell) => {
                        if head {
                            headers.push(cell.trim().to_owned())
                        } else {
                            row.push(cell.trim().to_owned())
                        }
                        in_cell = false;
                    }
                    Event::End(TagEnd::TableHead) => head = false,
                    Event::End(TagEnd::TableRow) => {
                        rows += 1;
                        row_sentence(&mut out, &headers, &row);
                        row.clear();
                    }
                    Event::End(TagEnd::Table) => {
                        if rows == 0 {
                            out.push_str(&headers.join(", "));
                            sentence(&mut out)
                        }
                        headers.clear();
                        rows = 0;
                    }
                    Event::End(TagEnd::Heading(_) | TagEnd::Item) => sentence(dest),
                    Event::End(TagEnd::Paragraph) => dest.push(' '),
                    _ => (),
                }
            }
        }
    }
    out.push_str(&words::prose(&text));
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}
