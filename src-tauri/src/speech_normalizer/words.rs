//! Keep addresses and identifiers opaque to the math reader. Match whole tokens
//! in priority order so, for example, a slash inside /api/v2/3 is never a fraction.
use super::numbers::{digits, fraction, integer, number, percent};
use regex::{Captures, Regex};
use std::sync::LazyLock;

static TOKEN: LazyLock<Regex> = LazyLock::new(|| Regex::new(concat!(
    r"(?P<literal>(?:https?://|www\.)[^\s<>()]+|[\w.+-]+@[\w.-]+\.[a-zA-Z]+|",
    r"\b[0-9]{4}-[0-9]{2}-[0-9]{2}\b|\b[0-9]{1,2}[./][0-9]{1,2}[./][0-9]{2,4}\b|",
    r"\b[vV]?[0-9]+(?:\.[0-9]+){2,}(?:[-+][A-Za-z0-9.-]+)?|",
    r"[A-Za-z0-9_]+(?:/[A-Za-z0-9_.-]+)+[A-Za-z0-9_-]|/[A-Za-z_][A-Za-z0-9_./-]*|",
    r"[A-Za-z_][A-Za-z0-9_]*(?:[._][A-Za-z0-9_]+)+|--[a-zA-Z][a-zA-Z0-9-]*)|",
    r"(?P<tech>(?i:\bwi[-‐‑‒–—]?fi\b)|[A-Za-z_][A-Za-z0-9_]*(?:[-‐‑][A-Za-z0-9_]+)*)|",
    r"(?P<math>[-−+]?[0-9]|√|π|∞)|(?P<word>[\p{L}\p{N}_]+)"
)).unwrap());
static ATOM: LazyLock<Regex> = LazyLock::new(|| Regex::new(concat!(
    r"^(?P<sign>[-−+]?)\s*(?P<root>√\s*)?(?P<value>[0-9]+(?:[.,][0-9]+)?|π|∞)",
    r"(?P<modifier>\s*\^\s*[-−+]?[0-9]+|[⁰¹²³⁴⁵⁶⁷⁸⁹⁻]+|\s*%|/[0-9]+)?"
)).unwrap());
static OP: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*(>=|<=|!=|[=<>≥≤≠×*·÷/+−-])\s*").unwrap());
static LEGACY_TEX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)\\\((.*?)\\\)|\\\[(.*?)\\\]").unwrap());
static FRAC: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\\(?:d?frac|tfrac)\s*\{([0-9]+)\}\s*\{([0-9]+)\}").unwrap());
static ROOT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\\sqrt\s*\{([0-9]+(?:[.,][0-9]+)?)\}").unwrap());
static POWER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\^\s*\{([-+]?[0-9]+)\}").unwrap());
static TEX_COMMAND: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\\[A-Za-z]+|\\%").unwrap());

pub(super) fn legacy_tex(s:&str) -> std::borrow::Cow<'_,str> {
    LEGACY_TEX.replace_all(s,|c:&Captures| format!(" ${}$ ",c.get(1).or_else(|| c.get(2)).unwrap().as_str()))
}
pub(super) fn math_tex(src:&str) -> String {
    let s=FRAC.replace_all(src,"$1/$2");
    let s=ROOT.replace_all(&s,"√$1");
    let s=POWER.replace_all(&s,"^$1");
    let s=TEX_COMMAND.replace_all(&s,|c:&Captures| match &c[0] {
        "\\times"|"\\cdot"=>"×","\\div"=>"÷","\\pi"=>"π","\\infty"=>"∞","\\ge"|"\\geq"=>"≥","\\le"|"\\leq"=>"≤","\\neq"=>"≠","\\%"=>"%",other=>other
    }.to_owned());
    if s.contains(['\\','{','}']) { return literal(src) }
    prose(&s.replace('('," открыть скобку ").replace(')'," закрыть скобку "))
}

fn acronym(s:&str) -> Option<String> {
    let word=match s.to_ascii_uppercase().as_str() {
        "WIFI"|"WI-FI"|"WI‑FI"|"WI‐FI"|"WI‒FI"|"WI–FI"|"WI—FI"=>"вай-фай",
        "VPN"=>"ви-пи-эн","API"=>"эй-пи-ай","HTTP"=>"эйч-ти-ти-пи","HTTPS"=>"эйч-ти-ти-пи-эс",
        "USB"=>"ю-эс-би","CPU"=>"си-пи-ю","GPU"=>"джи-пи-ю","DNS"=>"ди-эн-эс","SSH"=>"эс-эс-эйч",
        "TLS"=>"ти-эл-эс","TCP"=>"ти-си-пи","IP"=>"ай-пи","LLM"=>"эл-эл-эм","TTS"=>"ти-ти-эс",
        "STT"=>"эс-ти-ти","AI"=>"эй-ай","RAM"=>"рэм","SQL"=>"эс-кью-эл","SSD"=>"эс-эс-ди",
        _=>return None
    };
    Some(word.into())
}

fn letter(c:u8)-> &'static str {
    const LETTERS:[&str;26]=["эй","би","си","ди","и","эф","джи","эйч","ай","джей","кей","эл","эм","эн","оу","пи","кью","ар","эс","ти","ю","ви","дабл-ю","экс","уай","зед"];
    LETTERS[(c.to_ascii_uppercase()-b'A') as usize]
}

fn technical(s:&str) -> String {
    if let Some(name)=acronym(s) {return name}
    if s.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit()) && s.len() <= 12 {
        let mut parts=Vec::new();
        let mut rest=s;
        while let Some(byte)=rest.bytes().next() {
            if byte.is_ascii_digit() {
                let end=rest.find(|c:char| !c.is_ascii_digit()).unwrap_or(rest.len());
                parts.push(number(&rest[..end],"n"));
                rest=&rest[end..];
            } else {
                parts.push(letter(byte).into());
                rest=&rest[1..];
            }
        }
        return parts.join(" ")
    }
    if s.contains('_') || s.contains(['-', '‐','‑']) {return literal(s)}
    s.to_owned()
}

pub(super) fn literal(s:&str) -> String {
    let mut parts=Vec::new();
    let mut chars=s.chars().peekable();
    while let Some(c)=chars.next() {
        if c.is_whitespace(){continue}
        if c.is_alphabetic() || c.is_ascii_digit() {
            let mut chunk=c.to_string();
            while chars.peek().is_some_and(|x| if c.is_ascii_digit(){x.is_ascii_digit()}else{x.is_alphabetic()}) {
                chunk.push(chars.next().unwrap());
            }
            parts.push(if c.is_ascii_digit(){ digits(&chunk) } else if chunk.is_ascii() {technical(&chunk)} else {chunk});
            continue
        }
        let name=match c {
            '/'=>"слэш",'\\'=>"обратный слэш",'.'=>"точка",','=>"запятая",':'=>"двоеточие",';'=>"точка с запятой",
            '-'|'‐'|'‑'=>"дефис",'_'=>"нижнее подчёркивание",'='=>"знак равно",'^'=>"знак степени",'*'=>"звёздочка",
            '&'=>"амперсанд",'@'=>"собака",'#'=>"решётка",'?'=>"вопросительный знак",'!'=>"восклицательный знак",
            '+'=>"плюс",'%'=>"процент",'|'=>"вертикальная черта",'~'=>"тильда",'$'=>"доллар",'"'=>"кавычка",'\''=>"апостроф",
            '('=>"открыть скобку",')'=>"закрыть скобку",'['=>"открыть квадратную скобку",']'=>"закрыть квадратную скобку",
            '{'=>"открыть фигурную скобку",'}'=>"закрыть фигурную скобку",'<'=>"левая угловая скобка",'>'=>"правая угловая скобка",
            '`'=>"обратная кавычка",_=>{parts.push(c.to_string());continue}
        };
        parts.push(name.into());
    }
    parts.join(" ")
}

fn atom(s:&str,case:&str)->Option<(usize,String,bool)> {
    let c=ATOM.captures(s)?;
    let root=c.name("root").is_some();
    let case=if root {"g"} else {case};
    let val=&c["value"];
    let sign=match &c["sign"] {"-"|"−"=>"минус ","+"=>"плюс ",_=>""};
    let mut spoken=match val {"π"=>"пи".into(),"∞"=>match case {"d"|"g"=>"бесконечности",_=>"бесконечность"}.into(), _=>number(val,case)};
    let modifier=c.name("modifier").map(|m|m.as_str().trim()).unwrap_or_default();
    if let Some(exp)=modifier.strip_prefix('^') {
        let exp=exp.trim();
        let exponent=exp.trim_start_matches(['-','−','+']);
        let ordinal=integer(exponent,"p","f",true)?;
        let preposition=if exponent=="2" && exp==exponent {"во"}else{"в"};
        let sign=if exp.starts_with(['-','−']) {"минус "}else{""};
        spoken=format!("{spoken} {preposition} {sign}{ordinal} степени");
    } else if let Some(den)=modifier.strip_prefix('/') {
        if !val.bytes().all(|b|b.is_ascii_digit()){return None}
        spoken=fraction(val,den,case).unwrap_or_else(||format!("{spoken} разделить на {}",number(den,"n")));
    } else if modifier=="%" {spoken.push(' ');spoken.push_str(percent(val,case))}
    else if !modifier.is_empty() {
        if modifier=="²" {spoken.push_str(" в квадрате")} else if modifier=="³" {spoken.push_str(" в кубе")} else {
            let exp=modifier.chars().map(|c| "⁰¹²³⁴⁵⁶⁷⁸⁹⁻".chars().position(|x|x==c).map(|i| if i==10 {'-'} else {(b'0'+i as u8) as char}).unwrap()).collect::<String>();
            let sign=if exp.starts_with('-'){"минус "}else{""};
            let ordinal=integer(exp.trim_start_matches('-'),"p","f",true)?;
            spoken=format!("{spoken} в {sign}{ordinal} степени")
        }
    }
    if root {spoken=format!("квадратный корень из {spoken}")}
    Some((c.get(0)?.end(),format!("{sign}{spoken}"),root))
}

fn expression(s:&str)->Option<(usize,String)> {
    let (mut end,mut spoken,mut root)=atom(s,"n")?;
    while let Some(op)=OP.captures(&s[end..]) {
        let (word,case)=match &op[1] {
            "=" if root=>("равен","d"),"="=>("равно","d"),"≠"|"!="=>("не равно","d"),
            ">"=>("больше","g"),"<"=>("меньше","g"),"≥"|">="=>("больше или равно","d"),"≤"|"<="=>("меньше или равно","d"),
            "*"|"×"|"·"=>("умножить на","n"),"/"|"÷"=>("разделить на","n"),"+"=>("плюс","n"),_=>("минус","n")
        };
        let offset=end+op.get(0)?.end();
        let Some((size,next,_))=atom(&s[offset..],case) else {break};
        spoken.push_str(&format!(" {word} {next}"));
        end=offset+size;root=false;
    }
    Some((end,spoken))
}

pub(super) fn prose(s:&str) -> String {
    let mut out=String::new();
    let mut i=0;
    while let Some(c)=TOKEN.captures_at(s,i) {
        let m=c.get(0).unwrap();
        out.push_str(&s[i..m.start()]); i=m.end();
        let token=m.as_str();
        if c.name("literal").is_some() {
            let trimmed=token.trim_end_matches(['.',',',';',':','!','?']);
            out.push_str(&literal(trimmed));
            out.push_str(&token[trimmed.len()..]);
        }else if c.name("tech").is_some() {
            out.push_str(&technical(token));
            if let Some(mut chars)=s.get(i..).map(str::chars) {
                if let Some(h @ ('-'|'‐'|'‑'|'‒'|'–'|'—'))=chars.next() {
                    if chars.next().is_some_and(|v|v.is_alphabetic()&&!v.is_ascii()) {i+=h.len_utf8();out.push(' ')}
                }
            }
        } else if c.name("math").is_some() {
            if let Some((n,spoken))=expression(&s[m.start()..]) {
                i=m.start()+n;out.push_str(&spoken);
            }else{out.push_str(token)}
        }else{out.push_str(token)}
    }
    out.push_str(&s[i..]);
    out
}
