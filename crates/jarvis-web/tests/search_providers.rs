//! Поисковики на мок-сервере (форматы запросов/ответов — по официальной документации) и цепочка с failover.

mod common;

use std::sync::Arc;

use common::{StaticSearch, fixture, hit};
use jarvis_web::http::{BROWSER_USER_AGENT, USER_AGENT, default_client};
use jarvis_web::search::duckduckgo::{DuckDuckGo, parse_results};
use jarvis_web::search::{
    Brave, GoogleCse, JinaSearch, SearchChain, SearchError, SearchProvider, Serper, Tavily,
    validate,
};
use wiremock::matchers::{
    body_partial_json, body_string_contains, header, method, path, query_param,
};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn json_fixture(name: &str) -> ResponseTemplate {
    ResponseTemplate::new(200)
        .insert_header("content-type", "application/json")
        .set_body_string(fixture(name))
}

// ------------------------------------------------------------------ DuckDuckGo

#[test]
fn ddg_real_fixture_parses() {
    let hits = parse_results(&fixture("ddg_rust.html"), 10).unwrap();
    assert_eq!(hits.len(), 10);
    assert_eq!(hits[0].title, "Rust Programming Language");
    assert_eq!(hits[0].url, "https://rust-lang.org/");
    assert!(
        hits[0]
            .snippet
            .starts_with("Rust is a fast, reliable, and productive programming language"),
        "{}",
        hits[0].snippet
    );
    assert_eq!(
        hits[1].url,
        "https://en.wikipedia.org/wiki/Rust_(programming_language)"
    );
    // HTML-сущности в сниппете декодированы.
    assert!(
        hits[3].snippet.contains("you're using Rust")
            && hits[3].snippet.contains("edition = \"2024\"")
    );
    assert!(hits.iter().all(|h| !h.url.contains("duckduckgo.com")));
    assert_eq!(
        parse_results(&fixture("ddg_rust.html"), 3).unwrap().len(),
        3
    );
}

#[test]
fn ddg_legacy_uddg_and_ads() {
    let hits = parse_results(&fixture("ddg_legacy_uddg.html"), 10).unwrap();
    let urls: Vec<_> = hits.iter().map(|h| h.url.as_str()).collect();
    // Реклама (y.js, result--ad) отброшена, редиректы раскрыты, дубль убран.
    assert_eq!(
        urls,
        [
            "https://www.gismeteo.ru/weather-moscow-4368/",
            "https://yandex.ru/pogoda/moscow?lat=55.75&lon=37.62"
        ]
    );
    assert_eq!(hits[0].title, "Погода в Москве — Gismeteo");
    assert_eq!(
        hits[0].snippet,
        "Прогноз погоды в Москве на сегодня & завтра: температура, осадки, ветер."
    );
}

#[test]
fn ddg_captcha_is_blocked_error() {
    // Реальная страница, которую DuckDuckGo отдаёт на UA `JarvisBot/1.0` (HTTP 202).
    assert_eq!(
        parse_results(&fixture("ddg_anomaly.html"), 10),
        Err(SearchError::Blocked)
    );
}

#[test]
fn ddg_garbage_is_bad_response() {
    assert!(matches!(
        parse_results("<html><body>hello</body></html>", 5),
        Err(SearchError::BadResponse(_))
    ));
    let no_results =
        r#"<div id="links" class="results"><div class="no-results">No results.</div></div>"#;
    assert_eq!(parse_results(no_results, 5), Ok(vec![]));
}

#[tokio::test]
async fn ddg_posts_form_with_browser_ua() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/html/"))
        .and(header("content-type", "application/x-www-form-urlencoded"))
        .and(body_string_contains("q=rust+programming+language"))
        .respond_with(ResponseTemplate::new(200).set_body_string(fixture("ddg_rust.html")))
        .expect(1)
        .mount(&server)
        .await;
    let ddg = DuckDuckGo::new(default_client()).with_base_url(server.uri());
    let hits = ddg.search("rust programming language", 5).await.unwrap();
    assert_eq!(hits.len(), 5);
    assert_eq!(ddg.id(), "duckduckgo");
    // Матчер `header` у wiremock делит значение по запятым, поэтому UA сверяем по записанному запросу.
    let reqs = server.received_requests().await.unwrap();
    assert_eq!(
        reqs[0].headers.get("user-agent").unwrap(),
        BROWSER_USER_AGENT
    );
}

#[tokio::test]
async fn ddg_http_202_captcha() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(202).set_body_string(fixture("ddg_anomaly.html")))
        .mount(&server)
        .await;
    let ddg = DuckDuckGo::new(default_client()).with_base_url(server.uri());
    assert_eq!(ddg.search("x", 5).await, Err(SearchError::Blocked));
}

// ------------------------------------------------------------------ Brave

#[tokio::test]
async fn brave_request_and_parse() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/res/v1/web/search"))
        .and(query_param("q", "rust programming language"))
        .and(query_param("count", "3"))
        .and(header("x-subscription-token", "BSA-test"))
        .and(header("accept", "application/json"))
        .and(header("user-agent", USER_AGENT))
        .respond_with(json_fixture("brave_web_search.json"))
        .expect(1)
        .mount(&server)
        .await;
    let p = Brave::new(default_client(), "BSA-test".into()).with_base_url(server.uri());
    let hits = p.search("rust programming language", 3).await.unwrap();
    assert_eq!(hits.len(), 3);
    assert_eq!(hits[0].url, "https://www.rust-lang.org/");
    // <strong> из description снят.
    assert_eq!(
        hits[0].snippet,
        "A language empowering everyone to build reliable and efficient software."
    );
}

#[tokio::test]
async fn brave_bad_key_is_auth_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(401).set_body_string(
            r#"{"type":"ErrorResponse","error":{"id":"x","status":401,"code":"SUBSCRIPTION_TOKEN_INVALID","detail":"The provided subscription token is invalid."}}"#,
        ))
        .mount(&server)
        .await;
    let p = Brave::new(default_client(), "bad".into()).with_base_url(server.uri());
    match p.search("x", 3).await {
        Err(SearchError::Auth(m)) => assert!(m.contains("subscription token is invalid"), "{m}"),
        other => panic!("{other:?}"),
    }
}

// ------------------------------------------------------------------ Tavily

#[tokio::test]
async fn tavily_request_and_parse() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/search"))
        .and(header("authorization", "Bearer tvly-test"))
        .and(body_partial_json(
            serde_json::json!({"query": "Who is Leo Messi?", "max_results": 5}),
        ))
        .respond_with(json_fixture("tavily_search.json"))
        .expect(1)
        .mount(&server)
        .await;
    let p = Tavily::new(default_client(), "tvly-test".into()).with_base_url(server.uri());
    let hits = p.search("Who is Leo Messi?", 5).await.unwrap();
    assert_eq!(hits.len(), 2);
    assert_eq!(hits[0].title, "Lionel Messi Facts | Britannica");
    assert!(
        hits[0]
            .snippet
            .starts_with("Lionel Messi, an Argentine footballer")
    );
}

#[tokio::test]
async fn tavily_plan_limit_is_rate_limited() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(432).set_body_string(
            r#"{"detail":{"error":"This request exceeds your plan's set usage limit."}}"#,
        ))
        .mount(&server)
        .await;
    let p = Tavily::new(default_client(), "k".into()).with_base_url(server.uri());
    assert!(
        matches!(p.search("x", 5).await, Err(SearchError::RateLimited(m)) if m.contains("usage limit"))
    );
}

// ------------------------------------------------------------------ Serper

#[tokio::test]
async fn serper_request_and_parse() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/search"))
        .and(header("x-api-key", "serper-test"))
        .and(header("content-type", "application/json"))
        .and(body_partial_json(
            serde_json::json!({"q": "apple inc", "num": 10}),
        ))
        .respond_with(json_fixture("serper_search.json"))
        .expect(1)
        .mount(&server)
        .await;
    let p = Serper::new(default_client(), "serper-test".into()).with_base_url(server.uri());
    let hits = p.search("apple inc", 10).await.unwrap();
    assert_eq!(hits.len(), 3);
    assert_eq!(hits[1].url, "https://en.wikipedia.org/wiki/Apple_Inc.");
    assert_eq!(hits[2].snippet, "", "сниппета может не быть");
}

// ------------------------------------------------------------------ Google Programmable Search

#[tokio::test]
async fn google_cse_request_and_parse() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/customsearch/v1"))
        .and(query_param("key", "AIza-test"))
        .and(query_param("cx", "017576662512468239146:omuauf_lfve"))
        .and(query_param("q", "rust"))
        .and(query_param("num", "10"))
        .respond_with(json_fixture("google_cse.json"))
        .expect(1)
        .mount(&server)
        .await;
    let p = GoogleCse::new(
        default_client(),
        "AIza-test".into(),
        "017576662512468239146:omuauf_lfve".into(),
    )
    .with_base_url(server.uri());
    // num у Google максимум 10.
    let hits = p.search("rust", 25).await.unwrap();
    assert_eq!(hits.len(), 2);
    assert_eq!(hits[0].title, "Rust Programming Language");
    assert_eq!(hits[1].url, "https://en.wikipedia.org/wiki/Rust");
}

#[tokio::test]
async fn google_cse_no_items_and_bad_key() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(query_param("key", "empty"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{"kind":"customsearch#search","searchInformation":{"totalResults":"0"}}"#,
        ))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(query_param("key", "bad"))
        .respond_with(ResponseTemplate::new(400).set_body_string(
            r#"{"error":{"code":400,"message":"API key not valid. Please pass a valid API key.","status":"INVALID_ARGUMENT"}}"#,
        ))
        .mount(&server)
        .await;
    let empty =
        GoogleCse::new(default_client(), "empty".into(), "cx".into()).with_base_url(server.uri());
    assert_eq!(empty.search("q", 5).await, Ok(vec![]));
    let bad =
        GoogleCse::new(default_client(), "bad".into(), "cx".into()).with_base_url(server.uri());
    assert!(
        matches!(bad.search("q", 5).await, Err(SearchError::Auth(m)) if m.contains("API key not valid"))
    );
}

// ------------------------------------------------------------------ Jina

#[tokio::test]
async fn jina_search_with_and_without_key() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        // Запрос — сегмент пути с процентным кодированием.
        .and(path("/what%20is%20jina%2Fai%3F"))
        .and(header("accept", "application/json"))
        .and(header("x-respond-with", "no-content"))
        .respond_with(json_fixture("jina_search.json"))
        .expect(2)
        .mount(&server)
        .await;
    let keyed =
        JinaSearch::new(default_client(), Some("jina_test".into())).with_base_url(server.uri());
    let hits = keyed.search("what is jina/ai?", 5).await.unwrap();
    assert_eq!(hits.len(), 2);
    assert_eq!(
        hits[0].snippet,
        "Acquisition advances Elastic’s leadership in context engineering."
    );
    let anon = JinaSearch::new(default_client(), None).with_base_url(server.uri());
    assert_eq!(anon.search("what is jina/ai?", 1).await.unwrap().len(), 1);

    let reqs = server.received_requests().await.unwrap();
    assert_eq!(
        reqs[0].headers.get("authorization").unwrap(),
        "Bearer jina_test"
    );
    assert!(reqs[1].headers.get("authorization").is_none());
    assert_eq!(reqs[0].url.path(), "/what%20is%20jina%2Fai%3F");
}

// ------------------------------------------------------------------ Цепочка и проверка

#[tokio::test]
async fn chain_fails_over_in_config_order() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/res/v1/web/search"))
        .respond_with(
            ResponseTemplate::new(429).set_body_string(r#"{"error":{"detail":"Rate limit"}}"#),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(503))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/html/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(fixture("ddg_rust.html")))
        .expect(1)
        .mount(&server)
        .await;
    let c = default_client();
    let chain = SearchChain::new(vec![
        Arc::new(Brave::new(c.clone(), "k".into()).with_base_url(server.uri())),
        Arc::new(Tavily::new(c.clone(), "k".into()).with_base_url(server.uri())),
        Arc::new(DuckDuckGo::new(c).with_base_url(server.uri())),
    ]);
    let r = chain.search_detailed("rust", 4).await.unwrap();
    assert_eq!(r.provider, "duckduckgo");
    assert_eq!(r.hits.len(), 4);
    let failed: Vec<_> = r.failures.iter().map(|(id, _)| id.as_str()).collect();
    assert_eq!(failed, ["brave", "tavily"]);
    assert!(matches!(r.failures[0].1, SearchError::RateLimited(_)));
    assert!(matches!(
        r.failures[1].1,
        SearchError::Http { status: 503, .. }
    ));
}

#[tokio::test]
async fn chain_skips_empty_and_reports_all_failed() {
    let empty = StaticSearch {
        id: "empty".into(),
        ..Default::default()
    };
    let good = StaticSearch {
        id: "good".into(),
        ..StaticSearch::with_default(vec![hit("A", "https://a.com/", "")])
    };
    let chain = SearchChain::new(vec![Arc::new(empty), Arc::new(good)]);
    let r = chain.search_detailed("q", 5).await.unwrap();
    assert_eq!((r.provider.as_str(), r.hits.len()), ("good", 1));

    let chain = SearchChain::new(vec![
        Arc::new(StaticSearch::failing("a", SearchError::Timeout)),
        Arc::new(StaticSearch::failing("b", SearchError::Blocked)),
    ]);
    match chain.search("q", 5).await {
        Err(SearchError::AllFailed(f)) => {
            assert_eq!(f.len(), 2);
            let msg = SearchError::AllFailed(f).to_string();
            assert!(
                msg.contains("a: таймаут") && msg.contains("b: поисковик заблокировал"),
                "{msg}"
            );
        }
        other => panic!("{other:?}"),
    }

    // Все честно ответили «ничего» — не ошибка.
    let chain = SearchChain::new(vec![Arc::new(StaticSearch {
        id: "e".into(),
        ..Default::default()
    })]);
    assert_eq!(chain.search("q", 5).await, Ok(vec![]));
    assert_eq!(
        SearchChain::new(vec![]).search("q", 5).await,
        Err(SearchError::NoProviders)
    );
}

#[tokio::test]
async fn validate_reports_count_latency_and_errors() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(query_param("q", jarvis_web::search::VALIDATE_QUERY))
        .and(header("x-subscription-token", "good"))
        .respond_with(json_fixture("brave_web_search.json"))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(header("x-subscription-token", "bad"))
        .respond_with(
            ResponseTemplate::new(401).set_body_string(r#"{"error":{"detail":"invalid token"}}"#),
        )
        .mount(&server)
        .await;
    let good = Brave::new(default_client(), "good".into()).with_base_url(server.uri());
    let r = validate(&good).await;
    assert!(r.ok, "{r:?}");
    assert_eq!(r.results, 3);
    assert!(r.error.is_none());
    assert!(
        r.summary_ru().starts_with("работает ✓ (3 результата, "),
        "{}",
        r.summary_ru()
    );

    let bad = Brave::new(default_client(), "bad".into()).with_base_url(server.uri());
    let r = validate(&bad).await;
    assert!(!r.ok);
    assert!(
        r.error.as_deref().unwrap().starts_with("ключ не принят"),
        "{r:?}"
    );

    // Запись конфига без обязательного ключа — понятная ошибка без сети.
    let cfg = jarvis_core::config::SearchProviderConfig {
        id: "tavily".into(),
        enabled: true,
        ..Default::default()
    };
    let r = jarvis_web::validate_config(&cfg, &default_client()).await;
    assert_eq!(
        r.error.as_deref(),
        Some("для поисковика «tavily» нужен API-ключ")
    );
}
