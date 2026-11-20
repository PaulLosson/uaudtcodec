//! End-to-end timings against `tests/demo_server.py` (compare with `bench/bench_python_e2e.py`).
//!
//! `cargo run --release -p uaudt-client --example bench_e2e -- opc.tcp://127.0.0.1:48400/ [ns]`
use std::sync::Arc;
use std::time::{Duration, Instant};

use opcua::client::{ClientBuilder, IdentityToken};
use opcua::types::{EndpointDescription, MessageSecurityMode, NodeId, UserTokenPolicy};
use uaudt_client::{TypeSystem, UdtNode};

fn report(name: &str, samples: &mut [Duration]) {
    samples.sort();
    println!(
        "{name:34} median {:>10.3?}  min {:>10.3?}",
        samples[samples.len() / 2],
        samples[0]
    );
}

#[tokio::main]
async fn main() -> uaudt_client::Result<()> {
    let url = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "opc.tcp://127.0.0.1:48400/".into());
    let ns: u16 = std::env::args()
        .nth(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(2);
    let mut client = uaudt_client::with_udt_limits(ClientBuilder::new())
        .application_name("uaudt bench")
        .application_uri("urn:uaudt:bench")
        .pki_dir(std::env::temp_dir().join("uaudt-bench-pki"))
        .create_sample_keypair(false)
        .trust_server_certs(true)
        .session_retry_limit(1)
        .client()
        .expect("client");
    let endpoint: EndpointDescription = (
        url.as_str(),
        "None",
        MessageSecurityMode::None,
        UserTokenPolicy::anonymous(),
    )
        .into();
    let (session, event_loop) = client
        .connect_to_matching_endpoint(endpoint, IdentityToken::Anonymous)
        .await
        .unwrap();
    let handle = event_loop.spawn();
    session.wait_for_connection().await;
    let session: Arc<_> = session;
    let n = 10;

    let mut t = Vec::new();
    let mut types = None;
    for _ in 0..n {
        let s = Instant::now();
        let ts = TypeSystem::browse(&session).await?;
        let node = UdtNode::open(session.clone(), NodeId::new(ns, "BigRecipes"), &ts).await?;
        t.push(s.elapsed());
        assert_eq!(node.len(), 1000);
        types = Some(ts);
    }
    report("init (browse i=93 + open)", &mut t);
    let types = types.unwrap();

    let mut node = UdtNode::open(session.clone(), NodeId::new(ns, "BigRecipes"), &types).await?;
    let mut t = Vec::new();
    for _ in 0..n {
        let s = Instant::now();
        let v = node.read().await?;
        t.push(s.elapsed());
        assert_eq!(v.len(), 1000);
    }
    report("read + decode 1000 recipes", &mut t);

    let mut t = Vec::new();
    for _ in 0..n {
        let s = Instant::now();
        let v = node.decode_all()?;
        t.push(s.elapsed());
        assert_eq!(v.len(), 1000);
    }
    report("decode only 1000 recipes", &mut t);

    let mut t = Vec::new();
    for i in 0..n {
        let s = Instant::now();
        node.patch(
            500,
            &[
                ("Name", format!("Patched {i}").into()),
                ("SubItems[3].Value", 1.0.into()),
            ],
        )
        .await?;
        t.push(s.elapsed());
    }
    report("patch 1 element (full array write)", &mut t);

    let mut small = UdtNode::open(session.clone(), NodeId::new(ns, "Recipes"), &types).await?;
    let mut t = Vec::new();
    for i in 0..n * 5 {
        let s = Instant::now();
        small.patch(1, &[("Count", i.into())]).await?;
        t.push(s.elapsed());
    }
    report("patch 1 element (3-element array)", &mut t);

    session.disconnect().await.ok();
    handle.await.ok();
    Ok(())
}
