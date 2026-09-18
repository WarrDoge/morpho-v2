//! Proposal and payload schemas with the same validation rules as the Python models.

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};

use crate::pyfmt::{Row, parse_dt};
use crate::store::obj;

pub const LIVE_MEMORY: &[&str] = &["candidate", "active", "reinforced", "consolidated"];
pub const LIVE_BELIEF: &[&str] = &["hypothesis", "active", "uncertain", "contradicted"];
pub const LIVE_GOAL: &[&str] = &["proposed", "active", "blocked"];
pub const LIVE_TRAIT: &[&str] = &["active", "uncertain"];
pub const TERMINAL_MEMORY: &[&str] = &["deprecated", "archived"];
pub const TERMINAL_BELIEF: &[&str] = &["contradicted", "deprecated"];
pub const MEMORY_KIND: &[&str] = &["episodic", "semantic"];
pub const MEMORY_STATUS: &[&str] = &[
    "candidate",
    "active",
    "reinforced",
    "consolidated",
    "deprecated",
    "archived",
];
pub const BELIEF_STATUS: &[&str] = &[
    "hypothesis",
    "active",
    "uncertain",
    "contradicted",
    "deprecated",
];
pub const GOAL_STATUS: &[&str] = &[
    "proposed",
    "active",
    "blocked",
    "completed",
    "abandoned",
    "superseded",
];
pub const GOAL_ORIGIN: &[&str] = &["user", "system", "inferred", "self"];
pub const TRAIT_KIND: &[&str] = &["value", "preference", "stance", "style", "relationship"];
pub const TRAIT_STATUS: &[&str] = &["active", "uncertain", "retired"];
pub const MESSAGE_TYPES: &[&str] = &["user_message", "assistant_message"];

/// What can prove an outcome: something a speaker said, something the workspace returned, or
/// the digest code built from those results.
pub fn is_outcome(event: &Row) -> bool {
    matches!(
        event.get("type").and_then(Value::as_str),
        Some("user_message" | "observation" | "episode")
    )
}

/// Raw steps of work, read by reflection through the episode digest instead.
pub const STEP_EVENTS: &[&str] = &["action", "observation", "thought"];

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Proposal {
    #[serde(default)]
    pub reason: Option<String>,
    pub agent: String,
    pub operation: String,
    #[serde(default)]
    pub target: Option<String>,
    #[serde(default)]
    pub payload: Row,
    #[serde(default)]
    pub evidence: Vec<String>,
    #[serde(default = "d_half")]
    pub confidence: f64,
}

impl Proposal {
    pub fn new(agent: &str, operation: &str, payload: Value) -> Proposal {
        Proposal {
            reason: None,
            agent: agent.into(),
            operation: operation.into(),
            target: None,
            payload: obj(payload),
            evidence: Vec::new(),
            confidence: 0.5,
        }
    }

    pub fn reason(mut self, reason: &str) -> Self {
        self.reason = Some(reason.into());
        self
    }

    pub fn target(mut self, t: &str) -> Self {
        self.target = Some(t.into());
        self
    }

    pub fn evidence(mut self, e: Vec<String>) -> Self {
        self.evidence = e;
        self
    }

    pub fn confidence(mut self, c: f64) -> Self {
        self.confidence = c;
        self
    }
}

fn d_half() -> f64 {
    0.5
}
fn d_08() -> f64 {
    0.8
}
fn d_07() -> f64 {
    0.7
}
fn d_06() -> f64 {
    0.6
}
fn d_episodic() -> String {
    "episodic".into()
}
fn d_active() -> String {
    "active".into()
}
fn d_hypothesis() -> String {
    "hypothesis".into()
}
fn d_inferred() -> String {
    "inferred".into()
}

#[derive(Debug, Deserialize)]
pub struct CreateMemory {
    #[serde(default = "d_episodic")]
    pub kind: String,
    pub summary: String,
    #[serde(default = "d_half")]
    pub importance: f64,
    #[serde(default = "d_08")]
    pub confidence: f64,
    #[serde(default = "d_active")]
    pub status: String,
    #[serde(default)]
    pub source_events: Vec<String>,
    #[serde(default)]
    pub entity_ids: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateMemory {
    pub summary: Option<String>,
    pub importance: Option<f64>,
    pub confidence: Option<f64>,
    pub status: Option<String>,
    #[serde(default)]
    pub reinforce: bool,
    #[serde(default)]
    pub add_source_events: Vec<String>,
    #[serde(default)]
    pub add_entity_ids: Vec<String>,
    /// Outcome credit: how strongly the memory was in mind when work was verified or not.
    #[serde(default)]
    pub add_success: f64,
    #[serde(default)]
    pub add_failure: f64,
    pub expected_version: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct MergeMemories {
    pub source_ids: Vec<String>,
    pub summary: String,
    #[serde(default = "d_episodic")]
    pub kind: String,
    #[serde(default = "d_half")]
    pub importance: f64,
    #[serde(default = "d_08")]
    pub confidence: f64,
}

#[derive(Debug, Deserialize)]
pub struct CreateBelief {
    pub proposition: String,
    #[serde(default = "d_half")]
    pub confidence: f64,
    #[serde(default = "d_hypothesis")]
    pub status: String,
}

#[derive(Debug, Deserialize)]
pub struct UpdateBelief {
    pub confidence: Option<f64>,
    pub status: Option<String>,
    #[serde(default)]
    pub add_supporting: Vec<String>,
    #[serde(default)]
    pub add_contradicting: Vec<String>,
    pub expected_version: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct CreateTrait {
    pub kind: String,
    pub statement: String,
    #[serde(default = "d_06")]
    pub confidence: f64,
    pub speaker: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateTrait {
    pub statement: Option<String>,
    pub confidence: Option<f64>,
    pub status: Option<String>,
    #[serde(default)]
    pub add_supporting: Vec<String>,
    #[serde(default)]
    pub add_contradicting: Vec<String>,
    pub expected_version: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct CreateGoal {
    pub description: String,
    #[serde(default = "d_half")]
    pub priority: f64,
    #[serde(default = "d_inferred")]
    pub origin: String,
    #[serde(default = "d_active")]
    pub status: String,
    pub parent_goal: Option<String>,
    pub deadline: Option<String>,
    /// An open loop's kind (`surprise`, `unverified`); absent on every other goal.
    pub kind: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateGoal {
    pub status: Option<String>,
    pub priority: Option<f64>,
    pub next_step: Option<String>,
    pub expected_version: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct CreateJournal {
    pub entry: String,
    pub mood: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct SetNarrative {
    pub text: String,
    #[serde(default)]
    pub sources: std::collections::BTreeMap<String, i64>,
}

#[derive(Debug, Deserialize)]
pub struct CreatePrediction {
    pub prediction: String,
    pub probability: f64,
    pub deadline: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct VerifyPrediction {
    pub verified: bool,
}

#[derive(Debug, Deserialize)]
pub struct UpsertEntity {
    pub name: Option<String>,
    pub kind: Option<String>,
    #[serde(default)]
    pub attributes: Row,
}

#[derive(Debug, Deserialize)]
pub struct AddRelationship {
    pub src: String,
    pub rel: String,
    pub dst: String,
    #[serde(default = "d_07")]
    pub confidence: f64,
}

#[derive(Debug, Deserialize)]
pub struct StatePatch {
    pub patch: Row,
    pub expected_version: Option<i64>,
}

#[derive(Debug)]
pub enum Payload {
    CreateMemory(CreateMemory),
    UpdateMemory(UpdateMemory),
    MergeMemories(MergeMemories),
    CreateBelief(CreateBelief),
    UpdateBelief(UpdateBelief),
    CreateTrait(CreateTrait),
    UpdateTrait(UpdateTrait),
    CreateGoal(CreateGoal),
    UpdateGoal(UpdateGoal),
    CreatePrediction(CreatePrediction),
    VerifyPrediction(VerifyPrediction),
    UpsertEntity(UpsertEntity),
    AddRelationship(AddRelationship),
    SetWorkingState(StatePatch),
    UpdateSelfState(StatePatch),
    CreateJournal(CreateJournal),
    SetNarrative(SetNarrative),
}

impl Payload {
    pub fn expected_version(&self) -> Option<i64> {
        match self {
            Payload::UpdateMemory(p) => p.expected_version,
            Payload::UpdateBelief(p) => p.expected_version,
            Payload::UpdateTrait(p) => p.expected_version,
            Payload::UpdateGoal(p) => p.expected_version,
            Payload::SetWorkingState(p) | Payload::UpdateSelfState(p) => p.expected_version,
            _ => None,
        }
    }
}

fn parse<T: DeserializeOwned>(payload: &Row) -> Result<T, String> {
    serde_json::from_value(Value::Object(payload.clone())).map_err(|e| e.to_string())
}

fn unit(name: &str, x: f64) -> Result<(), String> {
    if (0.0..=1.0).contains(&x) {
        Ok(())
    } else {
        Err(format!("{name}: must be between 0 and 1"))
    }
}

fn unit_opt(name: &str, x: Option<f64>) -> Result<(), String> {
    x.map_or(Ok(()), |x| unit(name, x))
}

fn nonempty(name: &str, s: &str) -> Result<(), String> {
    if s.is_empty() {
        Err(format!("{name}: must have at least 1 character"))
    } else {
        Ok(())
    }
}

fn one_of(name: &str, s: &str, allowed: &[&str]) -> Result<(), String> {
    if allowed.contains(&s) {
        Ok(())
    } else {
        Err(format!("{name}: must be one of {allowed:?}"))
    }
}

fn one_of_opt(name: &str, s: Option<&str>, allowed: &[&str]) -> Result<(), String> {
    s.map_or(Ok(()), |s| one_of(name, s, allowed))
}

fn datetime_opt(name: &str, s: Option<&str>) -> Result<(), String> {
    match s {
        Some(s) if parse_dt(s).is_none() => Err(format!("{name}: invalid datetime")),
        _ => Ok(()),
    }
}

fn parse_patch(payload: &Row) -> Result<StatePatch, String> {
    if payload.contains_key("patch") {
        return parse(payload);
    }
    // A flat state update is already a patch; keep version guards out of the state data.
    let mut patch = payload.clone();
    let version = patch.remove("expected_version").unwrap_or(Value::Null);
    parse(&obj(json!({"patch":patch,"expected_version":version})))
}

/// Validate operation fields at the shared proposal boundary.
pub fn parse_payload(op: &str, payload: &Row) -> Result<Payload, String> {
    Ok(match op {
        "create_memory" => {
            let p: CreateMemory = parse(payload)?;
            one_of("kind", &p.kind, MEMORY_KIND)?;
            nonempty("summary", &p.summary)?;
            unit("importance", p.importance)?;
            unit("confidence", p.confidence)?;
            one_of("status", &p.status, MEMORY_STATUS)?;
            Payload::CreateMemory(p)
        }
        "update_memory" => {
            let p: UpdateMemory = parse(payload)?;
            unit_opt("importance", p.importance)?;
            unit_opt("confidence", p.confidence)?;
            one_of_opt("status", p.status.as_deref(), MEMORY_STATUS)?;
            Payload::UpdateMemory(p)
        }
        "merge_memories" => {
            let p: MergeMemories = parse(payload)?;
            if p.source_ids.len() < 2 {
                return Err("source_ids: must have at least 2 items".into());
            }
            nonempty("summary", &p.summary)?;
            one_of("kind", &p.kind, MEMORY_KIND)?;
            unit("importance", p.importance)?;
            unit("confidence", p.confidence)?;
            Payload::MergeMemories(p)
        }
        "create_belief" => {
            let p: CreateBelief = parse(payload)?;
            nonempty("proposition", &p.proposition)?;
            unit("confidence", p.confidence)?;
            one_of("status", &p.status, BELIEF_STATUS)?;
            Payload::CreateBelief(p)
        }
        "update_belief" => {
            let p: UpdateBelief = parse(payload)?;
            unit_opt("confidence", p.confidence)?;
            one_of_opt("status", p.status.as_deref(), BELIEF_STATUS)?;
            Payload::UpdateBelief(p)
        }
        "create_trait" => {
            let p: CreateTrait = parse(payload)?;
            one_of("kind", &p.kind, TRAIT_KIND)?;
            nonempty("statement", &p.statement)?;
            unit("confidence", p.confidence)?;
            if p.kind == "relationship" && p.speaker.as_deref().unwrap_or("").is_empty() {
                return Err("speaker: required for relationship traits".into());
            }
            Payload::CreateTrait(p)
        }
        "update_trait" => {
            let p: UpdateTrait = parse(payload)?;
            unit_opt("confidence", p.confidence)?;
            one_of_opt("status", p.status.as_deref(), TRAIT_STATUS)?;
            if let Some(st) = &p.statement {
                nonempty("statement", st)?;
            }
            Payload::UpdateTrait(p)
        }
        "create_goal" => {
            let p: CreateGoal = parse(payload)?;
            nonempty("description", &p.description)?;
            unit("priority", p.priority)?;
            one_of("origin", &p.origin, GOAL_ORIGIN)?;
            one_of("status", &p.status, GOAL_STATUS)?;
            datetime_opt("deadline", p.deadline.as_deref())?;
            Payload::CreateGoal(p)
        }
        "update_goal" => {
            let p: UpdateGoal = parse(payload)?;
            one_of_opt("status", p.status.as_deref(), GOAL_STATUS)?;
            unit_opt("priority", p.priority)?;
            Payload::UpdateGoal(p)
        }
        "create_prediction" => {
            let p: CreatePrediction = parse(payload)?;
            nonempty("prediction", &p.prediction)?;
            unit("probability", p.probability)?;
            datetime_opt("deadline", p.deadline.as_deref())?;
            Payload::CreatePrediction(p)
        }
        "verify_prediction" => Payload::VerifyPrediction(parse(payload)?),
        "upsert_entity" => {
            let p: UpsertEntity = parse(payload)?;
            if let Some(name) = &p.name {
                nonempty("name", name)?;
            }
            Payload::UpsertEntity(p)
        }
        "add_relationship" => {
            let p: AddRelationship = parse(payload)?;
            nonempty("rel", &p.rel)?;
            unit("confidence", p.confidence)?;
            Payload::AddRelationship(p)
        }
        "set_working_state" => Payload::SetWorkingState(parse_patch(payload)?),
        "update_self_state" => Payload::UpdateSelfState(parse_patch(payload)?),
        "create_journal" => {
            let p: CreateJournal = parse(payload)?;
            nonempty("entry", &p.entry)?;
            Payload::CreateJournal(p)
        }
        "set_narrative" => {
            let p: SetNarrative = parse(payload)?;
            nonempty("text", &p.text)?;
            Payload::SetNarrative(p)
        }
        other => return Err(format!("unknown operation {other}")),
    })
}

/// Order-preserving union, first occurrence wins.
pub fn union(a: &[String], b: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(a.len() + b.len());
    for x in a.iter().chain(b) {
        if !out.contains(x) {
            out.push(x.clone());
        }
    }
    out
}

pub fn table_for(object_id: &str) -> Option<&'static str> {
    Some(match object_id.split('_').next().unwrap_or("") {
        "mem" => "memories",
        "belief" => "beliefs",
        "trait" => "traits",
        "goal" => "goals",
        "pred" => "predictions",
        "ent" => "entities",
        "rel" => "entity_relationships",
        "journal" => "journal",
        _ => return None,
    })
}

pub fn strings(v: &Value) -> Vec<String> {
    v.as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(String::from)
        .collect()
}

pub fn strings_json(v: &[String]) -> Value {
    json!(v)
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Change {
    pub operation: String,
    pub target: Option<String>,
    pub payload: serde_json::Map<String, Value>,
    pub evidence_ids: Vec<String>,
    pub confidence: f64,
    pub reason: String,
}

impl Change {
    pub fn proposal(self, agent: &str) -> Proposal {
        let mut p = Proposal::new(agent, &self.operation, Value::Object(self.payload));
        p.target = self.target;
        p.evidence = self.evidence_ids;
        p.confidence = self.confidence;
        p.reason = Some(self.reason);
        p
    }
}
