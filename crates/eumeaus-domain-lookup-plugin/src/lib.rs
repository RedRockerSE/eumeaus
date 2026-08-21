//! `eumeaus-domain-lookup-plugin` — the fourth real plugin: looks up a
//! `Domain` entity's registration data (registrar, creation/expiry dates,
//! nameservers) via RDAP, the free, keyless, JSON-based WHOIS replacement
//! (ICANN-mandated for gTLDs; well-documented and stable). `rdap.org`
//! acts as a bootstrap redirector — a plain HTTP 302 to whichever
//! registry actually holds the TLD (verified live: `rdap.org/domain/
//! example.com` → 302 → `rdap.verisign.com/com/v1/domain/example.com`)
//! — so `reqwest`'s default redirect-following handles TLD discovery for
//! free; there's no bootstrap logic to implement here.
//!
//! Unlike the first three plugins, this one enriches the *target entity
//! itself*, not only new related ones: a `Domain` `EntityFinding` with
//! the same canonical key as the scanned domain auto-merges into that
//! same entity (SPEC.md §4.4's exact-key match), adding a fresh fact
//! carrying registrar/dates/nameservers rather than creating a
//! duplicate. It *also* emits a genuinely new `Organization` entity for
//! the registrar (using the same `"org:{name}"` canonical-key convention
//! `eumeaus-ip-lookup-plugin` already established, so the same registrar
//! seen through either plugin can merge into one entity) — so this one
//! plugin exercises both patterns in the protocol: self-enrichment and
//! new-entity creation.
//!
//! Every check is a real HTTP GET via `reqwest` — including under test,
//! where `EUMEAUS_DOMAIN_LOOKUP_BASE_URL` redirects requests to a local
//! mock server instead of the real domain, without changing any of the
//! request/response handling code. See `tests/`.

use std::collections::HashMap;
use std::time::Duration;

use eumeaus_plugin_protocol::{
    CheckRequest, CheckResult, ConfidenceStatus, EntityFinding, Provenance, RelationshipFinding,
};
use serde::Deserialize;

const DEFAULT_BASE_URL: &str = "https://rdap.org";

#[derive(Debug, Default, Deserialize)]
struct RdapResponse {
    #[serde(default)]
    entities: Vec<RdapEntity>,
    #[serde(default)]
    events: Vec<RdapEvent>,
    #[serde(default)]
    nameservers: Vec<RdapNameserver>,
}

#[derive(Debug, Deserialize)]
struct RdapEntity {
    #[serde(default)]
    roles: Vec<String>,
    #[serde(default, rename = "vcardArray")]
    vcard_array: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct RdapEvent {
    #[serde(rename = "eventAction")]
    event_action: String,
    #[serde(rename = "eventDate")]
    event_date: String,
}

#[derive(Debug, Deserialize)]
struct RdapNameserver {
    #[serde(default, rename = "ldhName")]
    ldh_name: Option<String>,
}

/// A vCard-array's `fn` (formatted name) property — RDAP's own vCard
/// encoding is `["vcard", [ [prop, params, type, value], ... ]]`, too
/// heterogeneous for a plain serde struct, so this navigates the raw
/// `serde_json::Value` directly rather than modeling the whole vCard
/// grammar for the one field this plugin needs.
fn vcard_fn(vcard_array: &serde_json::Value) -> Option<String> {
    let props = vcard_array.as_array()?.get(1)?.as_array()?;
    props.iter().find_map(|prop| {
        let prop = prop.as_array()?;
        if prop.first()?.as_str()? == "fn" {
            prop.get(3)?.as_str().map(|s| s.to_string())
        } else {
            None
        }
    })
}

fn registrar_name(entities: &[RdapEntity]) -> Option<String> {
    entities
        .iter()
        .find(|e| e.roles.iter().any(|r| r == "registrar"))
        .and_then(|e| e.vcard_array.as_ref())
        .and_then(vcard_fn)
}

fn event_date(events: &[RdapEvent], action: &str) -> Option<String> {
    events
        .iter()
        .find(|e| e.event_action == action)
        .map(|e| e.event_date.clone())
}

fn nameserver_list(nameservers: &[RdapNameserver]) -> Option<String> {
    let names: Vec<String> = nameservers
        .iter()
        .filter_map(|n| n.ldh_name.as_deref())
        .map(|n| n.to_lowercase())
        .collect();
    if names.is_empty() {
        None
    } else {
        Some(names.join(", "))
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(bytes))
}

fn now_unix_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before unix epoch")
        .as_millis() as i64
}

fn provenance(source_url: String, raw_response_sha256: String) -> Provenance {
    Provenance {
        source_url,
        retrieval_method: "HTTP GET".to_string(),
        raw_response_sha256,
        collected_at_unix_ms: now_unix_ms(),
        plugin_name: "domain-lookup".to_string(),
        plugin_version: env!("CARGO_PKG_VERSION").to_string(),
    }
}

/// Turns a raw RDAP response body into a `CheckResult`. Separated from
/// the actual HTTP call so it's directly unit-testable without a mock
/// server (the mock-server tests in `tests/check.rs` cover the HTTP
/// layer itself: status codes, transport failures).
fn result_from_body(domain: &str, source_url: String, body: &[u8]) -> CheckResult {
    let raw_response_sha256 = sha256_hex(body);

    let parsed: RdapResponse = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => {
            return CheckResult {
                status: ConfidenceStatus::Error as i32,
                entities: vec![],
                relationships: vec![],
                provenance: Some(provenance(source_url, raw_response_sha256)),
                error_message: format!("invalid response body: {e}"),
            }
        }
    };

    let mut entities = Vec::new();
    let mut relationships = Vec::new();

    // Self-enrichment: same canonical key as the target domain, so this
    // auto-merges a fresh fact onto the entity the scan was already
    // targeting rather than creating a duplicate.
    let mut domain_attrs = HashMap::new();
    if let Some(created) = event_date(&parsed.events, "registration") {
        domain_attrs.insert("created".to_string(), created);
    }
    if let Some(expires) = event_date(&parsed.events, "expiration") {
        domain_attrs.insert("expires".to_string(), expires);
    }
    if let Some(ns) = nameserver_list(&parsed.nameservers) {
        domain_attrs.insert("nameservers".to_string(), ns);
    }
    let registrar = registrar_name(&parsed.entities);
    if let Some(r) = &registrar {
        domain_attrs.insert("registrar".to_string(), r.clone());
    }
    if !domain_attrs.is_empty() {
        entities.push(EntityFinding {
            entity_type: "Domain".to_string(),
            canonical_key: domain.to_string(),
            display_label: domain.to_string(),
            attributes: domain_attrs,
        });
    }

    // A genuinely new entity: the registrar, as its own Organization —
    // "org:{name}" matches eumeaus-ip-lookup-plugin's own convention, so
    // the same registrar seen through either plugin merges into one.
    if let Some(name) = registrar {
        let canonical_key = format!("org:{}", name.trim().to_lowercase());
        entities.push(EntityFinding {
            entity_type: "Organization".to_string(),
            canonical_key: canonical_key.clone(),
            display_label: name,
            attributes: HashMap::from([("role".to_string(), "registrar".to_string())]),
        });
        relationships.push(RelationshipFinding {
            from_canonical_key: domain.to_string(),
            to_canonical_key: canonical_key,
            relationship_type: "AssociatedWith".to_string(),
        });
    }

    CheckResult {
        status: ConfidenceStatus::Found as i32,
        entities,
        relationships,
        provenance: Some(provenance(source_url, raw_response_sha256)),
        error_message: String::new(),
    }
}

/// Checks one domain. Never panics or propagates a transport error out —
/// a request failure becomes `ConfidenceStatus::Error`, per SPEC.md §5
/// ("one bad plugin/site never aborts a scan").
pub async fn check_domain(
    client: &reqwest::Client,
    domain: &str,
    base_override: Option<&str>,
) -> CheckResult {
    let base = base_override.unwrap_or(DEFAULT_BASE_URL);
    let normalized = domain.trim().to_lowercase();
    let url = format!("{}/domain/{normalized}", base.trim_end_matches('/'));

    let response = match client.get(&url).send().await {
        Ok(r) => r,
        Err(e) => {
            return CheckResult {
                status: ConfidenceStatus::Error as i32,
                entities: vec![],
                relationships: vec![],
                provenance: Some(provenance(url, String::new())),
                error_message: e.to_string(),
            }
        }
    };

    let http_status = response.status();
    if http_status.as_u16() == 429 {
        let body = response.bytes().await.unwrap_or_default();
        return CheckResult {
            status: ConfidenceStatus::Uncertain as i32,
            entities: vec![],
            relationships: vec![],
            provenance: Some(provenance(url, sha256_hex(&body))),
            error_message: String::new(),
        };
    }
    if http_status.as_u16() == 404 {
        return CheckResult {
            status: ConfidenceStatus::NotFound as i32,
            entities: vec![],
            relationships: vec![],
            provenance: Some(provenance(url, String::new())),
            error_message: String::new(),
        };
    }
    if !http_status.is_success() {
        return CheckResult {
            status: ConfidenceStatus::Error as i32,
            entities: vec![],
            relationships: vec![],
            provenance: Some(provenance(url, String::new())),
            error_message: format!("unexpected status {http_status}"),
        };
    }

    let body = response.bytes().await.unwrap_or_default();
    result_from_body(domain, url, &body)
}

pub struct DomainLookup {
    client: reqwest::Client,
    base_override: Option<String>,
}

impl DomainLookup {
    pub fn new() -> Self {
        Self {
            // rdap.org sits behind Cloudflare, which returns a bare 403
            // to reqwest's default User-Agent ("reqwest/x.y.z") — a real
            // gotcha caught live (curl worked, this plugin's first real
            // scan against example.com silently got zero data back until
            // this was added). Any non-empty, non-default UA string
            // clears it; this one just reads honestly as what it is.
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(5))
                .user_agent("Mozilla/5.0 (compatible; eumeaus-domain-lookup-plugin/0.1)")
                .build()
                .expect("building the reqwest client cannot fail with this config"),
            base_override: std::env::var("EUMEAUS_DOMAIN_LOOKUP_BASE_URL").ok(),
        }
    }
}

impl Default for DomainLookup {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl eumeaus_plugin_sdk::PluginRuntime for DomainLookup {
    fn describe(&self) -> (String, String) {
        (
            "domain-lookup".to_string(),
            env!("CARGO_PKG_VERSION").to_string(),
        )
    }

    async fn check(&self, request: &CheckRequest) -> Vec<CheckResult> {
        vec![
            check_domain(
                &self.client,
                &request.input_value,
                self.base_override.as_deref(),
            )
            .await,
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn success_body() -> &'static str {
        r#"{
            "entities": [
                {
                    "roles": ["registrar"],
                    "vcardArray": ["vcard", [
                        ["version", {}, "text", "4.0"],
                        ["fn", {}, "text", "Example Registrar, LLC"]
                    ]]
                }
            ],
            "events": [
                {"eventAction": "registration", "eventDate": "1995-08-14T04:00:00Z"},
                {"eventAction": "expiration", "eventDate": "2027-08-13T04:00:00Z"}
            ],
            "nameservers": [
                {"ldhName": "ELLIOTT.NS.CLOUDFLARE.COM"},
                {"ldhName": "HERA.NS.CLOUDFLARE.COM"}
            ]
        }"#
    }

    #[test]
    fn a_success_response_self_enriches_the_domain_and_creates_an_organization() {
        let result = result_from_body(
            "example.com",
            "http://example.invalid".to_string(),
            success_body().as_bytes(),
        );

        assert_eq!(result.status, ConfidenceStatus::Found as i32);
        assert_eq!(result.entities.len(), 2);

        let domain_entity = result
            .entities
            .iter()
            .find(|e| e.entity_type == "Domain")
            .unwrap();
        assert_eq!(domain_entity.canonical_key, "example.com");
        assert_eq!(
            domain_entity.attributes.get("registrar"),
            Some(&"Example Registrar, LLC".to_string())
        );
        assert_eq!(
            domain_entity.attributes.get("created"),
            Some(&"1995-08-14T04:00:00Z".to_string())
        );
        assert_eq!(
            domain_entity.attributes.get("nameservers"),
            Some(&"elliott.ns.cloudflare.com, hera.ns.cloudflare.com".to_string())
        );

        let org = result
            .entities
            .iter()
            .find(|e| e.entity_type == "Organization")
            .unwrap();
        assert_eq!(org.canonical_key, "org:example registrar, llc");

        let rel = &result.relationships[0];
        assert_eq!(rel.from_canonical_key, "example.com");
        assert_eq!(rel.to_canonical_key, "org:example registrar, llc");
        assert_eq!(rel.relationship_type, "AssociatedWith");
    }

    #[test]
    fn missing_registrar_and_dates_skips_both_findings_gracefully() {
        let result = result_from_body("example.com", "http://example.invalid".to_string(), b"{}");

        assert_eq!(result.status, ConfidenceStatus::Found as i32);
        assert!(result.entities.is_empty());
        assert!(result.relationships.is_empty());
    }

    #[test]
    fn invalid_json_is_an_error_not_a_panic() {
        let result = result_from_body(
            "example.com",
            "http://example.invalid".to_string(),
            b"not json",
        );

        assert_eq!(result.status, ConfidenceStatus::Error as i32);
        assert!(!result.error_message.is_empty());
    }
}
