//! Test fixture only (not compiled by plain `cargo build` — see
//! CLAUDE.md's note on why fixture plugins are Cargo examples, not
//! `[[bin]]` targets). A second, near-instant plugin alongside the real
//! username-search-plugin in tests/e2e_v1_proof.rs, so that test can kill
//! the CLI process after this one has already finished but while
//! username-search is still in flight — and then show resume only
//! re-invokes the one that didn't finish.

use eumeaus_plugin_protocol::{
    CheckRequest, CheckResult, ConfidenceStatus, EntityFinding, RelationshipFinding,
};
use eumeaus_plugin_sdk::PluginRuntime;

struct QuickCheck;

#[async_trait::async_trait]
impl PluginRuntime for QuickCheck {
    fn describe(&self) -> (String, String) {
        ("quick-check".to_string(), "0.1.0".to_string())
    }

    async fn check(&self, request: &CheckRequest) -> Vec<CheckResult> {
        let account_key = format!("quickcheck:{}", request.input_value);
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
    eumeaus_plugin_sdk::serve(QuickCheck)
        .await
        .expect("quick-check plugin server failed");
}
