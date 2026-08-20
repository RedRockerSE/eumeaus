//! Exercises `check_ip`'s HTTP layer against a real local server (wiremock)
//! standing in for ip-api.com — real request/response handling, just not
//! the real internet. Response-body parsing/entity-building logic itself
//! is unit-tested directly in `src/lib.rs` against real captured response
//! shapes; this covers what only a real HTTP round trip can: status codes,
//! transport failures.

use eumeaus_ip_lookup_plugin::check_ip;
use eumeaus_plugin_protocol::ConfidenceStatus;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn client() -> reqwest::Client {
    reqwest::Client::new()
}

#[tokio::test]
async fn a_successful_response_is_found() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/json/8.8.8.8"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{"status":"success","country":"United States","city":"Ashburn","lat":39.03,"lon":-77.5,"isp":"Google LLC","as":"AS15169 Google LLC","query":"8.8.8.8"}"#,
        ))
        .mount(&server)
        .await;

    let result = check_ip(&client(), "8.8.8.8", Some(&server.uri())).await;

    assert_eq!(result.status, ConfidenceStatus::Found as i32);
    assert_eq!(result.entities.len(), 2);
    let prov = result.provenance.expect("provenance recorded");
    assert!(!prov.raw_response_sha256.is_empty());
}

#[tokio::test]
async fn a_fail_status_response_is_not_found() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/json/10.0.0.1"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string(
                r#"{"status":"fail","message":"private range","query":"10.0.0.1"}"#,
            ),
        )
        .mount(&server)
        .await;

    let result = check_ip(&client(), "10.0.0.1", Some(&server.uri())).await;

    assert_eq!(result.status, ConfidenceStatus::NotFound as i32);
    assert!(result.entities.is_empty());
}

#[tokio::test]
async fn rate_limited_is_uncertain_not_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/json/8.8.8.8"))
        .respond_with(ResponseTemplate::new(429))
        .mount(&server)
        .await;

    let result = check_ip(&client(), "8.8.8.8", Some(&server.uri())).await;

    assert_eq!(
        result.status,
        ConfidenceStatus::Uncertain as i32,
        "SPEC.md §5: a 429 means the plugin couldn't tell, not that it failed"
    );
    assert!(result.entities.is_empty());
}

#[tokio::test]
async fn unexpected_status_is_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/json/8.8.8.8"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;

    let result = check_ip(&client(), "8.8.8.8", Some(&server.uri())).await;

    assert_eq!(result.status, ConfidenceStatus::Error as i32);
    assert!(!result.error_message.is_empty());
}

#[tokio::test]
async fn unreachable_host_is_error_not_a_panic() {
    // Port 1 is reserved and nothing will ever be listening there — this
    // exercises the connection-failure path, not just non-2xx responses.
    let result = check_ip(&client(), "8.8.8.8", Some("http://127.0.0.1:1")).await;

    assert_eq!(result.status, ConfidenceStatus::Error as i32);
    assert!(!result.error_message.is_empty());
}
