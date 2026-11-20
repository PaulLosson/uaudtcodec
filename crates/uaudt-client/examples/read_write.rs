//! Minimal usage: connect, read every element, patch one, edit another.
//!
//! `cargo run --release -p uaudt-client --example read_write -- opc.tcp://127.0.0.1:48400/ "ns=2;s=Recipes"`
use opcua::client::{ClientBuilder, IdentityToken};
use opcua::types::{EndpointDescription, MessageSecurityMode, NodeId, UserTokenPolicy};
use uaudt_client::{TypeSystem, UdtNode};
use uaudt_core::Value;

#[tokio::main]
async fn main() -> uaudt_client::Result<()> {
    let mut args = std::env::args().skip(1);
    let url = args
        .next()
        .unwrap_or_else(|| "opc.tcp://127.0.0.1:48400/".into());
    let node_id: NodeId = args
        .next()
        .unwrap_or_else(|| "ns=2;s=Recipes".into())
        .parse()
        .expect("invalid NodeId");

    // async-opcua session (raise the default decoding limits for UDT traffic).
    let mut client = uaudt_client::with_udt_limits(ClientBuilder::new())
        .application_name("uaudt example")
        .application_uri("urn:uaudt:example")
        .pki_dir(std::env::temp_dir().join("uaudt-example-pki"))
        .trust_server_certs(true)
        .session_retry_limit(3)
        .client()
        .expect("client config");
    let endpoint: EndpointDescription = (
        url.as_str(),
        "None",
        MessageSecurityMode::None,
        UserTokenPolicy::anonymous(),
    )
        .into();
    let (session, event_loop) = client
        .connect_to_matching_endpoint(endpoint, IdentityToken::Anonymous)
        .await?;
    let handle = event_loop.spawn();
    session.wait_for_connection().await;

    // Type dictionaries: browsed once, then loaded from the cache file.
    let types =
        TypeSystem::load_or_browse(&session, std::env::temp_dir().join("uaudt-types.json")).await?;
    let mut node = UdtNode::open(session.clone(), node_id, &types).await?;
    let dict = node.dictionary().clone();
    println!("{} element(s) of type {}", node.len(), node.type_name());

    for (i, item) in node.read().await?.into_iter().enumerate() {
        println!("[{i}] {}", Value::Struct(item).display(&dict));
    }

    if node.len() > 1 {
        // Byte-level partial update of element 1.
        node.patch(1, &[("Name", "Updated from Rust".into())])
            .await?;
        // Decode / modify / re-encode element 0.
        node.edit(0, |dict, v| dict.set_str(v, "Enabled", false.into()))
            .await?;
        let v = node.read().await?;
        println!(
            "after: [1].Name = {}",
            dict.get_str(&v[1], "Name")?.display(&dict)
        );
    }

    session.disconnect().await.ok();
    handle.await.ok();
    Ok(())
}
