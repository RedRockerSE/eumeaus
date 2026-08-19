//! Exercises `check_site`'s detection logic against a real local HTTP
//! server (wiremock) standing in for each real site — real request/response
//! handling, just not the real internet. This is the "cassette" SPEC.md §6
//! calls for: recorded/canned responses replayed deterministically.

use eumeaus_plugin_protocol::ConfidenceStatus;
use eumeaus_username_search_plugin::{check_site, default_sites, Site};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn site(slug: &str) -> Site {
    default_sites()
        .into_iter()
        .find(|s| s.slug == slug)
        .unwrap()
}

fn client() -> reqwest::Client {
    reqwest::Client::new()
}

#[tokio::test]
async fn status_code_site_found() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/github/carol"))
        .respond_with(ResponseTemplate::new(200).set_body_string("<html>carol's profile</html>"))
        .mount(&server)
        .await;

    let result = check_site(&client(), &site("github"), "carol", Some(&server.uri())).await;

    assert_eq!(result.status, ConfidenceStatus::Found as i32);
    assert_eq!(result.entities.len(), 1);
    assert_eq!(result.entities[0].canonical_key, "github:carol");
    assert_eq!(result.relationships.len(), 1);
    assert_eq!(result.relationships[0].from_canonical_key, "carol");
    assert_eq!(result.relationships[0].to_canonical_key, "github:carol");
    let prov = result.provenance.expect("provenance recorded");
    assert!(!prov.raw_response_sha256.is_empty());
}

#[tokio::test]
async fn status_code_site_not_found() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/github/nobody"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;

    let result = check_site(&client(), &site("github"), "nobody", Some(&server.uri())).await;

    assert_eq!(result.status, ConfidenceStatus::NotFound as i32);
    assert!(result.entities.is_empty());
    assert!(result.relationships.is_empty());
}

#[tokio::test]
async fn body_marker_site_found() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/flaky-forum/u/carol"))
        .respond_with(ResponseTemplate::new(200).set_body_string("<html>Welcome, carol!</html>"))
        .mount(&server)
        .await;

    let result = check_site(
        &client(),
        &site("flaky-forum"),
        "carol",
        Some(&server.uri()),
    )
    .await;

    assert_eq!(result.status, ConfidenceStatus::Found as i32);
    assert_eq!(result.entities[0].canonical_key, "flaky-forum:carol");
}

#[tokio::test]
async fn body_marker_site_not_found() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/flaky-forum/u/nobody"))
        .respond_with(ResponseTemplate::new(200).set_body_string("<html>Profile not found</html>"))
        .mount(&server)
        .await;

    let result = check_site(
        &client(),
        &site("flaky-forum"),
        "nobody",
        Some(&server.uri()),
    )
    .await;

    assert_eq!(result.status, ConfidenceStatus::NotFound as i32);
    assert!(result.entities.is_empty());
}

#[tokio::test]
async fn rate_limited_site_is_uncertain_not_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/github/carol"))
        .respond_with(ResponseTemplate::new(429))
        .mount(&server)
        .await;

    let result = check_site(&client(), &site("github"), "carol", Some(&server.uri())).await;

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
        .and(path("/github/carol"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;

    let result = check_site(&client(), &site("github"), "carol", Some(&server.uri())).await;

    assert_eq!(result.status, ConfidenceStatus::Error as i32);
    assert!(!result.error_message.is_empty());
}

#[tokio::test]
async fn unreachable_host_is_error_not_a_panic() {
    // Port 1 is reserved and nothing will ever be listening there — this
    // exercises the connection-failure path, not just non-2xx responses.
    let result = check_site(
        &client(),
        &site("github"),
        "carol",
        Some("http://127.0.0.1:1"),
    )
    .await;

    assert_eq!(result.status, ConfidenceStatus::Error as i32);
    assert!(!result.error_message.is_empty());
}
