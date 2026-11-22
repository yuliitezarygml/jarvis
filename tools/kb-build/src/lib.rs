//! Сборщик базы знаний Windows (`tools/kb-build`, план, раздел 7). Бинарь — `src/main.rs`, логика — здесь,
//! чтобы её можно было тестировать: front matter, docfx-очистка, Markdown → разделы, фрагменты, команды,
//! URL на learn.microsoft.com, git sparse-клоны, сборка и оценка Recall@5/MRR.

pub mod build;
pub mod chunker;
pub mod config;
pub mod docfx;
pub mod eval;
pub mod extract;
pub mod frontmatter;
pub mod git;
pub mod glob;
pub mod markdown;
pub mod url;
pub mod urlcheck;

use std::path::{Path, PathBuf};

/// Корень проекта: каталог, где лежит `tools/kb-build/kb_sources.json` (вверх от `start`), иначе — по месту
/// исходников крейта (`CARGO_MANIFEST_DIR/../..`).
pub fn find_project_root(start: &Path) -> PathBuf {
    let mut cur = Some(start);
    while let Some(d) = cur {
        if d.join("tools").join("kb-build").join("kb_sources.json").is_file() {
            return d.to_path_buf();
        }
        cur = d.parent();
    }
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..").canonicalize().unwrap_or_else(|_| start.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_root_from_nested_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("tools/kb-build")).unwrap();
        std::fs::write(root.join("tools/kb-build/kb_sources.json"), "{}").unwrap();
        std::fs::create_dir_all(root.join("a/b")).unwrap();
        assert_eq!(find_project_root(&root.join("a/b")), root);
    }
}
