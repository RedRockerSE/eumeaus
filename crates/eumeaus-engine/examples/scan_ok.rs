//! Test fixture only (not compiled by plain `cargo build`, only `cargo
//! test` — see CLAUDE.md's note on why these are examples, not `[[bin]]`
//! targets). Responds immediately with one FOUND entity + a relationship
//! back to the scan's target, exercising both halves of result ingestion.

use eumeaus_plugin_protocol::{
    CheckRequest, CheckResult, ConfidenceStatus, EntityFinding, RelationshipFinding,
};
use eumeaus_plugin_sdk::PluginRuntime;

struct ScanOk;

impl PluginRuntime for ScanOk {
    fn describe(&self) -> (String, String) {
        ("scan-ok".to_string(), "0.1.0".to_string())
    }

    fn check(&self, request: &CheckRequest) -> Vec<CheckResult> {
        let account_key = format!("{}-account", request.input_value);
        vec![CheckResult {
            status: ConfidenceStatus::Found as i32,
            entities: vec![EntityFinding {
                entity_type: "OnlineAccount".to_string(),
                canonical_key: account_key.clone(),
                display_label: account_key.clone(),
                attributes: Default::default(),
            }],
            relationships: vec![RelationshipFinding {
                from_canonical_key: request.input_value.clone(),
                to_canonical_key: account_key,
                relationship_type: "HasAccount".to_string(),
            }],
            provenance: None,
            error_message: String::new(),
        }]
    }
}

#[tokio::main]
async fn main() {
    eumeaus_plugin_sdk::serve(ScanOk)
        .await
        .expect("scan-ok plugin server failed");
}
