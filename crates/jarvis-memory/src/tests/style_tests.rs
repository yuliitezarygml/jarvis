//! Категория `style` (манера общения пользователя): хранение, блок «Как с ним общаться», рефлектор.

use jarvis_core::{CancellationToken, Tool};
use std::sync::Arc;

use super::{MockModel, exec, temp_mem};
use crate::*;

fn style(text: &str) -> NewFact {
    NewFact::inferred(text, 0.7).with_category(category::STYLE)
}

#[test]
fn style_category_roundtrip() {
    assert!(category::ALL.contains(&category::STYLE));
    let (_d, mem) = temp_mem();
    let id = mem.upsert_fact(style("Часто матерится, когда что-то не работает")).unwrap().id;
    let f = mem.get_fact(id).unwrap().unwrap();
    assert_eq!(f.category, "style");
    assert_eq!(f.source, FactSource::Inferred);
    // русские синонимы категории приводятся к style
    let id2 = mem.upsert_fact(NewFact::explicit("Любит, когда ему отвечают подколом").with_category("Стиль")).unwrap().id;
    assert_eq!(mem.get_fact(id2).unwrap().unwrap().category, "style");
    // правка из «Памяти» сохраняет категорию
    let mut f = mem.get_fact(id).unwrap().unwrap();
    f.text = "Матерится, когда что-то долго грузится".into();
    mem.update_fact(&f).unwrap();
    let f = mem.get_fact(id).unwrap().unwrap();
    assert_eq!((f.category.as_str(), f.text.as_str()), ("style", "Матерится, когда что-то долго грузится"));
    assert!(mem.list_facts(10, 0).unwrap().iter().all(|f| f.category == "style"));
    // и enum инструмента memory_remember берёт её из ALL
    let spec = Tool::spec(&RememberTool { mem: mem.clone() });
    let cats = spec.parameters["properties"]["category"]["enum"].as_array().unwrap();
    assert!(cats.iter().any(|c| c == "style"), "{cats:?}");
}

#[test]
fn style_facts_always_in_context_in_own_section() {
    let (_d, mem) = temp_mem();
    let sir = mem.upsert_fact(NewFact::explicit("Обращаться к пользователю «сэр»").with_category("preference")).unwrap().id;
    let coffee = mem.upsert_fact(NewFact::explicit("Пьёт чёрный кофе без сахара")).unwrap().id;
    // важный style-факт и style-факт со словом «кофе» — не должны попасть в общие факты
    let angry = mem.upsert_fact(style("Злится, если переспрашивать").with_important(true)).unwrap().id;
    let coffee_style = mem.upsert_fact(style("Ругается, когда кофемашина не варит кофе")).unwrap().id;

    // запрос не про стиль — style всё равно в контексте
    let ctx = mem.retrieve_context("закажи мне кофе", None, None).unwrap();
    let style_ids: Vec<i64> = ctx.style.iter().map(|f| f.id).collect();
    assert_eq!(style_ids[0], angry, "важный — первым");
    assert!(style_ids.contains(&coffee_style));
    let fact_ids: Vec<i64> = ctx.facts.iter().map(|f| f.id).collect();
    assert!(fact_ids.contains(&sir) && fact_ids.contains(&coffee), "{fact_ids:?}");
    assert!(ctx.facts.iter().all(|f| f.category != "style"), "style не дублируется в фактах: {:?}", ctx.facts);
    // и в важные они не идут
    assert!(mem.important_facts(10).unwrap().iter().all(|f| f.category != "style"));

    let block = ctx.render(DEFAULT_BUDGET_TOKENS);
    assert!(block.starts_with(&format!("Память:\n{STYLE_HEADING}\n- Злится, если переспрашивать")), "{block}");
    assert_eq!(block.matches("Ругается, когда кофемашина").count(), 1, "{block}");
    let facts_at = block.find("Факты о пользователе:").expect("секция фактов");
    assert!(block.find(STYLE_HEADING).unwrap() < facts_at);
    assert!(!block[facts_at..].contains("Злится"), "{block}");

    // mark_used учитывает и style
    mem.mark_used(&ctx.refs()).unwrap();
    assert_eq!(mem.get_fact(angry).unwrap().unwrap().use_count, 1);

    // пустой запрос — style и важные всё равно есть
    let ctx = mem.retrieve_context("", None, None).unwrap();
    assert_eq!(ctx.style.len(), 2);
    assert!(!ctx.is_empty());
}

#[test]
fn style_block_limited_to_five_newest() {
    let (_d, mem) = temp_mem();
    let texts = [
        "Любит чёрный юмор",
        "Часто матерится, когда что-то не работает",
        "Злится, если ассистент медлит",
        "Предпочитает общение на «ты»",
        "Любит подкалывать ассистента",
        "Не любит длинные объяснения",
        "Говорит «ёпта», когда удивлён",
    ];
    let mut ids = Vec::new();
    for (i, t) in texts.iter().enumerate() {
        let id = mem.upsert_fact(style(t)).unwrap().id;
        // разное время обновления: последний — самый свежий
        exec(&mem, "UPDATE facts SET updated_at = ?1 WHERE id = ?2", rusqlite::params![1_000 + i as i64, id]);
        ids.push(id);
    }
    // старый, но часто используемый факт не вытесняет свежие навсегда
    exec(&mem, "UPDATE facts SET use_count = 50 WHERE id = ?1", [ids[0]]);
    // факт с почти нулевым доверием в контекст не идёт
    exec(&mem, "UPDATE facts SET confidence = 0.05 WHERE id = ?1", [ids[6]]);

    let ctx = mem.retrieve_context("что-нибудь", None, None).unwrap();
    assert_eq!(ctx.style.len(), STYLE_FACTS_K);
    let got: Vec<i64> = ctx.style.iter().map(|f| f.id).collect();
    assert_eq!(got, vec![ids[5], ids[4], ids[3], ids[2], ids[1]], "свежие первыми");
    assert_eq!(mem.style_facts(10).unwrap().len(), 7, "style_facts без порога доверия");

    let block = ctx.render(DEFAULT_BUDGET_TOKENS);
    let section: Vec<&str> = block.lines().skip(2).take_while(|l| l.starts_with("- ")).collect();
    assert_eq!(section.len(), 5, "{block}");

    // вручную собранный контекст с лишними style-фактами: в блоке не больше пяти
    let all = mem.list_facts(100, 0).unwrap();
    let manual = MemoryContext { style: all.clone(), facts: all, ..Default::default() };
    let out = manual.render(100_000);
    assert_eq!(out.lines().filter(|l| l.starts_with("- ")).count(), 5, "{out}");
    assert!(!out.contains("Факты о пользователе:"), "style из facts не уходит в общие: {out}");

    // бюджет соблюдается и с блоком style
    for budget in [0usize, 10, 40, 100, 300, 1500] {
        let out = ctx.render(budget);
        assert!(estimate_tokens(&out) <= budget, "бюджет {budget}: {out}");
    }

    // отдельный блок для промпта
    let only = ctx.render_style(1500);
    assert!(only.starts_with(STYLE_HEADING), "{only}");
    assert_eq!(only.lines().count(), 6);
    assert_eq!(MemoryContext::default().render_style(1500), "");
}

const STYLE_JSON: &str = r#"{
  "facts": [
    {"text": "Часто матерится, когда что-то не работает", "category": "style", "confidence": 0.8},
    {"text": "Любит, когда ему отвечают подколом", "category": "STYLE"},
    {"text": "Живёт в Кишинёве", "category": "город"}
  ],
  "lessons": [],
  "playbooks": [],
  "slang": ["Ёпта", "«красава»", "жиза!", "ёпта", "...", "x", "", 5, {"word": "по кайфу"}]
}"#;

#[test]
fn reflector_parses_style_facts_and_slang() {
    let r = parse_reflection(STYLE_JSON).unwrap();
    assert_eq!(r.facts.len(), 3);
    assert_eq!(r.facts[0].category, "style");
    assert_eq!(r.facts[1].category, "style", "регистр категории не важен");
    assert_eq!(r.facts[2].category, "general", "неизвестная категория → general, как раньше");
    assert!(r.facts.iter().all(|f| f.source == FactSource::Inferred));
    assert_eq!(r.slang, vec!["ёпта", "красава", "жиза", "по кайфу"]);
    // 9 сырых словечек → 4 (дубль, заглушка, слишком короткое, пустое, число)
    assert_eq!(r.dropped, 5);
    assert!(!r.is_empty());

    // ответ только со slang — тоже ответ
    let only = parse_reflection(r#"{"slang": ["жиза"]}"#).unwrap();
    assert_eq!(only.slang, vec!["жиза"]);
    assert!(only.facts.is_empty());
    assert!(extract_json(r#"Вот: {"slang": []}"#).is_some());
    let one = parse_reflection(r#"{"slang": "красава"}"#).unwrap();
    assert_eq!(one.slang, vec!["красава"]);
    // оборванный ответ со slang чинится
    let cut = parse_reflection(r#"{"facts":[],"slang":["ёпта","жи"#).unwrap();
    assert_eq!(cut.slang, vec!["ёпта", "жи"]);
    // без slang — пустой список, старые ответы разбираются как раньше
    let old = parse_reflection(r#"{"facts": [{"text": "Пьёт чай", "category": "habit"}]}"#).unwrap();
    assert!(old.slang.is_empty());
    // сериализованный результат без поля slang читается
    let v: ReflectionResult = serde_json::from_str(r#"{"facts":[],"lessons":[],"playbooks":[],"dropped":0}"#).unwrap();
    assert!(v.slang.is_empty());
}

#[test]
fn reflect_prompt_mentions_style_rules() {
    for needle in ["\"style\"", "slang", "ёпта", "[голос:", "[речь:", "нейтрально", "без оценок и оскорблений"] {
        assert!(REFLECT_SYSTEM_PROMPT.contains(needle), "в промпте нет {needle}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reflect_and_apply_stores_style_but_not_slang() {
    let (_d, mem) = temp_mem();
    let r = Reflector::new(Arc::new(MockModel::ok(STYLE_JSON)));
    let t = TaskTranscript {
        user_request: "да ёпта, опять не работает [голос: громче обычного на 11 дБ] [речь: мат]".into(),
        final_answer: Some("Сейчас разберусь.".into()),
        ..Default::default()
    };
    let (res, rep) = r.reflect_and_apply(&mem, &t, CancellationToken::new()).await.unwrap();
    assert_eq!(res.slang.len(), 4);
    assert_eq!(rep.facts.len(), 3);
    let facts = mem.list_facts(10, 0).unwrap();
    assert_eq!(facts.iter().filter(|f| f.category == "style").count(), 2);
    assert!(facts.iter().all(|f| !f.text.contains("ёпта")), "словечки в факты не пишутся");
    // повторная рефлексия видит style в «уже есть в памяти» и не дублирует
    let model = Arc::new(MockModel::ok(STYLE_JSON));
    let r = Reflector::new(model.clone());
    let (_, rep2) = r.reflect_and_apply(&mem, &t, CancellationToken::new()).await.unwrap();
    assert!(rep2.facts.iter().all(|s| s.status != SaveStatus::Inserted), "{rep2:?}");
    let req = model.last.lock().unwrap().clone().unwrap();
    assert!(req.messages[1].content.contains("Как с ним общаться"), "{}", req.messages[1].content);
    assert!(req.messages[1].content.contains("[голос: громче обычного на 11 дБ]"));
}
