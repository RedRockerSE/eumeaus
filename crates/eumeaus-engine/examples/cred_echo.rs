//! Test fixture only (see scan_ok.rs's doc comment). Reports back whether
//! it received the credential it declared needing, via
//! `CheckRequest.resolved_credentials`, and whether that value shows up
//! anywhere in its own argv or environment — used by scan.rs's M6
//! credential-injection test (SPEC.md §7).

use std::collections::HashMap;

use eumeaus_plugin_protocol::{CheckRequest, CheckResult, ConfidenceStatus, EntityFinding};
use eumeaus_plugin_sdk::PluginRuntime;

struct CredEcho;

#[async_trait::async_trait]
impl PluginRuntime for CredEcho {
    fn describe(&self) -> (String, String) {
        ("cred-echo".to_string(), "0.1.0".to_string())
    }

    async fn check(&self, request: &CheckRequest) -> Vec<CheckResult> {
        let received = request
            .resolved_credentials
            .get("test_credential")
            .cloned()
            .unwrap_or_else(|| "MISSING".to_string());

        let leaked_via_argv = std::env::args().any(|a| a == received);
        let leaked_via_env = std::env::vars().any(|(_, v)| v == received);

        let mut attributes = HashMap::new();
        attributes.insert("received_credential".to_string(), received);
        attributes.insert("leaked_via_argv".to_string(), leaked_via_argv.to_string());
        attributes.insert("leaked_via_env".to_string(), leaked_via_env.to_string());

        vec![CheckResult {
            status: ConfidenceStatus::Found as i32,
            entities: vec![EntityFinding {
                entity_type: "OnlineAccount".to_string(),
                canonical_key: format!("cred-echo:{}", request.input_value),
                display_label: "cred-echo result".to_string(),
                attributes,
            }],
            relationships: vec![],
            provenance: None,
            error_message: String::new(),
        }]
    }
}

#[tokio::main]
async fn main() {
    eumeaus_plugin_sdk::serve(CredEcho)
        .await
        .expect("cred-echo plugin server failed");
}
