//! LLM access: live DeepInfra client, record/replay cache, and a fake for tests.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow, bail, ensure};
use regex::Regex;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::{config::settings, telemetry};
use tracing::{Instrument, Span, field::Empty, info_span};

pub struct Schema {
    pub name: &'static str,
    /// JSON schema text; part of the record/replay cache key.
    pub json: &'static str,
}

macro_rules! schema {
    ($n:ident, $f:literal) => {
        pub static $n: Schema = Schema {
            name: $f,
            json: include_str!(concat!("../schemas/", $f, ".json")),
        };
    };
}
schema!(REPLY, "Reply");
schema!(CHANGES, "Changes");
schema!(CONSOLIDATION, "Consolidation");
schema!(REFLECTION, "Reflection");
schema!(ACT, "Act");
schema!(THINK, "Think");
schema!(LESSON, "Lesson");

static TIMESTAMP: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\d{4}-\d{2}-\d{2}[ T]\d{2}:\d{2}(?::\d{2}(?:\.\d+)?)?(?:[+-]\d{2}:\d{2}|Z)?")
        .unwrap()
});

pub fn key(parts: &[&str]) -> String {
    let norm: Vec<String> = parts
        .iter()
        .map(|p| TIMESTAMP.replace_all(p, "<ts>").into_owned())
        .collect();
    hex::encode(Sha256::digest(norm.join("\x1f").as_bytes()))
}

fn prompt_tokens(system: &str, user: &str) -> u64 {
    ((system.chars().count() + user.chars().count()) / 4 + 1) as u64
}

#[derive(Debug)]
pub struct CacheMiss(pub String);

impl std::fmt::Display for CacheMiss {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for CacheMiss {}

pub fn is_miss(e: &anyhow::Error) -> bool {
    e.downcast_ref::<CacheMiss>().is_some()
}

pub struct DeepInfra {
    http: reqwest::Client,
    base: String,
    key: String,
    model: String,
    embed_model: String,
    counts: Mutex<Counts>,
}

impl Default for DeepInfra {
    fn default() -> Self {
        Self::new()
    }
}

impl DeepInfra {
    pub fn with_model(model: &str) -> Self {
        let mut d = Self::new();
        d.model = model.into();
        d
    }

    pub fn new() -> Self {
        let s = settings();
        DeepInfra {
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(s.llm_timeout_seconds))
                .build()
                .unwrap(),
            base: s.llm_base_url.clone(),
            key: s.deepinfra_api_key.clone(),
            model: s.llm_model.clone(),
            embed_model: s.embed_model.clone(),
            counts: Mutex::new(Counts::default()),
        }
    }

    async fn post(&self, path: &str, body: &Value) -> Result<Value> {
        // Only unbilled failures (transport, 5xx) retry here; durable inbox/maintenance retries own the rest.
        let mut delay = 2;
        loop {
            let sent = self
                .http
                .post(format!("{}{path}", self.base))
                .bearer_auth(&self.key)
                .json(body)
                .send()
                .await;
            let transient = match &sent {
                Ok(r) => r.status().is_server_error() || r.status().as_u16() == 429,
                Err(e) => e.is_connect() || e.is_timeout(),
            };
            if transient && delay <= 8 {
                tokio::time::sleep(Duration::from_secs(delay)).await;
                delay *= 2;
                continue;
            }
            return Ok(sent?.error_for_status()?.json().await?);
        }
    }

    pub async fn chat(
        &self,
        model: Option<&str>,
        temperature: Option<f64>,
        system: &str,
        user: &str,
        schema: Option<&Schema>,
    ) -> Result<String> {
        let mut body = json!({
            "model": model.unwrap_or(&self.model),
            "messages": [{"role": "system", "content": system}, {"role": "user", "content": user}],
            "reasoning_effort": "low",
            "max_tokens": settings().max_completion_tokens,
        });
        if let Some(t) = temperature {
            body["temperature"] = json!(t);
        }
        if let Some(s) = schema {
            body["response_format"] = json!({
                "type": "json_schema",
                "json_schema": {
                    "name": s.name,
                    "strict": true,
                    "schema": serde_json::from_str::<Value>(s.json)?,
                },
            });
        }
        let kind = schema.map_or("text", |s| s.name);
        let t0 = Instant::now();
        let mut v = self.post("/chat/completions", &body).await?;
        // Thinking cannot be disabled on this model; a runaway think rarely truncates the
        // output or leaves the content empty, so resample.
        let degenerate = |v: &Value| {
            v["choices"][0]["finish_reason"] == "length"
                || v["choices"][0]["message"]["content"]
                    .as_str()
                    .is_none_or(str::is_empty)
        };
        for _ in 0..2 {
            if !degenerate(&v) {
                break;
            }
            self.counts.lock().unwrap().count(kind, 0);
            v = self.post("/chat/completions", &body).await?;
        }
        {
            let mut c = self.counts.lock().unwrap();
            let reported = v["usage"]["prompt_tokens"].as_u64();
            let prompt = reported.unwrap_or_else(|| {
                prompt_tokens(system, user) + schema.map_or(0, |s| s.json.len() as u64 / 4)
            });
            let completion = v["usage"]["completion_tokens"].as_u64().unwrap_or(0);
            c.count(kind, prompt);
            c.completion_tokens += completion;
            c.usage_reported_calls += u64::from(reported.is_some());
            c.provider_prompt_tokens += reported.unwrap_or(0);
            c.provider_completion_tokens += completion;
            let span = Span::current();
            span.record("cache", "live");
            span.record("completion_tokens", completion);
            if let Some(p) = reported {
                span.record("provider_prompt_tokens", p);
            }
            telemetry::record_call(
                kind,
                model.unwrap_or(&self.model),
                "live",
                prompt,
                completion,
                t0.elapsed().as_secs_f64(),
            );
        }
        ensure!(
            v["choices"][0]["finish_reason"] != "length",
            "completion reached MAX_COMPLETION_TOKENS; increase the limit or request fewer changes"
        );
        v["choices"][0]["message"]["content"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(String::from)
            .ok_or_else(|| anyhow!("missing or empty completion content"))
    }

    pub async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        self.counts.lock().unwrap().embed_calls += 1;
        let body = json!({"model": self.embed_model, "input": texts});
        let v = self.post("/embeddings", &body).await?;
        let data = v["data"]
            .as_array()
            .ok_or_else(|| anyhow!("bad embeddings response"))?;
        ensure!(
            data.len() == texts.len(),
            "embedding response count mismatch"
        );
        let mut ordered: Vec<Option<Vec<f32>>> = vec![None; texts.len()];
        for item in data {
            let index = item["index"]
                .as_u64()
                .ok_or_else(|| anyhow!("missing embedding index"))?
                as usize;
            ensure!(
                index < texts.len() && ordered[index].is_none(),
                "invalid or duplicate embedding index"
            );
            ordered[index] = Some(floats(&item["embedding"]));
        }
        let vectors: Vec<Vec<f32>> = ordered.into_iter().map(Option::unwrap).collect();
        ensure!(
            vectors
                .iter()
                .all(|v| v.len() == settings().embed_dim && v.iter().all(|x| x.is_finite())),
            "invalid embedding dimensions or values"
        );
        Ok(vectors)
    }
}

fn floats(v: &Value) -> Vec<f32> {
    v.as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_f64)
        .map(|x| x as f32)
        .collect()
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Counts {
    pub llm_calls: u64,
    pub calls_by_kind: BTreeMap<String, u64>,
    pub prompt_tokens_by_kind: BTreeMap<String, u64>,
    pub embed_calls: u64,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub usage_reported_calls: u64,
    pub provider_prompt_tokens: u64,
    pub provider_completion_tokens: u64,
    pub cache_misses: u64,
}

impl Counts {
    fn count(&mut self, kind: &str, prompt_tokens: u64) {
        self.llm_calls += 1;
        self.prompt_tokens += prompt_tokens;
        *self.calls_by_kind.entry(kind.into()).or_default() += 1;
        *self.prompt_tokens_by_kind.entry(kind.into()).or_default() += prompt_tokens;
    }
}

struct RecInner {
    cache: BTreeMap<String, Value>,
    dirty: bool,
    counts: Counts,
}

pub struct Recording {
    inner: Option<DeepInfra>,
    path: PathBuf,
    strict: bool,
    legacy_control: bool,
    model: String,
    temperature: Option<f64>,
    rec: Mutex<RecInner>,
}

impl Recording {
    pub fn new(
        inner: Option<DeepInfra>,
        path: PathBuf,
        strict: bool,
        legacy_control: bool,
    ) -> Result<Self> {
        let cache = if path.exists() {
            serde_json::from_str(&std::fs::read_to_string(&path)?)?
        } else {
            BTreeMap::new()
        };
        let rec = Mutex::new(RecInner {
            cache,
            dirty: false,
            counts: Counts::default(),
        });
        let model = inner
            .as_ref()
            .map_or_else(|| settings().llm_model.clone(), |d| d.model.clone());
        Ok(Recording {
            inner,
            path,
            strict,
            legacy_control,
            model,
            temperature: None,
            rec,
        })
    }

    /// Keys include the model, so a judge cache never replays another model's verdicts.
    pub fn model(mut self, model: &str) -> Self {
        self.model = model.into();
        self
    }

    pub fn temperature(mut self, temperature: Option<f64>) -> Self {
        self.temperature = temperature;
        self
    }

    pub fn counts(&self) -> Counts {
        let mut counts = self.rec.lock().unwrap().counts.clone();
        if let Some(inner) = &self.inner {
            let measured = inner.counts.lock().unwrap();
            counts.usage_reported_calls = measured.usage_reported_calls;
            counts.provider_prompt_tokens = measured.provider_prompt_tokens;
            counts.provider_completion_tokens = measured.provider_completion_tokens;
        }
        counts
    }

    fn cache_key(&self, model: Option<&str>, parts: &[&str]) -> String {
        if self.legacy_control {
            return key(parts);
        }
        let cfg = settings();
        let model = if parts[0] == "embed" {
            cfg.embed_model.as_str()
        } else {
            model.unwrap_or(&self.model)
        };
        let temperature = self.temperature.map(|t| format!("temperature={t}"));
        let mut parts = parts.to_vec();
        parts.extend(temperature.as_deref());
        let mut hash = Sha256::new();
        hash.update(
            serde_json::to_vec(&(
                model,
                cfg.embed_dim,
                cfg.max_completion_tokens,
                "low",
                parts,
            ))
            .unwrap(),
        );
        hex::encode(hash.finalize())
    }

    /// A replayed call reports whether it came from the cache.
    fn replayed(&self, kind: &str, model: Option<&str>, prompt: u64, completion: u64, t0: Instant) {
        let span = Span::current();
        span.record("cache", "hit");
        span.record("completion_tokens", completion);
        telemetry::record_call(
            kind,
            model.unwrap_or(&self.model),
            "hit",
            prompt,
            completion,
            t0.elapsed().as_secs_f64(),
        );
    }

    async fn get<F>(
        &self,
        key: String,
        what: impl FnOnce() -> String,
        fetch: F,
    ) -> Result<(Value, bool)>
    where
        F: Future<Output = Result<Value>>,
    {
        if let Some(v) = self.rec.lock().unwrap().cache.get(&key) {
            return Ok((v.clone(), true));
        }
        if self.strict || self.inner.is_none() {
            let n = self.rec.lock().unwrap().counts.llm_calls;
            return Err(CacheMiss(format!("call #{n} {}", what())).into());
        }
        self.rec.lock().unwrap().counts.cache_misses += 1;
        let v = fetch.await?;
        {
            let mut r = self.rec.lock().unwrap();
            r.cache.insert(key, v.clone());
            r.dirty = true;
        }
        self.save()?;
        Ok((v, false))
    }

    pub async fn complete_json<T: DeserializeOwned + Serialize>(
        &self,
        model: Option<&str>,
        system: &str,
        user: &str,
        schema: &Schema,
    ) -> Result<T> {
        let estimate = prompt_tokens(system, user) + schema.json.len() as u64 / 4;
        self.rec.lock().unwrap().counts.count(schema.name, estimate);
        let k = self.cache_key(model, &["json", system, user, schema.json]);
        let t0 = Instant::now();
        let fetch = async {
            let content = self
                .inner
                .as_ref()
                .unwrap()
                .chat(model, self.temperature, system, user, Some(schema))
                .await?;
            let t: T = parse_json(&content)?;
            Ok(serde_json::to_value(t)?)
        };
        let (v, hit) = self
            .get(k, || format!("json/{}:\n{user}", schema.name), fetch)
            .await?;
        let completion = v.to_string().len() as u64 / 4 + 1;
        self.rec.lock().unwrap().counts.completion_tokens += completion;
        if hit {
            self.replayed(schema.name, model, estimate, completion, t0);
        }
        Ok(serde_json::from_value(v)?)
    }

    pub async fn complete_text(&self, system: &str, user: &str) -> Result<String> {
        let estimate = prompt_tokens(system, user);
        self.rec.lock().unwrap().counts.count("text", estimate);
        let k = self.cache_key(None, &["text", system, user]);
        let t0 = Instant::now();
        let fetch = async {
            Ok(Value::String(
                self.inner
                    .as_ref()
                    .unwrap()
                    .chat(None, self.temperature, system, user, None)
                    .await?,
            ))
        };
        let (v, hit) = self
            .get(k, || format!("text:\n{system}\n---\n{user}"), fetch)
            .await?;
        let completion = v.as_str().unwrap_or("").len() as u64 / 4 + 1;
        self.rec.lock().unwrap().counts.completion_tokens += completion;
        if hit {
            self.replayed("text", None, estimate, completion, t0);
        }
        Ok(v.as_str().unwrap_or("").to_string())
    }

    pub async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        self.rec.lock().unwrap().counts.embed_calls += 1;
        let keys: Vec<String> = texts
            .iter()
            .map(|t| self.cache_key(None, &["embed", t]))
            .collect();
        let missing: Vec<String> = {
            let r = self.rec.lock().unwrap();
            texts
                .iter()
                .zip(&keys)
                .filter(|(_, k)| !r.cache.contains_key(*k))
                .map(|(t, _)| t.clone())
                .collect()
        };
        Span::current().record("cache", if missing.is_empty() { "hit" } else { "live" });
        if !missing.is_empty() {
            if self.strict || self.inner.is_none() {
                let n = self.rec.lock().unwrap().counts.embed_calls;
                let head: String = missing[0].chars().take(200).collect();
                return Err(CacheMiss(format!("embed #{n}: {head:?}")).into());
            }
            self.rec.lock().unwrap().counts.cache_misses += 1;
            let vecs = self.inner.as_ref().unwrap().embed(&missing).await?;
            let mut r = self.rec.lock().unwrap();
            for (t, v) in missing.iter().zip(vecs) {
                let rounded: Vec<f64> = v.iter().map(|x| (*x as f64 * 1e6).round() / 1e6).collect();
                r.cache
                    .insert(self.cache_key(None, &["embed", t]), json!(rounded));
            }
            r.dirty = true;
            drop(r);
            self.save()?;
        }
        let r = self.rec.lock().unwrap();
        Ok(keys.iter().map(|k| floats(&r.cache[k])).collect())
    }

    pub fn save(&self) -> Result<()> {
        let mut r = self.rec.lock().unwrap();
        if r.dirty {
            if let Some(p) = self.path.parent() {
                std::fs::create_dir_all(p)?;
            }
            let tmp = self.path.with_extension("json.tmp");
            std::fs::write(&tmp, serde_json::to_string(&r.cache)?)?;
            std::fs::rename(&tmp, &self.path)?;
            r.dirty = false;
        }
        Ok(())
    }
}

fn json_body(content: &str) -> &str {
    let body = content
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();
    if body.is_empty() { "{}" } else { body }
}

/// Providers occasionally emit raw control characters inside JSON strings; retry with them blanked.
fn parse_json<T: DeserializeOwned>(content: &str) -> Result<T> {
    let body = json_body(content);
    match serde_json::from_str(body) {
        Ok(v) => Ok(v),
        Err(e) => {
            let cleaned: String = body
                .chars()
                .map(|c| if c.is_control() { ' ' } else { c })
                .collect();
            serde_json::from_str(&cleaned).map_err(|_| e.into())
        }
    }
}

pub type Responder = fn(&str, &str, &str) -> Value;

/// Queue-driven stand-in for tests: unqueued JSON calls return the schema's default value.
#[derive(Default)]
pub struct Fake {
    pub json: Mutex<HashMap<String, VecDeque<Value>>>,
    pub text: Mutex<VecDeque<String>>,
    pub errors: Mutex<VecDeque<String>>,
    pub calls: Mutex<Vec<(String, String)>>,
    pub delay_ms: std::sync::atomic::AtomicU64,
    pub respond: Mutex<Option<Responder>>,
}

impl Fake {
    pub fn queue<T: Serialize>(&self, schema: &str, v: T) {
        let v = serde_json::to_value(v).unwrap();
        self.json
            .lock()
            .unwrap()
            .entry(schema.into())
            .or_default()
            .push_back(v);
    }

    pub fn queue_text(&self, reply: &str) {
        self.text.lock().unwrap().push_back(reply.into());
    }

    pub fn fail(&self, msg: &str) {
        self.errors.lock().unwrap().push_back(msg.into());
    }

    pub fn calls_for(&self, schema: &str) -> Vec<String> {
        let calls = self.calls.lock().unwrap();
        calls
            .iter()
            .filter(|(s, _)| s == schema)
            .map(|(_, u)| u.clone())
            .collect()
    }
}

/// Deterministic bag-of-words embedding for tests.
pub fn hash_embedding(text: &str, dim: usize) -> Vec<f32> {
    let mut v = vec![0f32; dim];
    for tok in text.to_lowercase().split_whitespace() {
        let h = Sha256::digest(tok.as_bytes());
        let i = u32::from_be_bytes([h[0], h[1], h[2], h[3]]) as usize % dim;
        v[i] += 1.0;
    }
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1.0);
    v.iter().map(|x| x / norm).collect()
}

#[allow(clippy::large_enum_variant)]
pub enum Llm {
    Live(DeepInfra),
    Recording(Recording),
    Fake(Fake),
}

impl Llm {
    pub async fn complete_json<T: DeserializeOwned + Serialize + Default>(
        &self,
        system: &str,
        user: &str,
        schema: &Schema,
    ) -> Result<T> {
        self.complete_json_as(None, system, user, schema).await
    }

    /// `model` overrides the configured model for this call; the cache key follows it.
    pub async fn complete_json_as<T: DeserializeOwned + Serialize + Default>(
        &self,
        model: Option<&str>,
        system: &str,
        user: &str,
        schema: &Schema,
    ) -> Result<T> {
        ensure!(
            prompt_tokens(system, user) + schema.json.len() as u64 / 4
                <= settings().max_prompt_tokens,
            "prompt exceeds MAX_PROMPT_TOKENS"
        );
        let span = info_span!(
            "llm.call",
            kind = schema.name,
            model = model.unwrap_or(&settings().llm_model),
            prompt_tokens = prompt_tokens(system, user) + schema.json.len() as u64 / 4,
            cache = Empty,
            completion_tokens = Empty,
            provider_prompt_tokens = Empty
        );
        async move {
            match self {
                Llm::Live(l) => {
                    let content = l.chat(model, None, system, user, Some(schema)).await?;
                    parse_json(&content)
                }
                Llm::Recording(r) => r.complete_json(model, system, user, schema).await,
                Llm::Fake(f) => {
                    Span::current().record("cache", "fake");
                    f.calls
                        .lock()
                        .unwrap()
                        .push((schema.name.into(), format!("{system}\n{user}")));
                    let delay = f.delay_ms.load(std::sync::atomic::Ordering::Relaxed);
                    if delay > 0 {
                        tokio::time::sleep(Duration::from_millis(delay)).await;
                    }
                    if let Some(e) = f.errors.lock().unwrap().pop_front() {
                        bail!("{e}");
                    }
                    let queued = f
                        .json
                        .lock()
                        .unwrap()
                        .get_mut(schema.name)
                        .and_then(VecDeque::pop_front);
                    Ok(match queued {
                        Some(v) => serde_json::from_value(v)?,
                        None => match *f.respond.lock().unwrap() {
                            Some(respond) => {
                                serde_json::from_value(respond(system, user, schema.name))?
                            }
                            None => T::default(),
                        },
                    })
                }
            }
        }
        .instrument(span)
        .await
    }

    pub async fn complete_text(&self, system: &str, user: &str) -> Result<String> {
        ensure!(
            prompt_tokens(system, user) <= settings().max_prompt_tokens,
            "prompt exceeds MAX_PROMPT_TOKENS"
        );
        let span = info_span!("llm.call", kind = "text", model = %settings().llm_model,
            prompt_tokens = prompt_tokens(system, user),
            cache = Empty, completion_tokens = Empty, provider_prompt_tokens = Empty);
        async move {
            match self {
                Llm::Live(l) => l.chat(None, None, system, user, None).await,
                Llm::Recording(r) => r.complete_text(system, user).await,
                Llm::Fake(f) => {
                    Span::current().record("cache", "fake");
                    f.calls.lock().unwrap().push(("text".into(), user.into()));
                    if let Some(e) = f.errors.lock().unwrap().pop_front() {
                        bail!("{e}");
                    }
                    let queued = f.text.lock().unwrap().pop_front();
                    Ok(queued.unwrap_or_else(|| match *f.respond.lock().unwrap() {
                        Some(respond) => respond(system, user, "text")
                            .as_str()
                            .unwrap_or("ok.")
                            .to_string(),
                        None => "ok.".into(),
                    }))
                }
            }
        }
        .instrument(span)
        .await
    }

    pub async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        let span = info_span!("embed", texts = texts.len(), cache = Empty);
        let vectors = async move {
            match self {
                Llm::Live(l) => {
                    Span::current().record("cache", "live");
                    l.embed(texts).await
                }
                Llm::Recording(r) => r.embed(texts).await,
                Llm::Fake(_) => {
                    Span::current().record("cache", "fake");
                    Ok(texts
                        .iter()
                        .map(|t| hash_embedding(t, settings().embed_dim))
                        .collect())
                }
            }
        }
        .instrument(span)
        .await?;
        ensure!(
            vectors.len() == texts.len()
                && vectors
                    .iter()
                    .all(|v| v.len() == settings().embed_dim && v.iter().all(|x| x.is_finite())),
            "invalid embedding response count, dimensions or values"
        );
        Ok(vectors)
    }

    pub fn counts(&self) -> Counts {
        match self {
            Llm::Recording(r) => r.counts(),
            Llm::Live(l) => l.counts.lock().unwrap().clone(),
            Llm::Fake(f) => {
                let calls = f.calls.lock().unwrap();
                Counts {
                    llm_calls: calls.len() as u64,
                    prompt_tokens: calls.iter().map(|(_, p)| p.len() as u64 / 4 + 1).sum(),
                    ..Counts::default()
                }
            }
        }
    }

    pub fn save(&self) -> Result<()> {
        match self {
            Llm::Recording(r) => r.save(),
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modern_cache_preserves_dates_and_namespaces_models() {
        let path =
            std::env::temp_dir().join(format!("morpho-key-test-{}.json", std::process::id()));
        let modern = Recording::new(None, path.clone(), true, false).unwrap();
        let legacy = Recording::new(None, path, true, true).unwrap();
        let first = ["embed", "deadline 2026-09-11T12:00:00Z"];
        let second = ["embed", "deadline 2026-09-12T12:00:00Z"];
        assert_ne!(
            modern.cache_key(None, &first),
            modern.cache_key(None, &second)
        );
        assert_ne!(modern.cache_key(None, &first), key(&first));
        assert_eq!(legacy.cache_key(None, &first), key(&first));
    }

    #[test]
    fn key_vectors() {
        assert_eq!(
            key(&["text", "a 2026-09-11 12:34", "b"]),
            "afa152c9439204b9977768126e9a730958bc62a506a378275c43a5c4a4ce110a"
        );
        let dana1 = "Hi, I'm Dana. I just moved from Lisbon to Berlin for a new job at a startup called Nimbus.";
        assert_eq!(
            key(&["embed", dana1]),
            "7426faf1745ec2ad82b2e6e9fde280f5747f76b591bdd73bb0c976e77caeb774"
        );
        let s = "x 2026-09-11 12:34:56.789012+00:00 y 2026-09-11T12:34Z";
        assert_eq!(TIMESTAMP.replace_all(s, "<ts>"), "x <ts> y <ts>");
    }
}

#[cfg(test)]
mod judge_tests {
    use super::*;

    #[test]
    fn judge_cache_keys_depend_on_the_model() {
        let dir = std::env::temp_dir().join(format!("morpho-llm-{}", std::process::id()));
        let a = Recording::new(None, dir.join("a.json"), true, false).unwrap();
        let b = Recording::new(None, dir.join("b.json"), true, false)
            .unwrap()
            .model("other/model");
        assert_ne!(
            a.cache_key(None, &["json", "s", "u", "{}"]),
            b.cache_key(None, &["json", "s", "u", "{}"])
        );
        assert_eq!(
            a.cache_key(None, &["embed", "x"]),
            b.cache_key(None, &["embed", "x"])
        );
    }

    #[test]
    fn judge_cache_keys_change_only_when_a_temperature_is_set() {
        let dir = std::env::temp_dir().join(format!("morpho-llm-t-{}", std::process::id()));
        let a = Recording::new(None, dir.join("a.json"), true, false).unwrap();
        let b = Recording::new(None, dir.join("b.json"), true, false)
            .unwrap()
            .temperature(Some(0.0));
        let c = Recording::new(None, dir.join("c.json"), true, false)
            .unwrap()
            .temperature(None);
        let parts = ["json", "s", "u", "{}"];
        assert_ne!(a.cache_key(None, &parts), b.cache_key(None, &parts));
        assert_eq!(a.cache_key(None, &parts), c.cache_key(None, &parts));
    }
}
