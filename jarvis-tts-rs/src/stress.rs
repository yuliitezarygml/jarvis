use crate::linguistics::{read_json, weights};
use anyhow::{Context, Result};
use candle_core::{Device, IndexOp, Tensor};
use candle_nn::{Linear, Module, linear};
use candle_transformers::models::bert;
use pocket_native::weights::SafetensorsFile;
use regex::Regex;
use serde::Deserialize;
use std::{collections::HashMap, path::Path, sync::LazyLock};

static WORD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[А-Яа-яёЁ\x{301}]+").unwrap());
static TOK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[/?HOMO\]|[\p{L}\p{N}]+|[^\s]").unwrap());
const VOWELS: &str = "аоуыэиеяёю";
#[derive(Deserialize)]
struct Dictionary {
    grams: HashMap<String, usize>,
    exceptions: HashMap<String, (i32, i32)>,
}
struct Layer {
    w: Vec<f32>,
    b: Vec<f32>,
}
struct Head(Vec<Layer>);
impl Head {
    fn load(file: &SafetensorsFile, name: &str) -> Result<Self> {
        let mut layers = vec![];
        for i in [0, 2, 4, 6] {
            let (w, _) = file.read_f32(&format!("{name}.{i}.weight"))?;
            let (b, _) = file.read_f32(&format!("{name}.{i}.bias"))?;
            layers.push(Layer { w, b });
        }
        Ok(Self(layers))
    }
    fn predict(&self, input: &[f32]) -> Option<usize> {
        let mut v = input.to_owned();
        for (i, l) in self.0.iter().enumerate() {
            v =
                l.w.chunks_exact(v.len())
                    .zip(&l.b)
                    .map(|(w, b)| {
                        let x = w.iter().zip(&v).fold(*b, |n, (a, b)| n + a * b);
                        if i < 3 { x.max(0.) } else { x }
                    })
                    .collect();
        }
        let (index, &best) = v.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1))?;
        (v.iter().map(|x| (x - best).exp()).sum::<f32>() < 2.).then_some(index)
    }
}
struct Homographs {
    bert: bert::BertModel,
    hidden: Linear,
    head: Linear,
    vocab: HashMap<String, u32>,
    variants: HashMap<String, Vec<String>>,
    phrases: HashMap<String, Vec<(String, String)>>,
}
impl Homographs {
    fn load(dir: &Path) -> Result<Self> {
        let cfg = read_json(&dir.join("config.json"))?;
        let vb = weights(dir)?;
        let bert = bert::BertModel::load(vb.pp("bert"), &cfg)?;
        Ok(Self {
            bert,
            hidden: linear(624, 256, vb.pp("homo_clf.0"))?,
            head: linear(256, 1, vb.pp("homo_clf.3"))?,
            vocab: std::fs::read_to_string(dir.join("vocab.txt"))?
                .lines()
                .enumerate()
                .map(|(i, w)| (w.to_owned(), i as u32))
                .collect(),
            variants: read_json(&dir.join("variants.json"))?,
            phrases: read_json(&dir.join("phrases.json"))?,
        })
    }
    fn resolve(&self, s: &str, m: regex::Match<'_>, word: &str) -> Result<Option<String>> {
        if !self.variants.contains_key(word) && !self.phrases.contains_key(word) {
            return Ok(None);
        }
        let left = s[..m.start()]
            .chars()
            .rev()
            .take(150)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<String>();
        let right = s[m.end()..].chars().take(150).collect::<String>();
        let context = format!("{} [HOMO] {word} [/HOMO] {}", left.trim(), right.trim());
        if let Some(rules) = self.phrases.get(word) {
            let ctx = context.to_lowercase();
            if let Some((_, v)) = rules
                .iter()
                .find(|(phrase, _)| !phrase.is_empty() && ctx.contains(phrase))
            {
                return Ok(Some(v.clone()));
            }
        }
        let Some(variants) = self.variants.get(word) else {
            return Ok(None);
        };
        let mut ids = vec![self.vocab["[CLS]"]];
        for m in TOK.find_iter(&context) {
            let token = m.as_str();
            if let Some(id) = self.vocab.get(token) {
                ids.push(*id);
                continue;
            }
            let mut rest = token;
            let start = ids.len();
            while !rest.is_empty() {
                let found = rest
                    .char_indices()
                    .map(|(i, c)| i + c.len_utf8())
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .find_map(|end| {
                        let mut key = String::new();
                        if start != ids.len() {
                            key.push_str("##")
                        }
                        key.push_str(&rest[..end]);
                        self.vocab.get(&key).map(|id| (end, *id))
                    });
                if let Some((end, id)) = found {
                    ids.push(id);
                    rest = &rest[end..]
                } else {
                    ids.truncate(start);
                    ids.push(self.vocab["[UNK]"]);
                    break;
                }
            }
        }
        ids.push(self.vocab["[SEP]"]);
        let a = ids
            .iter()
            .position(|i| *i == self.vocab["[HOMO]"])
            .context("start marker")?;
        let b = ids
            .iter()
            .position(|i| *i == self.vocab["[/HOMO]"])
            .context("end marker")?;
        let t = Tensor::new(ids.as_slice(), &Device::Cpu)?.unsqueeze(0)?;
        let h = self.bert.forward(&t, &t.zeros_like()?, None)?.get(0)?;
        let joint = Tensor::cat(&[h.get(a)?, h.i(a + 1..b)?.mean(0)?], 0)?.unsqueeze(0)?;
        let logit = self
            .head
            .forward(&self.hidden.forward(&joint)?.relu()?)?
            .flatten_all()?
            .get(0)?
            .to_scalar::<f32>()?;
        let mut sorted = variants.iter().collect::<Vec<_>>();
        sorted.sort();
        Ok(sorted.get(usize::from(logit > 0.)).map(|v| (*v).clone()))
    }
}
pub struct Accent {
    dict: Dictionary,
    emb: Vec<f32>,
    stress: Head,
    yo: Head,
    homo: Homographs,
}
impl Accent {
    pub fn load(dir: &Path) -> Result<Self> {
        let f = SafetensorsFile::open(&dir.join("stress.safetensors"))?;
        Ok(Self {
            dict: read_json(&dir.join("stress.json"))?,
            emb: f.read_f32("embedding.weight")?.0,
            stress: Head::load(&f, "stress_clf")?,
            yo: Head::load(&f, "yo_clf")?,
            homo: Homographs::load(&dir.join("homo"))?,
        })
    }
    pub fn mark(&self, s: &str) -> Result<String> {
        let mut out = String::new();
        let mut offset = 0;
        for m in WORD.find_iter(s) {
            out.push_str(&s[offset..m.start()]);
            offset = m.end();
            let raw = m.as_str();
            let word = raw.to_lowercase();
            if raw.contains('\u{301}') {
                out.push_str(raw);
                continue;
            }
            let chars = word.chars().collect::<Vec<_>>();
            let vowel = chars
                .iter()
                .enumerate()
                .filter(|(_, c)| VOWELS.contains(**c))
                .map(|(i, _)| i)
                .collect::<Vec<_>>();
            if vowel.is_empty() {
                out.push_str(raw);
                continue;
            }
            let (stress, yo) = if let Some(v) = self.homo.resolve(s, m, &word)? {
                let chars = v.chars().collect::<Vec<_>>();
                (
                    chars.iter().position(|c| *c == '+'),
                    v.replace('+', "").chars().position(|c| c == 'ё'),
                )
            } else if let Some(i) = chars.iter().position(|&c| c == 'ё') {
                (Some(i), Some(i))
            } else if let Some(&(a, b)) = self.dict.exceptions.get(&word) {
                (usize::try_from(a).ok(), usize::try_from(b).ok())
            } else {
                let mut v = [0.; 16];
                let mut n = 0.;
                let marked = format!("<{word}>");
                let boundaries = marked
                    .char_indices()
                    .map(|(i, _)| i)
                    .chain([marked.len()])
                    .collect::<Vec<_>>();
                for (i, &a) in boundaries.iter().enumerate() {
                    for &b in boundaries.iter().skip(i + 1).take(80) {
                        if let Some(idx) = self.dict.grams.get(&marked[a..b]) {
                            for (x, w) in v.iter_mut().zip(&self.emb[idx * 16..idx * 16 + 16]) {
                                *x += w;
                            }
                            n += 1.;
                        }
                    }
                }
                if n > 0. {
                    for x in &mut v {
                        *x /= n;
                    }
                }
                let stress = if vowel.len() == 1 {
                    Some(vowel[0])
                } else {
                    self.stress.predict(&v).and_then(|i| vowel.get(i).copied())
                };
                let yo = self
                    .yo
                    .predict(&v)
                    .and_then(|i| i.checked_sub(1))
                    .and_then(|i| {
                        chars
                            .iter()
                            .enumerate()
                            .filter(|(_, c)| **c == 'е')
                            .nth(i)
                            .map(|(i, _)| i)
                    })
                    .filter(|i| Some(*i) == stress);
                (stress, yo)
            };
            for (i, c) in raw.chars().enumerate() {
                out.push(if Some(i) == yo {
                    if c.is_uppercase() { 'Ё' } else { 'ё' }
                } else {
                    c
                });
                if Some(i) == stress {
                    out.push('\u{301}');
                }
            }
        }
        out.push_str(&s[offset..]);
        Ok(out)
    }
}
