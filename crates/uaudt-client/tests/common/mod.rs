//! Connection helper shared by the integration tests.
use std::sync::Arc;

use opcua::client::{ClientBuilder, IdentityToken, Session};
use opcua::types::{EndpointDescription, MessageSecurityMode, StatusCode, UserTokenPolicy};

/// Endpoint of the demo server (`tests/demo_server.py`), or `None` to skip.
pub fn endpoint() -> Option<String> {
    let url = std::env::var("UAUDT_TEST_ENDPOINT").ok();
    if url.is_none() {
        eprintln!(
            "UAUDT_TEST_ENDPOINT not set: start tests/demo_server.py and set it to run this test"
        );
    }
    url
}

/// Namespace index of the demo nodes (`UAUDT_TEST_NS`, default 2).
pub fn ns() -> u16 {
    std::env::var("UAUDT_TEST_NS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(2)
}

pub async fn connect(url: &str) -> (Arc<Session>, tokio::task::JoinHandle<StatusCode>) {
    let pki = std::env::temp_dir().join("uaudt-client-test-pki");
    let mut client = uaudt_client::with_udt_limits(ClientBuilder::new())
        .application_name("uaudt-client tests")
        .application_uri("urn:uaudt-client:tests")
        .pki_dir(pki)
        .create_sample_keypair(false)
        .trust_server_certs(true)
        .session_retry_limit(1)
        .client()
        .expect("client config");
    let endpoint: EndpointDescription = (
        url,
        "None",
        MessageSecurityMode::None,
        UserTokenPolicy::anonymous(),
    )
        .into();
    let (session, event_loop) = client
        .connect_to_matching_endpoint(endpoint, IdentityToken::Anonymous)
        .await
        .expect("connect");
    let handle = event_loop.spawn();
    session.wait_for_connection().await;
    (session, handle)
}
