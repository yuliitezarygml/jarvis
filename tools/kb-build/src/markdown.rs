//! Markdown (после docfx-очистки) → разделы по H2/H3 с блоками текста и кода (`pulldown-cmark`).
//! Ссылки превращаются в текст, картинки и HTML отбрасываются, таблицы — строки «ячейка | ячейка»,
//! списки — строки «- пункт». H1 — заголовок документа, H4+ остаются внутри раздела строкой текста.

use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    Text(String),
    Code { lang: String, code: String },
}

impl Block {
    pub fn text(&self) -> &str {
        match self {
            Block::Text(t) => t,
            Block::Code { code, .. } => code,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Section {
    /// 1 — вводная часть до первого H2, 2 — H2, 3 — H3.
    pub level: u8,
    /// Путь заголовков: `[]` для вводной части, `["Examples"]`, `["Examples", "Example 1"]`.
    pub path: Vec<String>,
    pub blocks: Vec<Block>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ParsedMd {
    pub h1: Option<String>,
    pub sections: Vec<Section>,
}

fn level_num(l: HeadingLevel) -> u8 {
    match l {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

fn clean_ws(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for (i, line) in s.lines().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        // отступ вложенных пунктов списка сохраняем, остальные пробелы схлопываем
        let indent = line.len() - line.trim_start_matches(' ').len();
        let body = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if !body.is_empty() {
            out.push_str(&" ".repeat(indent.min(8)));
            out.push_str(&body);
        }
    }
    // схлопнуть пустые строки
    while out.contains("\n\n\n") {
        out = out.replace("\n\n\n", "\n\n");
    }
    out.trim().to_string()
}

struct Builder {
    out: ParsedMd,
    cur: Section,
    h2: Option<String>,
    /// Текст текущего «текстового» блока (абзац/список/таблица/цитата).
    buf: String,
    heading: Option<(u8, String)>,
    code: Option<(String, String)>,
    list_stack: Vec<Option<u64>>,
    in_table_cell: bool,
    row_cells: Vec<String>,
    /// Строки текущей таблицы (одна таблица — один блок).
    table: String,
    image_depth: usize,
    started_content: bool,
}

impl Builder {
    fn new() -> Self {
        Self {
            out: ParsedMd::default(),
            cur: Section { level: 1, ..Default::default() },
            h2: None,
            buf: String::new(),
            heading: None,
            code: None,
            list_stack: Vec::new(),
            in_table_cell: false,
            row_cells: Vec::new(),
            table: String::new(),
            image_depth: 0,
            started_content: false,
        }
    }

    fn push_text(&mut self, t: &str) {
        if self.image_depth > 0 {
            return;
        }
        if let Some((_, h)) = &mut self.heading {
            h.push_str(t);
        } else if let Some((_, c)) = &mut self.code {
            c.push_str(t);
        } else {
            self.buf.push_str(t);
        }
    }

    fn flush_text(&mut self) {
        let t = clean_ws(&self.buf);
        self.buf.clear();
        if !t.is_empty() {
            self.cur.blocks.push(Block::Text(t));
            self.started_content = true;
        }
    }

    fn new_section(&mut self, level: u8, title: String) {
        self.flush_text();
        let prev = std::mem::take(&mut self.cur);
        if !prev.blocks.is_empty() || prev.level > 1 {
            self.out.sections.push(prev);
        }
        let path = if level == 2 {
            self.h2 = Some(title.clone());
            vec![title]
        } else {
            match &self.h2 {
                Some(h2) => vec![h2.clone(), title],
                None => vec![title],
            }
        };
        self.cur = Section { level, path, blocks: Vec::new() };
    }

    fn finish(mut self) -> ParsedMd {
        self.flush_text();
        let cur = std::mem::take(&mut self.cur);
        if !cur.blocks.is_empty() || cur.level > 1 {
            self.out.sections.push(cur);
        }
        self.out
    }

    fn event(&mut self, ev: Event<'_>) {
        match ev {
            Event::Start(Tag::Heading { .. }) => {
                self.flush_text();
                self.heading = Some((0, String::new()));
            }
            Event::End(TagEnd::Heading(level)) => {
                let (_, text) = self.heading.take().unwrap_or_default();
                let text = clean_ws(&text);
                let lvl = level_num(level);
                if text.is_empty() {
                    return;
                }
                if lvl == 1 && self.out.h1.is_none() && !self.started_content && self.out.sections.is_empty() {
                    self.out.h1 = Some(text);
                } else if lvl == 2 || lvl == 3 {
                    self.new_section(lvl, text);
                } else {
                    // H1 в середине и H4+ — строка текста внутри раздела
                    self.cur.blocks.push(Block::Text(text));
                }
            }
            Event::Start(Tag::CodeBlock(kind)) => {
                self.flush_text();
                let lang = match kind {
                    CodeBlockKind::Fenced(info) => info.split_whitespace().next().unwrap_or("").to_lowercase(),
                    CodeBlockKind::Indented => String::new(),
                };
                self.code = Some((lang, String::new()));
            }
            Event::End(TagEnd::CodeBlock) => {
                if let Some((lang, code)) = self.code.take() {
                    let code = code.trim_end().to_string();
                    if !code.trim().is_empty() {
                        self.cur.blocks.push(Block::Code { lang, code });
                        self.started_content = true;
                    }
                }
                // код внутри пункта списка: продолжение списка пойдёт новым блоком
            }
            Event::Start(Tag::Paragraph) => {
                if !self.list_stack.is_empty() && !self.buf.is_empty() && !self.buf.ends_with(' ') && !self.buf.ends_with('\n') {
                    self.buf.push(' ');
                }
            }
            Event::End(TagEnd::Paragraph) => {
                if self.list_stack.is_empty() && !self.in_table_cell {
                    self.flush_text();
                }
            }
            Event::Start(Tag::List(start)) => {
                if self.list_stack.is_empty() {
                    self.flush_text();
                }
                self.list_stack.push(start);
            }
            Event::End(TagEnd::List(_)) => {
                self.list_stack.pop();
                if self.list_stack.is_empty() {
                    self.flush_text();
                }
            }
            Event::Start(Tag::Item) => {
                let depth = self.list_stack.len().saturating_sub(1);
                let marker = match self.list_stack.last_mut() {
                    Some(Some(n)) => {
                        let m = format!("{n}. ");
                        *n += 1;
                        m
                    }
                    _ => "- ".to_string(),
                };
                if !self.buf.is_empty() {
                    self.buf.push('\n');
                }
                self.buf.push_str(&"  ".repeat(depth));
                self.buf.push_str(&marker);
            }
            Event::End(TagEnd::Item) => {}
            Event::Start(Tag::Table(_)) => {
                self.flush_text();
                self.table.clear();
            }
            Event::End(TagEnd::Table) => {
                let t = std::mem::take(&mut self.table);
                if !t.is_empty() {
                    self.cur.blocks.push(Block::Text(t));
                    self.started_content = true;
                }
            }
            Event::Start(Tag::TableCell) => {
                self.in_table_cell = true;
            }
            Event::End(TagEnd::TableCell) => {
                self.in_table_cell = false;
                let cell = clean_ws(&std::mem::take(&mut self.buf)).replace('\n', " ");
                self.row_cells.push(cell);
            }
            Event::End(TagEnd::TableHead) | Event::End(TagEnd::TableRow) => {
                let row = std::mem::take(&mut self.row_cells).join(" | ");
                if !row.replace('|', "").trim().is_empty() {
                    if !self.table.is_empty() {
                        self.table.push('\n');
                    }
                    self.table.push_str(&row);
                }
            }
            Event::Start(Tag::Image { .. }) => self.image_depth += 1,
            Event::End(TagEnd::Image) => self.image_depth = self.image_depth.saturating_sub(1),
            Event::End(TagEnd::BlockQuote(_)) => {
                if self.list_stack.is_empty() {
                    self.flush_text();
                }
            }
            Event::Text(t) => self.push_text(&t),
            Event::Code(t) => {
                let s = format!("`{t}`");
                self.push_text(&s);
            }
            Event::SoftBreak => self.push_text(" "),
            Event::HardBreak => self.push_text("\n"),
            // HTML отбрасываем; встроенный тег (<br>, <li>, <kbd>) — граница слова
            Event::Html(_) => {}
            Event::InlineHtml(_) => self.push_text(" "),
            Event::TaskListMarker(done) => self.push_text(if done { "[x] " } else { "[ ] " }),
            _ => {}
        }
    }
}

pub fn parse(markdown: &str) -> ParsedMd {
    let opts = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS | Options::ENABLE_HEADING_ATTRIBUTES;
    let mut b = Builder::new();
    for ev in Parser::new_ext(markdown, opts) {
        b.event(ev);
    }
    b.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sections_by_h2_h3() {
        let md = "# robocopy\n\nCopies [file data](x.md) from one place.\n\n## Syntax\n\n```\nrobocopy <src> <dst>\n```\n\n\
                  ## Examples\n\n### Example 1 {#ex1}\n\nMirror:\n\n```cmd\nrobocopy a b /mir\n```\n\n#### Detail\n\nMore.\n";
        let p = parse(md);
        assert_eq!(p.h1.as_deref(), Some("robocopy"));
        let paths: Vec<_> = p.sections.iter().map(|s| s.path.join(" > ")).collect();
        assert_eq!(paths, ["", "Syntax", "Examples", "Examples > Example 1"]);
        assert_eq!(p.sections[0].blocks, [Block::Text("Copies file data from one place.".into())]);
        assert_eq!(p.sections[1].blocks, [Block::Code { lang: String::new(), code: "robocopy <src> <dst>".into() }]);
        assert!(p.sections[2].blocks.is_empty());
        assert_eq!(p.sections[3].blocks[1], Block::Code { lang: "cmd".into(), code: "robocopy a b /mir".into() });
        assert_eq!(p.sections[3].blocks[2], Block::Text("Detail".into()));
    }

    #[test]
    fn lists_tables_images_inline_code() {
        let md = "Intro ![alt text](a.png) with `Get-Service`.\n\n1. Open **Settings**\n2. Go to\n   - Apps\n   - Startup\n\n\
                  | Parameter | Description |\n|---|---|\n| /s | Copies subdirectories. |\n\n<div>html</div>\n\nEnd<br/>line";
        let p = parse(md);
        let texts: Vec<_> = p.sections[0].blocks.iter().map(Block::text).collect();
        assert_eq!(texts[0], "Intro with `Get-Service`.");
        assert_eq!(texts[1], "1. Open Settings\n2. Go to\n  - Apps\n  - Startup");
        assert_eq!(texts[2], "Parameter | Description\n/s | Copies subdirectories.");
        assert!(!texts.iter().any(|t| t.contains("html")));
        // <li> внутри ячейки — граница слова
        let p = parse("| Opt | Values |\n|---|---|\n| /copy | <ul><li>D - Data</li><li>A - Attributes</li></ul> |");
        assert_eq!(p.sections[0].blocks[0].text(), "Opt | Values\n/copy | D - Data A - Attributes");
        assert_eq!(*texts.last().unwrap(), "End line");
    }

    #[test]
    fn code_inside_list_item_is_kept_whole() {
        let md = "1. Run:\n\n   ```powershell\n   Get-Process | Sort-Object CPU\n   ```\n\n2. Done\n";
        let p = parse(md);
        let b = &p.sections[0].blocks;
        assert!(b.iter().any(|x| matches!(x, Block::Code { lang, code } if lang == "powershell" && code == "Get-Process | Sort-Object CPU")));
        assert!(b.iter().any(|x| x.text().contains("2. Done")));
    }

    #[test]
    fn h3_without_h2_and_late_h1() {
        let p = parse("Text first\n\n# Late H1\n\n### Lone H3\n\nx");
        assert_eq!(p.h1, None);
        assert_eq!(p.sections[0].blocks[1], Block::Text("Late H1".into()));
        assert_eq!(p.sections[1].path, ["Lone H3"]);
    }
}
