//! score = relevance × importance × recency × confidence × reinforcement (SPEC §15).
//! Reinforcement counts restatements and scales by outcome credit, 1 until credit exists.

use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::pyfmt::{Row, dt_of};

pub fn recency_factor(row: &Row, now: &DateTime<Utc>, half_life_days: f64) -> f64 {
    let reference = dt_of(row.get("last_reinforced_at"))
        .or_else(|| dt_of(row.get("created_at")))
        .unwrap_or(*now);
    let age_days = (*now - reference).num_seconds().div_euclid(86400).max(0);
    (-(age_days as f64) / half_life_days).exp()
}

fn f(row: &Row, k: &str, default: f64) -> f64 {
    row.get(k).and_then(Value::as_f64).unwrap_or(default)
}

pub fn score(row: &Row, relevance: f64, now: &DateTime<Utc>, half_life_days: f64) -> f64 {
    let importance = f(row, "importance", 1.0);
    let confidence = f(row, "confidence", 1.0);
    let (successes, failures) = (f(row, "successes", 0.0), f(row, "failures", 0.0));
    let credit = 2.0 * (1.0 + successes) / (2.0 + successes + failures);
    let reinforcement = (1.0 + f(row, "access_count", 0.0).ln_1p()) * credit;
    relevance * importance * recency_factor(row, now, half_life_days) * confidence * reinforcement
}

pub fn decayed_importance(row: &Row, now: &DateTime<Utc>, half_life_days: f64) -> f64 {
    f(row, "importance", 0.0) * recency_factor(row, now, half_life_days)
}
