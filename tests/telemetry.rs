mod common;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use common::services;
use morpho::interact::interact_as;
use tracing::field::{Field, Visit};
use tracing::{Subscriber, span};
use tracing_subscriber::layer::{Context, Layer, SubscriberExt};
use tracing_subscriber::registry::LookupSpan;

type Fields = BTreeMap<String, String>;

/// Closed spans with their recorded fields, in closing order.
#[derive(Clone, Default)]
struct Closed(Arc<Mutex<Vec<(String, Fields)>>>);

struct Collect<'a>(&'a mut Fields);

impl Visit for Collect<'_> {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.0.insert(field.name().into(), format!("{value:?}"));
    }
    fn record_str(&mut self, field: &Field, value: &str) {
        self.0.insert(field.name().into(), value.into());
    }
    fn record_u64(&mut self, field: &Field, value: u64) {
        self.0.insert(field.name().into(), value.to_string());
    }
}

impl<S: Subscriber + for<'a> LookupSpan<'a>> Layer<S> for Closed {
    fn on_new_span(&self, attrs: &span::Attributes<'_>, id: &span::Id, ctx: Context<'_, S>) {
        let mut fields = Fields::new();
        attrs.record(&mut Collect(&mut fields));
        ctx.span(id).unwrap().extensions_mut().insert(fields);
    }
    fn on_record(&self, id: &span::Id, values: &span::Record<'_>, ctx: Context<'_, S>) {
        let span = ctx.span(id).unwrap();
        let mut ext = span.extensions_mut();
        values.record(&mut Collect(ext.get_mut::<Fields>().unwrap()));
    }
    fn on_close(&self, id: span::Id, ctx: Context<'_, S>) {
        let span = ctx.span(&id).unwrap();
        let fields = span.extensions_mut().remove::<Fields>().unwrap_or_default();
        self.0
            .lock()
            .unwrap()
            .push((span.name().to_string(), fields));
    }
}

#[tokio::test]
async fn a_turn_traces_its_calls_and_changes() {
    let closed = Closed::default();
    let _guard =
        tracing::subscriber::set_default(tracing_subscriber::registry().with(closed.clone()));
    let svc = services("telemetry");
    interact_as(&svc, "hi", Some("s1"), "alice", None)
        .await
        .unwrap();
    let spans = closed.0.lock().unwrap();
    let named = |name: &str| -> Vec<&Fields> {
        spans
            .iter()
            .filter(|(n, _)| n == name)
            .map(|(_, f)| f)
            .collect()
    };
    let calls = named("llm.call");
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0]["kind"], "text");
    assert_eq!(calls[0]["cache"], "fake");
    assert_eq!(calls[1]["kind"], "Changes");
    assert!(calls[0]["prompt_tokens"].parse::<u64>().unwrap() > 0);
    assert_eq!(named("embed").len(), 2);
    let compose = named("compose")[0];
    assert_eq!(compose["tokens"], "0"); // nothing stored yet
    assert_eq!(compose["identity_traits"], "0");
    let names: Vec<&str> = spans.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(named("commit").len(), 1, "{names:?}");
    assert_eq!(named("commit")[0]["proposals"], "0");
    let turn = named("turn")[0];
    assert_eq!(turn["speaker"], "alice");
    assert_eq!(turn["session"], "s1");
    assert_eq!(turn["resamples"], "0");
    assert_eq!(turn["changes.proposed"], "0");
    assert_eq!(turn["reply_chars"], "3");
    assert!(spans.iter().position(|(n, _)| n == "turn").unwrap() > spans.len() - 2);
}
