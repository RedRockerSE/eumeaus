//! Exercises `check_domain` against a real local HTTP server (wiremock)
//! standing in for crt.sh — real request/response handling, just not the
//! real internet. Mirrors `eumeaus-ip-lookup-plugin/tests/check.rs`'s own
//! pattern, plus two cases novel to this plugin: retry-then-succeed and
//! retry-exhausted, since no other shipped plugin has retry logic yet.

use eumeaus_plugin_protocol::ConfidenceStatus;
use eumeaus_subdomain_lookup_plugin::check_domain;
use wiremock::matchers::{method, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn client() -> reqwest::Client {
    reqwest::Client::new()
}

fn crt_sh_body(names: &[&str]) -> serde_json::Value {
    serde_json::Value::Array(
        names
            .iter()
            .map(|n| serde_json::json!({"name_value": n}))
            .collect(),
    )
}

#[tokio::test]
async fn a_2xx_response_produces_found_with_domain_entities() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(query_param("q", "example.com"))
        .respond_with(ResponseTemplate::new(200).set_body_json(crt_sh_body(&[
            "www.example.com\nexample.com",
            "api.example.com",
        ])))
        .mount(&server)
        .await;

    let result = check_domain(&client(), "example.com", Some(&server.uri())).await;

    assert_eq!(result.status, ConfidenceStatus::Found as i32);
    assert_eq!(result.entities.len(), 2);
    assert!(result.entities.iter().all(|e| e.entity_type == "Domain"));
    assert!(result
        .relationships
        .iter()
        .all(|r| r.from_canonical_key == "example.com" && r.relationship_type == "HasSubdomain"));
    let prov = result.provenance.expect("provenance recorded");
    assert!(!prov.raw_response_sha256.is_empty());
}

#[tokio::test]
async fn an_empty_result_set_is_not_found() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(query_param("q", "example.com"))
        .respond_with(ResponseTemplate::new(200).set_body_json(crt_sh_body(&[])))
        .mount(&server)
        .await;

    let result = check_domain(&client(), "example.com", Some(&server.uri())).await;

    assert_eq!(result.status, ConfidenceStatus::NotFound as i32);
    assert!(result.entities.is_empty());
}

#[tokio::test]
async fn a_502_on_every_attempt_is_uncertain_not_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(query_param("q", "example.com"))
        .respond_with(ResponseTemplate::new(502))
        .mount(&server)
        .await;

    let result = check_domain(&client(), "example.com", Some(&server.uri())).await;

    assert_eq!(
        result.status,
        ConfidenceStatus::Uncertain as i32,
        "crt.sh being unavailable after retries isn't evidence the domain has no subdomains"
    );
    assert!(result.entities.is_empty());
    assert!(result.error_message.is_empty());
}

#[tokio::test]
async fn a_502_on_the_first_attempt_succeeds_on_retry() {
    let server = MockServer::start().await;
    // First matching request gets a 502, every subsequent one gets a
    // real 2xx body — proves the retry loop itself, not just its
    // final-failure fallback.
    Mock::given(method("GET"))
        .and(query_param("q", "example.com"))
        .respond_with(ResponseTemplate::new(502))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(query_param("q", "example.com"))
        .respond_with(ResponseTemplate::new(200).set_body_json(crt_sh_body(&["www.example.com"])))
        .mount(&server)
        .await;

    let result = check_domain(&client(), "example.com", Some(&server.uri())).await;

    assert_eq!(result.status, ConfidenceStatus::Found as i32);
    assert_eq!(result.entities.len(), 1);
    assert_eq!(result.entities[0].canonical_key, "www.example.com");
}

#[tokio::test]
async fn rate_limited_is_uncertain_not_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(query_param("q", "example.com"))
        .respond_with(ResponseTemplate::new(429))
        .mount(&server)
        .await;

    let result = check_domain(&client(), "example.com", Some(&server.uri())).await;

    assert_eq!(result.status, ConfidenceStatus::Uncertain as i32);
    assert!(result.entities.is_empty());
}

#[tokio::test]
async fn malformed_json_on_a_2xx_is_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(query_param("q", "example.com"))
        .respond_with(ResponseTemplate::new(200).set_body_string("<html>not json</html>"))
        .mount(&server)
        .await;

    let result = check_domain(&client(), "example.com", Some(&server.uri())).await;

    assert_eq!(result.status, ConfidenceStatus::Error as i32);
    assert!(!result.error_message.is_empty());
}

#[tokio::test]
async fn unreachable_host_is_uncertain_not_a_panic() {
    // Port 1 is reserved and nothing will ever be listening there — this
    // exercises the transport-failure retry path, not just non-2xx
    // responses.
    let result = check_domain(&client(), "example.com", Some("http://127.0.0.1:1")).await;

    assert_eq!(result.status, ConfidenceStatus::Uncertain as i32);
}
