pub fn valid(text: &str) -> bool {
    text.chars().count() <= 2000
        && text.chars().any(|c| {
            c.is_ascii_alphanumeric()
                || ('а'..='я').contains(&c.to_ascii_lowercase())
                || ('А'..='Я').contains(&c)
                || matches!(c, 'ё' | 'Ё')
        })
}
pub fn prepare(text: &str) -> anyhow::Result<String> {
    anyhow::ensure!(valid(text), "invalid text");
    Ok(text.split_whitespace().collect::<Vec<_>>().join(" "))
}
