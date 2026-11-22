//! Живые сетевые тесты (безопасные GET/POST к публичным сайтам). Запуск:
//! `cargo test --test live -- --ignored --nocapture`

use jarvis_web::http::default_client;
use jarvis_web::search::{DuckDuckGo, SearchProvider};

#[tokio::test]
#[ignore = "сеть: html.duckduckgo.com"]
async fn live_duckduckgo_search() {
    let ddg = DuckDuckGo::new(default_client());
    let started = std::time::Instant::now();
    let hits = ddg
        .search("rust programming language", 8)
        .await
        .expect("DuckDuckGo");
    println!(
        "DuckDuckGo: {} результатов за {} мс",
        hits.len(),
        started.elapsed().as_millis()
    );
    for (i, h) in hits.iter().enumerate() {
        println!("[{}] {} — {}\n    {}", i + 1, h.title, h.url, h.snippet);
    }
    assert!(hits.len() >= 3);
    assert!(hits.iter().any(|h| h.url.contains("rust-lang.org")));
    assert!(
        hits.iter()
            .all(|h| h.url.starts_with("http") && !h.url.contains("duckduckgo.com/l/"))
    );
    println!(
        "Проверить: {}",
        jarvis_web::validate(&ddg).await.summary_ru()
    );
}

#[tokio::test]
#[ignore = "сеть: www.rust-lang.org"]
async fn live_fetch_rust_lang() {
    let page = jarvis_web::fetch("https://www.rust-lang.org")
        .await
        .expect("fetch");
    println!(
        "url={} final_url={} title={:?} type={} truncated={} chars={}",
        page.url,
        page.final_url,
        page.title,
        page.content_type,
        page.truncated,
        page.markdown.chars().count()
    );
    println!(
        "---\n{}\n---",
        page.markdown.chars().take(1200).collect::<String>()
    );
    assert!(page.title.contains("Rust"));
    assert_eq!(page.content_type, "text/html");
    assert!(page.markdown.chars().count() > 300);
    assert!(page.markdown.contains("Rust"));
}
