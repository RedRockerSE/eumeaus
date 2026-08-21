//! Exercises `check_domain`'s HTTP layer against a real local server
//! (wiremock) standing in for RDAP — real request/response handling,
//! just not the real internet or the real bootstrap redirect. Response-
//! body parsing/entity-building logic itself is unit-tested directly in
//! `src/lib.rs` against a real captured response shape; this covers what
//! only a real HTTP round trip can: status codes, transport failures.

use eumeaus_domain_lookup_plugin::check_domain;
use eumeaus_plugin_protocol::ConfidenceStatus;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn client() -> reqwest::Client {
    reqwest::Client::new()
}

#[tokio::test]
async fn a_registered_domain_is_found() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/domain/example.com"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{"entities":[{"roles":["registrar"],"vcardArray":["vcard",[["fn",{},"text","Example Registrar"]]]}],"events":[],"nameservers":[]}"#,
        ))
        .mount(&server)
        .await;

    let result = check_domain(&client(), "example.com", Some(&server.uri())).await;

    assert_eq!(result.status, ConfidenceStatus::Found as i32);
    assert_eq!(result.entities.len(), 2);
    let prov = result.provenance.expect("provenance recorded");
    assert!(!prov.raw_response_sha256.is_empty());
}

#[tokio::test]
async fn an_unregistered_domain_is_not_found() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/domain/nobody-has-this.example"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;

    let result = check_domain(&client(), "nobody-has-this.example", Some(&server.uri())).await;

    assert_eq!(result.status, ConfidenceStatus::NotFound as i32);
    assert!(result.entities.is_empty());
}

#[tokio::test]
async fn domain_is_normalized_before_the_request() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/domain/example.com"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(r#"{"entities":[],"events":[],"nameservers":[]}"#),
        )
        .mount(&server)
        .await;

    let result = check_domain(&client(), " Example.COM ", Some(&server.uri())).await;

    assert_eq!(result.status, ConfidenceStatus::Found as i32);
}

#[tokio::test]
async fn rate_limited_is_uncertain_not_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/domain/example.com"))
        .respond_with(ResponseTemplate::new(429))
        .mount(&server)
        .await;

    let result = check_domain(&client(), "example.com", Some(&server.uri())).await;

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
        .and(path("/domain/example.com"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;

    let result = check_domain(&client(), "example.com", Some(&server.uri())).await;

    assert_eq!(result.status, ConfidenceStatus::Error as i32);
    assert!(!result.error_message.is_empty());
}

#[tokio::test]
async fn unreachable_host_is_error_not_a_panic() {
    // Port 1 is reserved and nothing will ever be listening there — this
    // exercises the connection-failure path, not just non-2xx responses.
    let result = check_domain(&client(), "example.com", Some("http://127.0.0.1:1")).await;

    assert_eq!(result.status, ConfidenceStatus::Error as i32);
    assert!(!result.error_message.is_empty());
}
