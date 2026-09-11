//! LLM access: live DeepInfra client, record/replay cache, and a fake for tests.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use regex::Regex;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::config::settings;

pub struct Schema {
    pub name: &'static str,
    /// Exact pydantic `json.dumps(model_json_schema(), sort_keys=True)`; part of the cache key.
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
schema!(INTERPRETATION, "Interpretation");
schema!(MEMORY_DECISION, "MemoryDecision");
schema!(BELIEF_DECISION, "BeliefDecision");
schema!(GOAL_REVIEW, "GoalReview");
schema!(SELF_PATCH, "SelfPatch");
schema!(CONSOLIDATION, "Consolidation");
schema!(REFLECTION, "Reflection");

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
}

impl Default for DeepInfra {
    fn default() -> Self {
        Self::new()
    }
}

impl DeepInfra {
    pub fn new() -> Self {
        let s = settings();
        DeepInfra {
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(600))
                .build()
                .unwrap(),
            base: s.llm_base_url.clone(),
            key: s.deepinfra_api_key.clone(),
            model: s.llm_model.clone(),
            embed_model: s.embed_model.clone(),
        }
    }

    async fn post(&self, path: &str, body: &Value) -> Result<Value> {
        let mut last = anyhow!("no attempts");
        for attempt in 0..3u32 {
            let req = self
                .http
                .post(format!("{}{path}", self.base))
                .bearer_auth(&self.key);
            match req.json(body).send().await {
                Ok(r) if r.status().is_success() => return Ok(r.json().await?),
                Ok(r)
                    if r.status().is_server_error()
                        || [408, 409, 429].contains(&r.status().as_u16()) =>
                {
                    last = anyhow!("{} {}", r.status(), r.text().await.unwrap_or_default());
                }
                Ok(r) => bail!("{} {}", r.status(), r.text().await.unwrap_or_default()),
                Err(e) => last = e.into(),
            }
            tokio::time::sleep(Duration::from_secs(1 << attempt)).await;
        }
        Err(last)
    }

    pub async fn chat(&self, system: &str, user: &str, schema: Option<&Schema>) -> Result<String> {
        let mut body = json!({
            "model": self.model,
            "messages": [{"role": "system", "content": system}, {"role": "user", "content": user}],
            "reasoning_effort": "low",
        });
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
        let v = self.post("/chat/completions", &body).await?;
        Ok(v["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or("")
            .to_string())
    }

    pub async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        let body = json!({"model": self.embed_model, "input": texts});
        let v = self.post("/embeddings", &body).await?;
        let data = v["data"]
            .as_array()
            .ok_or_else(|| anyhow!("bad embeddings response"))?;
        Ok(data.iter().map(|d| floats(&d["embedding"])).collect())
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

#[derive(Clone, Copy, Debug, Default, Serialize)]
pub struct Counts {
    pub llm_calls: u64,
    pub embed_calls: u64,
    pub prompt_tokens: u64,
    pub cache_misses: u64,
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
    rec: Mutex<RecInner>,
}

impl Recording {
    pub fn new(inner: Option<DeepInfra>, path: PathBuf, strict: bool) -> Result<Self> {
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
        Ok(Recording {
            inner,
            path,
            strict,
            rec,
        })
    }

    pub fn counts(&self) -> Counts {
        self.rec.lock().unwrap().counts
    }

    async fn get<F>(&self, key: String, what: impl FnOnce() -> String, fetch: F) -> Result<Value>
    where
        F: Future<Output = Result<Value>>,
    {
        if let Some(v) = self.rec.lock().unwrap().cache.get(&key) {
            return Ok(v.clone());
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
        Ok(v)
    }

    pub async fn complete_json<T: DeserializeOwned + Serialize>(
        &self,
        system: &str,
        user: &str,
        schema: &Schema,
    ) -> Result<T> {
        {
            let mut r = self.rec.lock().unwrap();
            r.counts.llm_calls += 1;
            r.counts.prompt_tokens += prompt_tokens(system, user);
        }
        let k = key(&["json", system, user, schema.json]);
        let fetch = async {
            let content = self
                .inner
                .as_ref()
                .unwrap()
                .chat(system, user, Some(schema))
                .await?;
            let t: T = serde_json::from_str(if content.is_empty() { "{}" } else { &content })?;
            Ok(serde_json::to_value(t)?)
        };
        let v = self
            .get(k, || format!("json/{}:\n{user}", schema.name), fetch)
            .await?;
        Ok(serde_json::from_value(v)?)
    }

    pub async fn complete_text(&self, system: &str, user: &str) -> Result<String> {
        {
            let mut r = self.rec.lock().unwrap();
            r.counts.llm_calls += 1;
            r.counts.prompt_tokens += prompt_tokens(system, user);
        }
        let k = key(&["text", system, user]);
        let fetch = async {
            Ok(Value::String(
                self.inner
                    .as_ref()
                    .unwrap()
                    .chat(system, user, None)
                    .await?,
            ))
        };
        let v = self
            .get(k, || format!("text:\n{system}\n---\n{user}"), fetch)
            .await?;
        Ok(v.as_str().unwrap_or("").to_string())
    }

    pub async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        self.rec.lock().unwrap().counts.embed_calls += 1;
        let keys: Vec<String> = texts.iter().map(|t| key(&["embed", t])).collect();
        let missing: Vec<String> = {
            let r = self.rec.lock().unwrap();
            texts
                .iter()
                .zip(&keys)
                .filter(|(_, k)| !r.cache.contains_key(*k))
                .map(|(t, _)| t.clone())
                .collect()
        };
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
                r.cache.insert(key(&["embed", t]), json!(rounded));
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
            std::fs::write(&self.path, serde_json::to_string(&r.cache)?)?;
            r.dirty = false;
        }
        Ok(())
    }
}

/// Queue-driven stand-in for tests: unqueued JSON calls return the schema's default value.
#[derive(Default)]
pub struct Fake {
    pub json: Mutex<HashMap<String, VecDeque<Value>>>,
    pub text: Mutex<VecDeque<String>>,
    pub errors: Mutex<VecDeque<String>>,
    pub calls: Mutex<Vec<(String, String)>>,
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
        match self {
            Llm::Live(l) => {
                let content = l.chat(system, user, Some(schema)).await?;
                Ok(serde_json::from_str(if content.is_empty() {
                    "{}"
                } else {
                    &content
                })?)
            }
            Llm::Recording(r) => r.complete_json(system, user, schema).await,
            Llm::Fake(f) => {
                f.calls
                    .lock()
                    .unwrap()
                    .push((schema.name.into(), user.into()));
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
                    None => T::default(),
                })
            }
        }
    }

    pub async fn complete_text(&self, system: &str, user: &str) -> Result<String> {
        match self {
            Llm::Live(l) => l.chat(system, user, None).await,
            Llm::Recording(r) => r.complete_text(system, user).await,
            Llm::Fake(f) => {
                f.calls.lock().unwrap().push(("text".into(), user.into()));
                if let Some(e) = f.errors.lock().unwrap().pop_front() {
                    bail!("{e}");
                }
                Ok(f.text
                    .lock()
                    .unwrap()
                    .pop_front()
                    .unwrap_or_else(|| "ok".into()))
            }
        }
    }

    pub async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        match self {
            Llm::Live(l) => l.embed(texts).await,
            Llm::Recording(r) => r.embed(texts).await,
            Llm::Fake(_) => Ok(texts
                .iter()
                .map(|t| hash_embedding(t, settings().embed_dim))
                .collect()),
        }
    }

    pub fn counts(&self) -> Counts {
        match self {
            Llm::Recording(r) => r.counts(),
            _ => Counts::default(),
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
