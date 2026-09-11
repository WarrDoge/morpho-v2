//! Renders values exactly as the Python implementation did, so recorded prompts replay.

use chrono::{DateTime, SecondsFormat, Timelike, Utc};
use serde_json::{Map, Value};

pub type Row = Map<String, Value>;

pub fn tokens(s: &str) -> usize {
    s.chars().count() / 4 + 1
}

/// Python `repr(float)`.
pub fn repr(x: f64) -> String {
    let s = format!("{x:?}");
    match s.find('e') {
        Some(i) => {
            let (m, e) = s.split_at(i);
            let (sign, digits) = match e[1..].strip_prefix('-') {
                Some(d) => ("-", d),
                None => ("+", &e[1..]),
            };
            format!("{m}e{sign}{digits:0>2}")
        }
        None => s,
    }
}

/// Python `format(x, ".{prec}f")`.
pub fn fixed(x: f64, prec: usize) -> String {
    format!("{x:.prec$}")
}

/// Python `round(x, 4)`.
pub fn round4(x: f64) -> f64 {
    fixed(x, 4).parse().unwrap()
}

/// Python `str(value)` for a JSON-decoded value.
pub fn py_str(v: &Value) -> String {
    match v {
        Value::Null => "None".into(),
        Value::Bool(b) => if *b { "True" } else { "False" }.into(),
        Value::Number(n) => num(n),
        Value::String(s) => s.clone(),
        Value::Array(a) => {
            let items: Vec<String> = a.iter().map(py_repr).collect();
            format!("[{}]", items.join(", "))
        }
        Value::Object(_) => py_dumps(v),
    }
}

fn py_repr(v: &Value) -> String {
    match v {
        Value::String(s) => format!("'{}'", s.replace('\\', "\\\\").replace('\'', "\\'")),
        _ => py_str(v),
    }
}

fn num(n: &serde_json::Number) -> String {
    if n.is_f64() {
        repr(n.as_f64().unwrap())
    } else {
        n.to_string()
    }
}

/// Python truthiness of a JSON value.
pub fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64() != Some(0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// jsonb canonical key order: shorter keys first, then bytewise.
pub fn jsonb_keys(m: &Row) -> Vec<&String> {
    let mut keys: Vec<&String> = m.keys().collect();
    keys.sort_by(|a, b| {
        a.len()
            .cmp(&b.len())
            .then_with(|| a.as_bytes().cmp(b.as_bytes()))
    });
    keys
}

/// Python `json.dumps(value)` with default kwargs, keys in jsonb order.
pub fn py_dumps(v: &Value) -> String {
    let mut out = String::new();
    dump(v, &mut out);
    out
}

fn dump(v: &Value, out: &mut String) {
    match v {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => out.push_str(&num(n)),
        Value::String(s) => dump_str(s, out),
        Value::Array(a) => {
            out.push('[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                dump(x, out);
            }
            out.push(']');
        }
        Value::Object(m) => {
            out.push('{');
            for (i, k) in jsonb_keys(m).into_iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                dump_str(k, out);
                out.push_str(": ");
                dump(&m[k], out);
            }
            out.push('}');
        }
    }
}

fn dump_str(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            ' '..='~' => out.push(c),
            _ => {
                let mut buf = [0u16; 2];
                for u in c.encode_utf16(&mut buf) {
                    out.push_str(&format!("\\u{u:04x}"));
                }
            }
        }
    }
    out.push('"');
}

static EVAL_CLOCK: std::sync::OnceLock<DateTime<Utc>> = std::sync::OnceLock::new();

/// Fix the clock in the standalone scenario runner so recorded state and deadlines replay exactly.
pub fn set_eval_clock(time: DateTime<Utc>) -> anyhow::Result<()> {
    EVAL_CLOCK
        .set(time)
        .map_err(|_| anyhow::anyhow!("evaluation clock already set"))
}

pub fn now() -> DateTime<Utc> {
    let t = EVAL_CLOCK.get().copied().unwrap_or_else(Utc::now);
    t.with_nanosecond(t.timestamp_subsec_micros() * 1000)
        .unwrap_or(t)
}

/// Python `datetime.isoformat()` for an aware UTC value.
pub fn iso(t: &DateTime<Utc>) -> String {
    t.to_rfc3339_opts(SecondsFormat::Micros, false)
}

pub fn parse_dt(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|t| t.with_timezone(&Utc))
}

pub fn dt_of(v: Option<&Value>) -> Option<DateTime<Utc>> {
    v.and_then(Value::as_str).and_then(parse_dt)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn float_repr() {
        let cases = [
            (1.0, "1.0"),
            (0.8, "0.8"),
            (0.1 + 0.2, "0.30000000000000004"),
            (1e-5, "1e-05"),
            (0.0001, "0.0001"),
            (5e-324, "5e-324"),
            (1e16, "1e+16"),
            (1e15, "1000000000000000.0"),
            (0.5, "0.5"),
        ];
        for (x, s) in cases {
            assert_eq!(repr(x), s);
        }
    }

    #[test]
    fn fixed_ties() {
        let cases = [
            (0.125, "0.12"),
            (0.375, "0.38"),
            (0.625, "0.62"),
            (0.875, "0.88"),
            (0.005, "0.01"),
            (0.015, "0.01"),
            (0.995, "0.99"),
        ];
        for (x, s) in cases {
            assert_eq!(fixed(x, 2), s);
        }
        assert_eq!(round4(2557.43333333), 2557.4333);
    }

    #[test]
    fn dumps() {
        let e9 = concat!("\\u00", "e9");
        let v = json!({"b": 1, "aa": [1.0, "é", null, true], "c": {"zz": 0.5, "y": ""}});
        let want =
            format!(r#"{{"b": 1, "c": {{"y": "", "zz": 0.5}}, "aa": [1.0, "{e9}", null, true]}}"#);
        assert_eq!(py_dumps(&v), want);
        let (ea, dash) = (concat!("\\u00", "ea"), concat!("\\u20", "14"));
        let (emoji, del) = (
            concat!("\\ud8", "3d", "\\ude", "00"),
            concat!("\\u00", "7f"),
        );
        let want = format!(r#""In{ea}s {dash} {emoji} {del}\n""#);
        assert_eq!(py_dumps(&json!("Inês — 😀 \u{7f}\n")), want);
        assert_eq!(py_str(&json!(null)), "None");
        assert_eq!(py_str(&json!(true)), "True");
        assert_eq!(py_str(&json!(3)), "3");
        assert_eq!(py_str(&json!(["a", 1])), "['a', 1]");
    }

    #[test]
    fn timestamps() {
        let t = parse_dt("2026-09-11T12:34:56.789012+00:00").unwrap();
        assert_eq!(iso(&t), "2026-09-11T12:34:56.789012+00:00");
        assert_eq!(t.format("%Y-%m-%d %H:%M").to_string(), "2026-09-11 12:34");
        assert_eq!(tokens("Inês"), 2);
        assert_eq!(tokens(""), 1);
    }
}
