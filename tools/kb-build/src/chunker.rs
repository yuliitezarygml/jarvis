//! Разделы → фрагменты ~700 токенов. Границы — H2/H3; блок кода никогда не режется (даже если больше лимита);
//! слишком длинный текстовый блок режется по строкам, затем по словам. Мелкие соседние разделы (параметры
//! командлета, короткие H3) упаковываются вместе, но новый H2 начинает новый фрагмент, если текущий уже не мал.

use crate::markdown::{Block, Section};

#[derive(Debug, Clone, Copy)]
pub struct ChunkOpts {
    pub target_tokens: usize,
    pub min_tokens: usize,
}

impl Default for ChunkOpts {
    fn default() -> Self {
        Self { target_tokens: 700, min_tokens: 120 }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    /// Последний заголовок первого раздела фрагмента (пусто — вводная часть).
    pub heading: String,
    /// «H2 > H3» первого раздела.
    pub heading_path: String,
    /// Текст для индекса и сниппета (код — в ``` ```).
    pub body: String,
    /// Блоки кода фрагмента: (язык, код) — для извлечения команд.
    pub code: Vec<(String, String)>,
    /// Только текст (без кода) — для извлечения командлетов из прозы.
    pub prose: String,
    pub tokens: usize,
}

/// Грубая оценка токенов: ~4 символа на токен (англ. текст и код).
pub fn est_tokens(s: &str) -> usize {
    s.chars().count().div_ceil(4)
}

fn block_tokens(b: &Block) -> usize {
    match b {
        Block::Text(t) => est_tokens(t),
        Block::Code { lang, code } => est_tokens(code) + est_tokens(lang) + 2,
    }
}

/// Порезать длинный текст на части ≤ limit токенов: сначала по строкам, потом по словам.
fn split_text(t: &str, limit: usize) -> Vec<String> {
    let max_chars = limit * 4;
    let mut parts = Vec::new();
    let mut cur = String::new();
    let push_piece = |piece: &str, cur: &mut String, parts: &mut Vec<String>| {
        if !cur.is_empty() && cur.chars().count() + piece.chars().count() + 1 > max_chars {
            parts.push(std::mem::take(cur));
        }
        if !cur.is_empty() {
            cur.push('\n');
        }
        cur.push_str(piece);
    };
    for line in t.lines() {
        if line.chars().count() <= max_chars {
            push_piece(line, &mut cur, &mut parts);
            continue;
        }
        // очень длинная строка — по словам
        let mut acc = String::new();
        for w in line.split_whitespace() {
            if !acc.is_empty() && acc.chars().count() + w.chars().count() + 1 > max_chars {
                push_piece(&std::mem::take(&mut acc), &mut cur, &mut parts);
            }
            if !acc.is_empty() {
                acc.push(' ');
            }
            acc.push_str(w);
        }
        if !acc.is_empty() {
            push_piece(&acc, &mut cur, &mut parts);
        }
    }
    if !cur.is_empty() {
        parts.push(cur);
    }
    parts
}

struct Piece<'a> {
    section: &'a Section,
    first_of_section: bool,
    blocks: Vec<Block>,
    tokens: usize,
}

fn pieces<'a>(sections: &'a [Section], target: usize) -> Vec<Piece<'a>> {
    let mut out = Vec::new();
    for s in sections {
        let mut blocks: Vec<Block> = Vec::new();
        let mut tokens = 0;
        let mut first = true;
        let mut expanded: Vec<Block> = Vec::new();
        for b in &s.blocks {
            match b {
                Block::Text(t) if est_tokens(t) > target => {
                    expanded.extend(split_text(t, target).into_iter().map(Block::Text));
                }
                other => expanded.push(other.clone()),
            }
        }
        for b in expanded {
            let t = block_tokens(&b);
            if !blocks.is_empty() && tokens + t > target {
                out.push(Piece { section: s, first_of_section: first, blocks: std::mem::take(&mut blocks), tokens });
                first = false;
                tokens = 0;
            }
            tokens += t;
            blocks.push(b);
        }
        if !blocks.is_empty() {
            out.push(Piece { section: s, first_of_section: first, blocks, tokens });
        }
    }
    out
}

fn render(chunk: &[Piece<'_>]) -> Chunk {
    let first = chunk[0].section;
    let mut body = String::new();
    let mut prose = String::new();
    let mut code = Vec::new();
    for (i, p) in chunk.iter().enumerate() {
        if i > 0 && p.first_of_section && !p.section.path.is_empty() {
            let h = p.section.path.last().cloned().unwrap_or_default();
            body.push_str(&format!("{} {}\n\n", "#".repeat(p.section.level as usize), h));
            prose.push_str(&h);
            prose.push('\n');
        }
        for b in &p.blocks {
            match b {
                Block::Text(t) => {
                    body.push_str(t);
                    prose.push_str(t);
                    prose.push('\n');
                }
                Block::Code { lang, code: c } => {
                    body.push_str(&format!("```{lang}\n{c}\n```"));
                    code.push((lang.clone(), c.clone()));
                }
            }
            body.push_str("\n\n");
        }
    }
    let body = body.trim_end().to_string();
    Chunk {
        heading: first.path.last().cloned().unwrap_or_default(),
        heading_path: first.path.join(" > "),
        tokens: est_tokens(&body),
        body,
        code,
        prose: prose.trim_end().to_string(),
    }
}

pub fn chunk(sections: &[Section], opts: ChunkOpts) -> Vec<Chunk> {
    let ps = pieces(sections, opts.target_tokens);
    let mut out = Vec::new();
    let mut cur: Vec<Piece<'_>> = Vec::new();
    let mut cur_tokens = 0;
    for p in ps {
        let new_h2 = p.first_of_section && p.section.level == 2;
        let over = cur_tokens + p.tokens > opts.target_tokens;
        if !cur.is_empty() && (over || (new_h2 && cur_tokens >= opts.min_tokens) || !p.first_of_section) {
            out.push(render(&cur));
            cur.clear();
            cur_tokens = 0;
        }
        cur_tokens += p.tokens;
        cur.push(p);
    }
    if !cur.is_empty() {
        out.push(render(&cur));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sec(level: u8, path: &[&str], blocks: Vec<Block>) -> Section {
        Section { level, path: path.iter().map(|s| s.to_string()).collect(), blocks }
    }
    fn text(n_words: usize, w: &str) -> Block {
        Block::Text(vec![w; n_words].join(" "))
    }

    #[test]
    fn small_sections_are_packed_and_headings_kept() {
        let s = vec![
            sec(1, &[], vec![Block::Text("Intro text.".into())]),
            sec(2, &["Parameters"], vec![]),
            sec(3, &["Parameters", "-Name"], vec![Block::Text("Name of service.".into())]),
            sec(3, &["Parameters", "-Force"], vec![Block::Text("Forces.".into())]),
        ];
        let c = chunk(&s, ChunkOpts::default());
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].heading_path, "");
        assert!(c[0].body.contains("### -Name\n\nName of service."), "{}", c[0].body);
        assert!(c[0].body.contains("### -Force"));
    }

    #[test]
    fn h2_starts_new_chunk_when_current_is_big_enough() {
        let s = vec![
            sec(2, &["Description"], vec![text(200, "word")]), // ~250 токенов
            sec(2, &["Examples"], vec![Block::Text("Example text.".into())]),
        ];
        let c = chunk(&s, ChunkOpts::default());
        assert_eq!(c.iter().map(|c| c.heading.as_str()).collect::<Vec<_>>(), ["Description", "Examples"]);
    }

    #[test]
    fn respects_target_and_never_splits_code() {
        let big_code = (0..400).map(|i| format!("robocopy src{i} dst{i} /e")).collect::<Vec<_>>().join("\n");
        let s = vec![sec(
            2,
            &["Examples"],
            vec![text(300, "alpha"), Block::Code { lang: "cmd".into(), code: big_code.clone() }, text(300, "omega")],
        )];
        let c = chunk(&s, ChunkOpts::default());
        assert_eq!(c.len(), 3, "{:?}", c.iter().map(|c| c.tokens).collect::<Vec<_>>());
        assert!(c[1].body.contains(&big_code), "код целиком в одном фрагменте");
        assert_eq!(c[1].code.len(), 1);
        assert!(c.iter().all(|x| x.heading_path == "Examples"));
        assert!(c[0].tokens <= 700 && c[2].tokens <= 700);
    }

    #[test]
    fn huge_text_block_is_split() {
        let lines = (0..300).map(|i| format!("Line number {i} with some descriptive words")).collect::<Vec<_>>().join("\n");
        let one_line = vec!["w"; 5000].join(" ");
        let s = vec![sec(2, &["Big"], vec![Block::Text(lines), Block::Text(one_line)])];
        let c = chunk(&s, ChunkOpts::default());
        assert!(c.len() >= 4);
        assert!(c.iter().all(|x| x.tokens <= 720), "{:?}", c.iter().map(|c| c.tokens).collect::<Vec<_>>());
        let all: String = c.iter().map(|x| x.body.clone()).collect();
        assert!(all.contains("Line number 0 ") && all.contains("Line number 299 "));
    }

    #[test]
    fn prose_excludes_code() {
        let s = vec![sec(1, &[], vec![Block::Text("Use Get-Service.".into()), Block::Code { lang: "powershell".into(), code: "Stop-Service x".into() }])];
        let c = chunk(&s, ChunkOpts::default());
        assert_eq!(c[0].prose, "Use Get-Service.");
        assert_eq!(c[0].code, vec![("powershell".to_string(), "Stop-Service x".to_string())]);
        assert!(c[0].body.contains("```powershell\nStop-Service x\n```"));
        assert!(chunk(&[], ChunkOpts::default()).is_empty());
    }
}
