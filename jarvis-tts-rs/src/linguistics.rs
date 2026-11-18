//! Inference for the three RUNorm checkpoints. The acoustic tokenizer is separate.
use anyhow::{Context, Result, ensure};
use candle_core::{DType, Device, Tensor};
use candle_nn::{Linear, Module, VarBuilder, linear};
use candle_transformers::models::{bert, t5};
use regex::Regex;
use std::{
    path::Path,
    sync::{
        LazyLock,
        atomic::{AtomicBool, Ordering},
    },
};
use tokenizers::Tokenizer;

pub fn weights(dir: &Path) -> Result<VarBuilder<'static>> {
    // Read immutable operator-owned assets without mapping writable files into tensors.
    let data = std::fs::read(dir.join("model.safetensors"))?;
    Ok(VarBuilder::from_buffered_safetensors(
        data,
        DType::F32,
        &Device::Cpu,
    )?)
}
fn tokenizer(dir: &Path) -> Result<Tokenizer> {
    Tokenizer::from_file(dir.join("tokenizer.json")).map_err(anyhow::Error::msg)
}
struct Generator {
    model: t5::T5ForConditionalGeneration,
    tok: Tokenizer,
    start: u32,
    eos: u32,
}
impl Generator {
    fn load(dir: &Path) -> Result<Self> {
        let mut cfg: t5::Config = serde_json::from_slice(&std::fs::read(dir.join("config.json"))?)?;
        let vb = weights(dir)?;
        if vb.contains_tensor("lm_head.weight") {
            cfg.tie_word_embeddings = false;
        }
        cfg.use_cache = true;
        Ok(Self {
            model: t5::T5ForConditionalGeneration::load(vb, &cfg)?,
            tok: tokenizer(dir)?,
            start: cfg.decoder_start_token_id.unwrap_or(0) as u32,
            eos: cfg.eos_token_id as u32,
        })
    }
    fn generate(&mut self, prompt: &str, limit: usize, stop: &AtomicBool) -> Result<String> {
        let ids = self.tok.encode(prompt, true).map_err(anyhow::Error::msg)?;
        self.model.clear_kv_cache();
        let input = Tensor::new(ids.get_ids(), &Device::Cpu)?.unsqueeze(0)?;
        let encoded = self.model.encode(&input)?;
        let mut prev = self.start;
        let mut output = Vec::new();
        for _ in 0..limit {
            ensure!(!stop.load(Ordering::Relaxed), "cancelled");
            let logits = self
                .model
                .decode(&Tensor::new(&[[prev]], &Device::Cpu)?, &encoded)?;
            prev = logits.argmax(1)?.get(0)?.to_scalar()?;
            if prev == self.eos {
                break;
            }
            output.push(prev);
        }
        self.tok.decode(&output, false).map_err(anyhow::Error::msg)
    }
}
pub struct Normalizer {
    latin: Generator,
    inflect: Generator,
    tagger: bert::BertModel,
    head: Linear,
    tok: Tokenizer,
}
static LATIN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[A-Za-z]+(?:[ '-]+[A-Za-z]+)*").unwrap());
static DIGITS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[0-9]+").unwrap());
static SLOTS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<extra_id_(\d+)>").unwrap());
static ABBR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b[А-ЯЁ]{2,}\b").unwrap());
const ALPHABET: &str = "АБВГДЕЁЖЗИЙКЛМНОПРСТУФХЦЧШЩЪЫЬЭЮЯ";
const NAMES: [&str; 33] = [
    "а",
    "бэ",
    "вэ",
    "гэ",
    "дэ",
    "е",
    "ё",
    "жэ",
    "зэ",
    "и",
    "и краткое",
    "ка",
    "эль",
    "эм",
    "эн",
    "о",
    "пэ",
    "эр",
    "эс",
    "тэ",
    "у",
    "эф",
    "ха",
    "цэ",
    "че",
    "ша",
    "ща",
    "твёрдый знак",
    "ы",
    "мягкий знак",
    "э",
    "ю",
    "я",
];
fn numbers(s: &str) -> String {
    s.parse::<i64>()
        .ok()
        .filter(|_| s.len() < 13)
        .and_then(|n| num2words2_core::to_words(n, "ru").ok())
        .unwrap_or_else(|| {
            s.bytes()
                .map(|c| {
                    [
                        "ноль",
                        "один",
                        "два",
                        "три",
                        "четыре",
                        "пять",
                        "шесть",
                        "семь",
                        "восемь",
                        "девять",
                    ][(c - b'0') as usize]
                })
                .collect::<Vec<_>>()
                .join(" ")
        })
}
impl Normalizer {
    pub fn load(dir: &Path) -> Result<Self> {
        let path = dir.join("tagger");
        let vb = weights(&path)?;
        let cfg: bert::Config = serde_json::from_slice(&std::fs::read(path.join("config.json"))?)?;
        Ok(Self {
            tagger: bert::BertModel::load(vb.pp("bert"), &cfg)?,
            head: linear(cfg.hidden_size, 16, vb.pp("classifier"))?,
            tok: tokenizer(&path)?,
            latin: Generator::load(&dir.join("kirillizator"))?,
            inflect: Generator::load(&dir.join("normalizer-small"))?,
        })
    }
    pub fn sentence(&mut self, s: &str, stop: &AtomicBool) -> Result<String> {
        let s = ABBR.replace_all(s, |c: &regex::Captures| {
            c[0].chars()
                .filter_map(|l| ALPHABET.chars().position(|v| v == l).map(|i| NAMES[i]))
                .collect::<Vec<_>>()
                .join(" ")
        });
        let enc = self
            .tok
            .encode(s.as_ref(), true)
            .map_err(anyhow::Error::msg)?;
        let mut spans: Vec<(usize, usize)> = Vec::new();
        for (part, offsets) in enc.get_ids().chunks(500).zip(enc.get_offsets().chunks(500)) {
            let ids = Tensor::new(part, &Device::Cpu)?.unsqueeze(0)?;
            let tags = self
                .head
                .forward(&self.tagger.forward(&ids, &ids.zeros_like()?, None)?)?
                .argmax(2)?
                .get(0)?
                .to_vec1::<u32>()?;
            for (&(a, b), tag) in offsets.iter().zip(tags) {
                if a == b || tag == 0 || s[a..b].chars().any(|c| c.is_ascii_alphabetic()) {
                    continue;
                }
                if let Some(prev) = spans.last_mut()
                    && prev.1 == a
                {
                    prev.1 = b;
                } else {
                    spans.push((a, b));
                }
            }
        }
        for d in DIGITS.find_iter(&s) {
            if !spans.iter().any(|&(a, b)| a < d.end() && b > d.start()) {
                spans.push((d.start(), d.end()));
            }
        }
        spans.sort_unstable();
        let mut text = s.to_string();
        if !spans.is_empty() {
            let mut prompt = String::from("<SC1>");
            let mut pos = 0;
            let mut replacements = Vec::new();
            for (i, &(a, b)) in spans.iter().enumerate() {
                let label = DIGITS
                    .replace_all(&s[a..b], |c: &regex::Captures| numbers(&c[0]))
                    .into_owned();
                prompt.push_str(&s[pos..a]);
                prompt.push_str(&format!("[{label}]<extra_id_{i}>"));
                pos = b;
                replacements.push(label);
            }
            prompt.push_str(&s[pos..]);
            let pred = self.inflect.generate(&prompt, 256, stop)?;
            let tags = SLOTS.captures_iter(&pred).collect::<Vec<_>>();
            for (i, c) in tags.iter().enumerate() {
                let idx = c[1].parse::<usize>()?;
                let end = tags
                    .get(i + 1)
                    .map_or(pred.len(), |n| n.get(0).unwrap().start());
                let word = pred[c.get(0).unwrap().end()..end].trim();
                if !word.is_empty() && idx < replacements.len() {
                    replacements[idx] = word.to_owned();
                }
            }
            for (&(a, b), word) in spans.iter().zip(replacements).rev() {
                text.replace_range(a..b, &word);
            }
        }
        let mut out = String::new();
        let mut pos = 0;
        for m in LATIN.find_iter(&text) {
            out.push_str(&text[pos..m.start()]);
            let translit = self.latin.generate(
                &format!("<pad><pad><pad>{}", m.as_str().to_lowercase()),
                128,
                stop,
            )?;
            let translit = translit.replace("<pad>", "").replace("<s>", "");
            out.push_str(translit.trim());
            pos = m.end();
        }
        out.push_str(&text[pos..]);
        ensure!(out.len() < 48000, "normalized text too long");
        Ok(out)
    }
}
pub fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    serde_json::from_reader(std::io::BufReader::new(std::fs::File::open(path)?))
        .with_context(|| format!("read {}", path.display()))
}
