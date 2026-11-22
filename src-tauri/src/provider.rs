use crate::storage::Settings;
use anyhow::{anyhow, bail, ensure, Context, Result};
use async_trait::async_trait;
use eventsource_stream::Eventsource;
use futures_util::StreamExt;
use reqwest::{Client, Url};
use serde_json::{json, Value};
use std::{collections::BTreeMap, time::Duration};
use tokio_util::sync::CancellationToken;

pub fn client() -> Result<Client> {
    Ok(Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(90))
        .build()?)
}
pub fn base(raw: &str) -> Result<Url> {
    let u = Url::parse(raw).context("Некорректный адрес API")?;
    ensure!(
        u.username().is_empty()
            && u.password().is_none()
            && u.query().is_none()
            && u.fragment().is_none(),
        "URL должен содержать только адрес и путь"
    );
    ensure!(
        u.scheme() == "https"
            || (u.scheme() == "http"
                && matches!(u.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"))),
        "Требуется HTTPS или localhost"
    );
    Ok(u)
}
fn credential(s: &Settings) -> Result<String> {
    let (host, key) = match s.provider.as_str() {
        "Groq" => ("GROQ_BASE_URL", "GROQ_API_KEY"),
        "OmniRouter" => ("OMNIROUTER_BASE_URL", "OMNIROUTER_API_KEY"),
        "OpenCode Zen" => ("OPENCODE_BASE_URL", "OPENCODE_API_KEY"),
        "Z.AI" => ("ZAI_BASE_URL", "ZAI_API_KEY"),
        "Совместимый" => ("COMPAT_BASE_URL", "COMPAT_API_KEY"),
        _ => bail!("Неизвестный провайдер"),
    };
    let trusted =
        base(&std::env::var(host).with_context(|| format!("Задайте {host} в backend.env"))?)?;
    ensure!(
        trusted.origin() == base(&s.url)?.origin(),
        "Для нового адреса подтвердите origin в backend.env и перезапустите приложение"
    );
    std::env::var(key).context("Ключ провайдера не настроен")
}
#[async_trait]
pub trait LlmProvider: Send + Sync {
    async fn models(&self, s: &Settings) -> Result<Vec<String>>;
    async fn stream(
        &self,
        s: &Settings,
        messages: &[Value],
        tools: &[Value],
        cancel: &CancellationToken,
        on_text: &mut (dyn for<'a> FnMut(&'a str) -> Result<()> + Send),
    ) -> Result<Vec<Value>>;
}
pub struct Compatible(pub Client);
impl Compatible {
    fn request(&self, s: &Settings, path: &str, get: bool) -> Result<reqwest::RequestBuilder> {
        let url = format!("{}/{}", s.url.trim_end_matches('/'), path);
        let r = if get {
            self.0.get(url)
        } else {
            self.0.post(url)
        };
        Ok(r.bearer_auth(credential(s)?))
    }
}
pub fn status(status: reqwest::StatusCode) -> Result<()> {
    if !status.is_success() {
        let reason = match status.as_u16() {
            401 | 403 => "проверьте ключ и права доступа",
            402 => "недостаточно средств у провайдера",
            404 => "маршрут или модель не найдены",
            429 => "превышен лимит запросов",
            500..=599 => "сервис временно недоступен",
            _ => "проверьте параметры запроса",
        };
        bail!("HTTP {} — {}", status.as_u16(), reason);
    }
    Ok(())
}
#[async_trait]
impl LlmProvider for Compatible {
    async fn models(&self, s: &Settings) -> Result<Vec<String>> {
        let r = self
            .request(s, "models", true)?
            .timeout(Duration::from_secs(6))
            .send()
            .await
            .context("Маршрутизатор недоступен")?;
        status(r.status())?;
        let v: Value = r.json().await?;
        Ok(v["data"]
            .as_array()
            .context("Некорректный список моделей")?
            .iter()
            .filter_map(|m| m["id"].as_str().map(str::to_owned))
            .collect())
    }
    async fn stream(
        &self,
        s: &Settings,
        messages: &[Value],
        tools: &[Value],
        cancel: &CancellationToken,
        on_text: &mut (dyn for<'a> FnMut(&'a str) -> Result<()> + Send),
    ) -> Result<Vec<Value>> {
        // 1024 cut long answers off silently; JARVIS_MAX_TOKENS overrides the default.
        let max_tokens = std::env::var("JARVIS_MAX_TOKENS")
            .ok()
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(2048)
            .clamp(128, 8192);
        let mut body =
            json!({"model":s.model,"messages":messages,"stream":true,"max_tokens":max_tokens});
        if !tools.is_empty() {
            body["tools"] = tools.into();
        }
        let req = self.request(s, "chat/completions", false)?.json(&body);
        let response = tokio::select! {_=cancel.cancelled()=>bail!("Отменено"),r=req.send()=>r.context("Нет связи с AI-провайдером")?};
        status(response.status())?;
        let mut stream = response.bytes_stream().eventsource();
        let mut calls: BTreeMap<usize, (String, String, String)> = BTreeMap::new();
        let mut ended = false;
        loop {
            let next =
                tokio::select! {_=cancel.cancelled()=>bail!("Отменено"),ev=stream.next()=>ev};
            let Some(ev) = next else { break };
            let data = ev.map_err(|_| anyhow!("Поток ответа прерван"))?.data;
            if data == "[DONE]" {
                ended = true;
                break;
            }
            let v: Value = serde_json::from_str(&data)?;
            ensure!(v.get("error").is_none(), "Ошибка модели в потоке");
            let choice = &v["choices"][0];
            if choice["finish_reason"].is_string() {
                ended = true;
            }
            let d = &choice["delta"];
            if let Some(t) = d["content"].as_str() {
                on_text(t)?;
            }
            if let Some(list) = d["tool_calls"].as_array() {
                for call in list {
                    let i = call["index"].as_u64().unwrap_or(0) as usize;
                    ensure!(i < 8, "Слишком много инструментов");
                    let e = calls.entry(i).or_default();
                    for (target, source) in [
                        (&mut e.0, &call["id"]),
                        (&mut e.1, &call["function"]["name"]),
                        (&mut e.2, &call["function"]["arguments"]),
                    ] {
                        if let Some(a) = source.as_str() {
                            target.push_str(a);
                            ensure!(target.len() < 4096, "Превышен размер запроса инструмента");
                        }
                    }
                }
            }
        }
        ensure!(ended, "Ответ прерван до завершения");
        Ok(calls.into_values().map(|(id,name,args)|json!({"id":id,"type":"function","function":{"name":name,"arguments":args}})).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn credentials_only_go_to_secure_origins() {
        assert!(base("http://public.example/v1").is_err());
        assert!(base("https://user:pass@example.com/v1").is_err());
        assert!(base("https://api.groq.com/openai/v1").is_ok());
        assert!(base("http://127.0.0.1:18181").is_ok());
    }
    #[test]
    fn identifies_network_failures() {
        assert!(status(reqwest::StatusCode::SERVICE_UNAVAILABLE)
            .unwrap_err()
            .to_string()
            .contains("временно"));
        assert!(status(reqwest::StatusCode::PAYMENT_REQUIRED)
            .unwrap_err()
            .to_string()
            .contains("средств"));
    }
}
