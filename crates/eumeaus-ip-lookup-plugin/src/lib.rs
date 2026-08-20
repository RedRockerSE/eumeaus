//! `eumeaus-ip-lookup-plugin` — the third real plugin (following
//! `eumeaus-username-search-plugin` and `eumeaus-email-lookup-plugin`,
//! SPEC.md §7 M5): geolocates an `IPAddress` entity via ip-api.com's free,
//! no-API-key JSON endpoint.
//!
//! Unlike the first two plugins, this one has no natural second free
//! provider to pair it with (the couple of alternatives considered don't
//! share ip-api.com's exact response shape, and weren't independently
//! re-verified against their live endpoints the way this one was — see
//! `check_ip`'s live HTTP-shape verification during development), so
//! there's no externalized-provider-list config here the way `sites.toml`/
//! `providers.toml` work for the first two. What this plugin proves
//! instead: the protocol generalizes to a check that emits *two different*
//! entity types (`Location`, `Organization`) and *two different*
//! relationship types (`LocatedAt`, `AssociatedWith`) from one HTTP call —
//! a materially different shape than "found on N sites/providers".
//!
//! ip-api.com's free tier is HTTP-only (no TLS) — not this project's
//! choice, the service's own documented limitation — and rate-limited to
//! 45 requests/minute, returning HTTP 429 once exceeded (mapped to
//! `ConfidenceStatus::Uncertain`, same convention as the other two
//! plugins' 429 handling).
//!
//! Every check is a real HTTP GET via `reqwest` — including under test,
//! where `EUMEAUS_IP_LOOKUP_BASE_URL` redirects requests to a local mock
//! server instead of the real domain, without changing any of the
//! request/response handling code. See `tests/`.

use std::collections::HashMap;
use std::time::Duration;

use eumeaus_plugin_protocol::{
    CheckRequest, CheckResult, ConfidenceStatus, EntityFinding, Provenance, RelationshipFinding,
};
use serde::Deserialize;

const DEFAULT_BASE_URL: &str = "http://ip-api.com";
/// ip-api.com's own documented field list — requesting exactly what this
/// plugin uses keeps the response small and avoids depending on fields
/// that might carry more personal/sensitive data than needed (e.g. `mobile`,
/// `proxy`, `hosting` flags exist on the real API but aren't used here).
const FIELDS: &str = "status,message,country,countryCode,region,regionName,city,zip,lat,lon,timezone,isp,org,as,query";

#[derive(Debug, Deserialize)]
struct GeoResponse {
    status: String,
    #[serde(default)]
    country: Option<String>,
    #[serde(default)]
    #[serde(rename = "regionName")]
    region_name: Option<String>,
    #[serde(default)]
    city: Option<String>,
    #[serde(default)]
    zip: Option<String>,
    #[serde(default)]
    lat: Option<f64>,
    #[serde(default)]
    lon: Option<f64>,
    #[serde(default)]
    timezone: Option<String>,
    #[serde(default)]
    isp: Option<String>,
    #[serde(default)]
    org: Option<String>,
    /// ip-api.com's field is literally named `as` (e.g. `"AS15169 Google
    /// LLC"`) — a Rust keyword, hence the rename.
    #[serde(default)]
    #[serde(rename = "as")]
    asn: Option<String>,
}

fn non_empty(s: Option<String>) -> Option<String> {
    s.filter(|v| !v.trim().is_empty())
}

/// A `Location` finding from a successful geolocation response.
/// `canonical_key` is the lat/lon pair (ip-api.com returns city-level
/// precision, ~2 decimal places) rather than the city name string, so two
/// IPs geolocated to the same coordinates auto-merge into one `Location`
/// entity regardless of how the city name happens to be spelled/cased.
/// Returns `None` if the response is missing coordinates (shouldn't
/// happen on a real `"success"` response, but never assume).
fn location_finding(geo: &GeoResponse) -> Option<(EntityFinding, RelationshipFinding, String)> {
    let (lat, lon) = (geo.lat?, geo.lon?);
    let canonical_key = format!("{lat},{lon}");

    let label_parts: Vec<&str> = [
        geo.city.as_deref(),
        geo.region_name.as_deref(),
        geo.country.as_deref(),
    ]
    .into_iter()
    .flatten()
    .filter(|s| !s.trim().is_empty())
    .collect();
    let display_label = if label_parts.is_empty() {
        canonical_key.clone()
    } else {
        label_parts.join(", ")
    };

    let mut attributes = HashMap::new();
    if let Some(city) = non_empty(geo.city.clone()) {
        attributes.insert("city".to_string(), city);
    }
    if let Some(region) = non_empty(geo.region_name.clone()) {
        attributes.insert("region".to_string(), region);
    }
    if let Some(country) = non_empty(geo.country.clone()) {
        attributes.insert("country".to_string(), country);
    }
    if let Some(zip) = non_empty(geo.zip.clone()) {
        attributes.insert("zip".to_string(), zip);
    }
    if let Some(timezone) = non_empty(geo.timezone.clone()) {
        attributes.insert("timezone".to_string(), timezone);
    }
    attributes.insert("lat".to_string(), lat.to_string());
    attributes.insert("lon".to_string(), lon.to_string());

    let entity = EntityFinding {
        entity_type: "Location".to_string(),
        canonical_key: canonical_key.clone(),
        display_label,
        attributes,
    };
    // from_canonical_key filled in by the caller, which has the original
    // request's IP string; a placeholder here would be misleading.
    let relationship = RelationshipFinding {
        from_canonical_key: String::new(),
        to_canonical_key: canonical_key.clone(),
        relationship_type: "LocatedAt".to_string(),
    };
    Some((entity, relationship, canonical_key))
}

/// An `Organization` finding (ISP/network operator) from a successful
/// geolocation response. `canonical_key` prefers the ASN (a genuinely
/// unique network identifier, e.g. `"as15169 google llc"` lowercased) over
/// the org/ISP name, which isn't guaranteed unique across providers.
/// Returns `None` if neither an ASN nor an org/ISP name is present —
/// there's nothing reliable to key an entity on.
fn organization_finding(geo: &GeoResponse) -> Option<(EntityFinding, RelationshipFinding)> {
    let asn = non_empty(geo.asn.clone());
    let org = non_empty(geo.org.clone());
    let isp = non_empty(geo.isp.clone());

    let canonical_key = if let Some(asn) = &asn {
        format!("asn:{}", asn.trim().to_lowercase())
    } else if let Some(org) = &org {
        format!("org:{}", org.trim().to_lowercase())
    } else if let Some(isp) = &isp {
        format!("org:{}", isp.trim().to_lowercase())
    } else {
        return None;
    };

    let display_label = org
        .clone()
        .or_else(|| isp.clone())
        .unwrap_or_else(|| canonical_key.clone());

    let mut attributes = HashMap::new();
    if let Some(isp) = &isp {
        attributes.insert("isp".to_string(), isp.clone());
    }
    if let Some(org) = &org {
        attributes.insert("org".to_string(), org.clone());
    }
    if let Some(asn) = &asn {
        attributes.insert("asn".to_string(), asn.clone());
    }

    let entity = EntityFinding {
        entity_type: "Organization".to_string(),
        canonical_key: canonical_key.clone(),
        display_label,
        attributes,
    };
    let relationship = RelationshipFinding {
        from_canonical_key: String::new(),
        to_canonical_key: canonical_key,
        relationship_type: "AssociatedWith".to_string(),
    };
    Some((entity, relationship))
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
        plugin_name: "ip-lookup".to_string(),
        plugin_version: env!("CARGO_PKG_VERSION").to_string(),
    }
}

/// Turns a raw ip-api.com response body into a `CheckResult`. Separated
/// from the actual HTTP call so it's directly unit-testable without a
/// mock server (the mock-server tests in `tests/check.rs` cover the HTTP
/// layer itself: status codes, transport failures).
fn result_from_body(ip: &str, source_url: String, body: &[u8]) -> CheckResult {
    let raw_response_sha256 = sha256_hex(body);

    let geo: GeoResponse = match serde_json::from_slice(body) {
        Ok(geo) => geo,
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

    if geo.status != "success" {
        // ip-api.com's own "fail" status (e.g. private/reserved range,
        // invalid query) — a clean negative result, not this plugin
        // failing: SPEC.md §5's "absence in the graph is the result".
        return CheckResult {
            status: ConfidenceStatus::NotFound as i32,
            entities: vec![],
            relationships: vec![],
            provenance: Some(provenance(source_url, raw_response_sha256)),
            error_message: String::new(),
        };
    }

    let mut entities = Vec::new();
    let mut relationships = Vec::new();

    if let Some((entity, mut relationship, _)) = location_finding(&geo) {
        relationship.from_canonical_key = ip.to_string();
        entities.push(entity);
        relationships.push(relationship);
    }
    if let Some((entity, mut relationship)) = organization_finding(&geo) {
        relationship.from_canonical_key = ip.to_string();
        entities.push(entity);
        relationships.push(relationship);
    }

    CheckResult {
        status: ConfidenceStatus::Found as i32,
        entities,
        relationships,
        provenance: Some(provenance(source_url, raw_response_sha256)),
        error_message: String::new(),
    }
}

/// Checks one IP address. Never panics or propagates a transport error
/// out — a request failure becomes `ConfidenceStatus::Error`, per
/// SPEC.md §5 ("one bad plugin/site never aborts a scan").
pub async fn check_ip(
    client: &reqwest::Client,
    ip: &str,
    base_override: Option<&str>,
) -> CheckResult {
    let base = base_override.unwrap_or(DEFAULT_BASE_URL);
    let url = format!("{}/json/{ip}?fields={FIELDS}", base.trim_end_matches('/'));

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
    result_from_body(ip, url, &body)
}

pub struct IpLookup {
    client: reqwest::Client,
    base_override: Option<String>,
}

impl IpLookup {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(5))
                .build()
                .expect("building the reqwest client cannot fail with this config"),
            base_override: std::env::var("EUMEAUS_IP_LOOKUP_BASE_URL").ok(),
        }
    }
}

impl Default for IpLookup {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl eumeaus_plugin_sdk::PluginRuntime for IpLookup {
    fn describe(&self) -> (String, String) {
        (
            "ip-lookup".to_string(),
            env!("CARGO_PKG_VERSION").to_string(),
        )
    }

    async fn check(&self, request: &CheckRequest) -> Vec<CheckResult> {
        vec![
            check_ip(
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
        r#"{"status":"success","country":"United States","countryCode":"US","region":"VA","regionName":"Virginia","city":"Ashburn","zip":"20149","lat":39.03,"lon":-77.5,"timezone":"America/New_York","isp":"Google LLC","org":"Google Public DNS","as":"AS15169 Google LLC","query":"8.8.8.8"}"#
    }

    #[test]
    fn a_success_response_produces_a_location_and_an_organization() {
        let result = result_from_body(
            "8.8.8.8",
            "http://example.invalid".to_string(),
            success_body().as_bytes(),
        );

        assert_eq!(result.status, ConfidenceStatus::Found as i32);
        assert_eq!(result.entities.len(), 2);
        assert_eq!(result.relationships.len(), 2);

        let location = result
            .entities
            .iter()
            .find(|e| e.entity_type == "Location")
            .unwrap();
        assert_eq!(location.canonical_key, "39.03,-77.5");
        assert_eq!(location.display_label, "Ashburn, Virginia, United States");
        assert_eq!(
            location.attributes.get("city"),
            Some(&"Ashburn".to_string())
        );

        let org = result
            .entities
            .iter()
            .find(|e| e.entity_type == "Organization")
            .unwrap();
        assert_eq!(org.canonical_key, "asn:as15169 google llc");

        let located_at = result
            .relationships
            .iter()
            .find(|r| r.relationship_type == "LocatedAt")
            .unwrap();
        assert_eq!(located_at.from_canonical_key, "8.8.8.8");
        assert_eq!(located_at.to_canonical_key, "39.03,-77.5");

        let associated_with = result
            .relationships
            .iter()
            .find(|r| r.relationship_type == "AssociatedWith")
            .unwrap();
        assert_eq!(associated_with.from_canonical_key, "8.8.8.8");
    }

    #[test]
    fn a_fail_status_is_not_found_not_an_error() {
        let body = r#"{"status":"fail","message":"private range","query":"10.0.0.1"}"#;
        let result = result_from_body(
            "10.0.0.1",
            "http://example.invalid".to_string(),
            body.as_bytes(),
        );

        assert_eq!(result.status, ConfidenceStatus::NotFound as i32);
        assert!(result.entities.is_empty());
        assert!(result.relationships.is_empty());
    }

    #[test]
    fn invalid_json_is_an_error_not_a_panic() {
        let result = result_from_body("8.8.8.8", "http://example.invalid".to_string(), b"not json");

        assert_eq!(result.status, ConfidenceStatus::Error as i32);
        assert!(!result.error_message.is_empty());
    }

    #[test]
    fn missing_org_and_isp_and_asn_skips_the_organization_entity() {
        let body = r#"{"status":"success","lat":1.0,"lon":2.0,"city":"Nowhere"}"#;
        let result = result_from_body(
            "1.2.3.4",
            "http://example.invalid".to_string(),
            body.as_bytes(),
        );

        assert_eq!(result.entities.len(), 1);
        assert_eq!(result.entities[0].entity_type, "Location");
    }

    #[test]
    fn organization_key_falls_back_to_org_name_then_isp_when_asn_is_absent() {
        let body = r#"{"status":"success","lat":1.0,"lon":2.0,"isp":"Some ISP"}"#;
        let result = result_from_body(
            "1.2.3.4",
            "http://example.invalid".to_string(),
            body.as_bytes(),
        );

        let org = result
            .entities
            .iter()
            .find(|e| e.entity_type == "Organization")
            .unwrap();
        assert_eq!(org.canonical_key, "org:some isp");
    }
}
