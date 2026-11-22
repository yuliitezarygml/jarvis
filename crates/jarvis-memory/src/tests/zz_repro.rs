use crate::*;
fn run(two: bool, lessons: bool, reads: bool) -> String {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("m.db");
    let a = Memory::open(&p).unwrap();
    let b = if two { Memory::open(&p).unwrap() } else { a.clone() };
    let mut hs = vec![];
    for t in 0..8 {
        let m = if t % 2 == 0 { a.clone() } else { b.clone() };
        hs.push(std::thread::spawn(move || {
            for i in 0..20 {
                let n = t * 20 + i;
                let w = format!("zq{}", n.to_string().chars().map(|c| (b'b' + c as u8 - b'0') as char).collect::<String>());
                if lessons { m.add_lesson(lesson_stub_from_tool_error("t", None, &format!("err {w}"))).unwrap(); }
                else { m.upsert_fact(NewFact::explicit(format!("вызов {w} err"))).unwrap(); }
                if reads { let _ = m.retrieve_context("значение", Some("t"), None).unwrap(); }
            }
        }));
    }
    for h in hs { h.join().unwrap(); }
    let r1 = a.check_integrity().is_ok(); let fresh = Memory::open(&p).unwrap().check_integrity().is_ok(); let rb = b.check_integrity().is_ok(); if r1 && fresh && rb { "ok".into() } else { println!("  a={r1} b={rb} fresh={fresh}"); "bad".into() }
}
#[test]
fn zz_repro() {
    for two in [false, true] { for lessons in [false, true] { for reads in [false, true] {
        let mut bad = 0;
        for _ in 0..5 { if run(two, lessons, reads) != "ok" { bad += 1; } }
        println!("two={two} lessons={lessons} reads={reads} bad={bad}/5");
    }}}
}
