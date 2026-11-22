//! `web_fetch` на мок-сервере: статья, документация, кодировки, PDF, обрезка, Jina Reader, ошибки и запреты.

mod common;

use std::time::Duration;

use common::{fixture, test_fetcher, tiny_pdf};
use jarvis_web::http::USER_AGENT;
use jarvis_web::{FetchError, FetchOptions, Fetcher};
use wiremock::matchers::{header, method, path, path_regex};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn html(body: impl Into<String>) -> ResponseTemplate {
    // set_body_string у wiremock ставит text/plain, поэтому raw с явным типом.
    ResponseTemplate::new(200).set_body_raw(body.into().into_bytes(), "text/html; charset=utf-8")
}

async fn serve(server: &MockServer, p: &str, resp: ResponseTemplate) {
    Mock::given(method("GET"))
        .and(path(p))
        .respond_with(resp)
        .mount(server)
        .await;
}

#[tokio::test]
async fn article_main_content_only() {
    let server = MockServer::start().await;
    serve(&server, "/news/rust-1-100", html(fixture("article.html"))).await;
    let page = test_fetcher(FetchOptions::default())
        .fetch(&format!("{}/news/rust-1-100#comments", server.uri()))
        .await
        .unwrap();

    assert_eq!(page.title, "Вышел Rust 1.100: что нового");
    assert_eq!(page.content_type, "text/html");
    assert!(!page.truncated);
    assert_eq!(
        page.url,
        format!("{}/news/rust-1-100", server.uri()),
        "фрагмент отброшен"
    );
    let md = &page.markdown;
    for must in [
        "Команда Rust выпустила версию 1.100",
        "## Ускорение компиляции",
        "## Асинхронные замыкания",
        "```\nlet fetch = async |url: &str|",
        "- улучшена производительность `HashMap` на коротких ключах;",
        // Относительная ссылка стала абсолютной.
        &format!("[примечаниях к выпуску]({}/go/release-notes)", server.uri()),
    ] {
        assert!(md.contains(must), "нет «{must}» в:\n{md}");
    }
    for junk in [
        "Мы используем cookies",
        "Обзоры",
        "Популярное",
        "Реклама",
        "Все права защищены",
        "Наконец-то",
        "gtag",
        "benchmark.png",
    ] {
        assert!(!md.contains(junk), "мусор «{junk}» в:\n{md}");
    }

    let reqs = server.received_requests().await.unwrap();
    assert_eq!(reqs[0].headers.get("user-agent").unwrap(), USER_AGENT);
}

#[tokio::test]
async fn documentation_page_keeps_code_drops_sidebar() {
    let server = MockServer::start().await;
    serve(
        &server,
        "/std/vec/struct.Vec.html",
        html(fixture("docs_page.html")),
    )
    .await;
    let page = test_fetcher(FetchOptions::default())
        .fetch(&format!("{}/std/vec/struct.Vec.html", server.uri()))
        .await
        .unwrap();
    assert_eq!(page.title, "Vec in std::vec - Rust");
    let md = &page.markdown;
    for must in [
        "A contiguous growable array type",
        "## Examples",
        "vec.push(1);\nvec.push(2);",
        "assert_eq!(vec.pop(), Some(2));",
        "## Capacity and reallocation",
        "Appends an element to the back of a collection.",
    ] {
        assert!(md.contains(must), "нет «{must}» в:\n{md}");
    }
    for junk in [
        "Trait Implementations",
        "Keyboard shortcuts",
        "show sidebar",
        "[§]",
    ] {
        assert!(!md.contains(junk), "мусор «{junk}» в:\n{md}");
    }
}

#[tokio::test]
async fn windows_1251_pages_decode() {
    let server = MockServer::start().await;
    let src =
        "<html><head><meta charset=\"windows-1251\"><title>Новости</title></head><body><article><p>"
            .to_string()
            + &"Сэр, это текст в кодировке windows-1251, его нужно правильно декодировать. "
                .repeat(10) + "</p></article></body></html>";
    let (bytes, _, _) = encoding_rs::WINDOWS_1251.encode(&src);
    // charset только в <meta>.
    Mock::given(method("GET"))
        .and(path("/meta"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(bytes.to_vec(), "text/html"))
        .mount(&server)
        .await;
    // charset только в заголовке.
    let no_meta = src.replace("<meta charset=\"windows-1251\">", "");
    let (bytes2, _, _) = encoding_rs::WINDOWS_1251.encode(&no_meta);
    Mock::given(method("GET"))
        .and(path("/header"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw(bytes2.to_vec(), "text/html; charset=windows-1251"),
        )
        .mount(&server)
        .await;
    let f = test_fetcher(FetchOptions::default());
    for p in ["/meta", "/header"] {
        let page = f.fetch(&format!("{}{p}", server.uri())).await.unwrap();
        assert_eq!(page.title, "Новости", "{p}");
        assert!(
            page.markdown
                .contains("Сэр, это текст в кодировке windows-1251"),
            "{p}: {}",
            page.markdown
        );
    }
}

#[tokio::test]
async fn tiny_pdf_to_text() {
    let server = MockServer::start().await;
    let pdf = tiny_pdf("Hello from Jarvis PDF test");
    Mock::given(method("GET"))
        .and(path("/files/report.pdf"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(pdf.clone(), "application/pdf"))
        .mount(&server)
        .await;
    // Без правильного типа — распознаётся по сигнатуре %PDF-.
    Mock::given(method("GET"))
        .and(path("/download"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(pdf, "application/octet-stream"))
        .mount(&server)
        .await;
    let f = test_fetcher(FetchOptions::default());
    for p in ["/files/report.pdf", "/download"] {
        let page = f.fetch(&format!("{}{p}", server.uri())).await.unwrap();
        assert_eq!(page.content_type, "application/pdf");
        assert_eq!(page.markdown.trim(), "Hello from Jarvis PDF test");
        assert_eq!(page.title, "Hello from Jarvis PDF test");
    }

    // Битый PDF — ошибка, а не паника.
    Mock::given(method("GET"))
        .and(path("/broken.pdf"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw(b"%PDF-1.4\ngarbage".to_vec(), "application/pdf"),
        )
        .mount(&server)
        .await;
    assert!(matches!(
        f.fetch(&format!("{}/broken.pdf", server.uri())).await,
        Err(FetchError::Pdf(_))
    ));
}

#[tokio::test]
async fn long_page_truncated_with_marker() {
    let server = MockServer::start().await;
    let para = "<p>Очень длинная статья о том, как Джарвис читает страницы и бережёт контекст модели.</p>\n";
    let big = format!(
        "<html><head><title>Длинная</title></head><body><article>{}</article></body></html>",
        para.repeat(1500)
    );
    serve(&server, "/long", html(big)).await;
    let page = test_fetcher(FetchOptions::default())
        .fetch(&format!("{}/long", server.uri()))
        .await
        .unwrap();
    assert!(page.truncated);
    let (body, marker) = page
        .markdown
        .split_once("\n\n[…текст обрезан: показано ")
        .expect("нет пометки обрезки");
    assert!(body.chars().count() <= 48_000, "{}", body.chars().count());
    assert!(body.chars().count() > 40_000);
    assert!(marker.contains("символов]"));
}

#[tokio::test]
async fn body_limit_reads_only_beginning() {
    let server = MockServer::start().await;
    let para = "<p>Абзац текста для проверки лимита тела ответа в байтах, а не в символах.</p>\n";
    let big = format!(
        "<html><head><title>Большая</title></head><body><article>{}</article></body></html>",
        para.repeat(3000)
    );
    assert!(big.len() > 200_000);
    serve(&server, "/huge", html(big)).await;
    let f = test_fetcher(FetchOptions {
        max_bytes: 64 * 1024,
        ..Default::default()
    });
    let page = f.fetch(&format!("{}/huge", server.uri())).await.unwrap();
    assert!(page.truncated);
    assert_eq!(page.title, "Большая");
    assert!(
        page.markdown
            .ends_with("[…страница больше 64 КБ, прочитано только начало]"),
        "{}",
        page.markdown
    );
    assert!(page.markdown.chars().count() < 64 * 1024);
}

#[tokio::test]
async fn js_page_uses_jina_reader_when_enabled() {
    let server = MockServer::start().await;
    serve(&server, "/app", html(fixture("js_app.html"))).await;
    Mock::given(method("GET"))
        .and(path_regex(r"^/jina/http://127\.0\.0\.1:\d+/app$"))
        .and(header("accept", "application/json"))
        .and(header("authorization", "Bearer jina_key"))
        .respond_with(ResponseTemplate::new(200).set_body_string(fixture("jina_reader.json")))
        .expect(1)
        .mount(&server)
        .await;

    let with_jina = test_fetcher(FetchOptions {
        jina_reader: true,
        jina_key: Some("jina_key".into()),
        jina_reader_base: format!("{}/jina", server.uri()),
        ..Default::default()
    });
    let page = with_jina
        .fetch(&format!("{}/app", server.uri()))
        .await
        .unwrap();
    assert_eq!(page.content_type, "text/markdown");
    assert_eq!(page.title, "Dashboard");
    assert!(
        page.markdown.contains("продажи за ноябрь выросли на 12 %"),
        "{}",
        page.markdown
    );

    // Без Jina: честная ошибка «нет текста».
    let plain = test_fetcher(FetchOptions::default());
    assert_eq!(
        plain.fetch(&format!("{}/app", server.uri())).await,
        Err(FetchError::Empty)
    );
}

#[tokio::test]
async fn redirects_errors_and_types() {
    let server = MockServer::start().await;
    serve(
        &server,
        "/old",
        ResponseTemplate::new(301).insert_header("location", "/news/rust-1-100"),
    )
    .await;
    serve(&server, "/news/rust-1-100", html(fixture("article.html"))).await;
    serve(&server, "/missing", ResponseTemplate::new(404)).await;
    serve(
        &server,
        "/logo.png",
        ResponseTemplate::new(200).set_body_raw(b"\x89PNG\r\n\x1a\n".to_vec(), "image/png"),
    )
    .await;
    serve(
        &server,
        "/notes.txt",
        ResponseTemplate::new(200).set_body_raw(
            "Заметка   \n\n\n\nвторая строка".as_bytes().to_vec(),
            "text/plain; charset=utf-8",
        ),
    )
    .await;

    let f = test_fetcher(FetchOptions::default());
    let page = f.fetch(&format!("{}/old", server.uri())).await.unwrap();
    assert_eq!(page.final_url, format!("{}/news/rust-1-100", server.uri()));
    assert_eq!(page.url, format!("{}/old", server.uri()));

    assert_eq!(
        f.fetch(&format!("{}/missing", server.uri())).await,
        Err(FetchError::Http(404))
    );
    assert_eq!(
        f.fetch(&format!("{}/logo.png", server.uri())).await,
        Err(FetchError::Unsupported("image/png".into()))
    );
    let txt = f
        .fetch(&format!("{}/notes.txt", server.uri()))
        .await
        .unwrap();
    assert_eq!(
        (
            txt.markdown.as_str(),
            txt.title.as_str(),
            txt.content_type.as_str()
        ),
        ("Заметка\n\nвторая строка", "notes.txt", "text/plain")
    );
    assert!(matches!(
        f.fetch("not a url at all").await,
        Err(FetchError::InvalidUrl(_))
    ));
}

#[tokio::test]
async fn timeout_is_reported() {
    let server = MockServer::start().await;
    serve(
        &server,
        "/slow",
        html("<p>slow</p>").set_delay(Duration::from_secs(3)),
    )
    .await;
    let f = test_fetcher(FetchOptions {
        timeout: Duration::from_millis(300),
        ..Default::default()
    });
    assert_eq!(
        f.fetch(&format!("{}/slow", server.uri())).await,
        Err(FetchError::Timeout)
    );
}

#[tokio::test]
async fn private_network_is_forbidden_by_default() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(html("<p>секрет роутера</p>"))
        .expect(0)
        .mount(&server)
        .await;
    let port = server.address().port();
    let f = Fetcher::new(FetchOptions::default());
    // IP-литерал внутренней сети — отказ до запроса.
    assert!(matches!(
        f.fetch(&format!("http://127.0.0.1:{port}/")).await,
        Err(FetchError::Forbidden(_))
    ));
    assert!(matches!(
        f.fetch("http://192.168.50.38:8443/ready").await,
        Err(FetchError::Forbidden(_))
    ));
    // Локальное имя (резолвер, отбрасывающий внутренние адреса, проверен юнит-тестом в http.rs).
    match f.fetch(&format!("http://localhost:{port}/")).await {
        Err(FetchError::Forbidden(m)) => assert!(m.contains("localhost"), "{m}"),
        other => panic!("{other:?}"),
    }
    // Не http(s).
    assert!(matches!(
        f.fetch("file:///C:/Windows/win.ini").await,
        Err(FetchError::Forbidden(_))
    ));
}
