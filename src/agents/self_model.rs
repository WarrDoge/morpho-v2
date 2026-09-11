//! Fields an agent may revise in its operational self model.
use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelfPatch {
    pub limitations: Vec<String>,
    pub commitments: Vec<String>,
    pub recent_actions: Vec<String>,
    pub known_failures: Vec<String>,
    pub uncertainties: Vec<String>,
    pub current_objectives: Vec<String>,
}
