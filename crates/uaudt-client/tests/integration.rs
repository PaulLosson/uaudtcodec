//! End-to-end tests against `tests/demo_server.py`:
//!
//! ```text
//! pip install asyncua beautifulsoup4 lxml
//! python crates/uaudt-client/tests/demo_server.py 48400 &
//! UAUDT_TEST_ENDPOINT=opc.tcp://127.0.0.1:48400/ cargo test -p uaudt-client
//! ```
//! Skipped (passing) when `UAUDT_TEST_ENDPOINT` is not set.

mod common;

use opcua::types::NodeId;
use uaudt_client::{Error, TypeSystem, UdtNode, WriteMode};
use uaudt_core::Value;

fn node(name: &str) -> NodeId {
    NodeId::new(common::ns(), name)
}

#[tokio::test(flavor = "multi_thread")]
async fn end_to_end() {
    let Some(url) = common::endpoint() else {
        return;
    };
    let (session, handle) = common::connect(&url).await;

    // ── Type discovery ──────────────────────────────────────────────
    let types = TypeSystem::browse(&session).await.unwrap();
    assert!(
        types.dictionary_names().any(|n| n == "DemoTypes"),
        "{:?}",
        types.dictionary_names().collect::<Vec<_>>()
    );
    let (dict, _) = types.find_struct("Recipe").unwrap();
    assert!(dict.struct_id("SubItem").is_some());
    // Description and "Default Binary" encoding nodes are both mapped.
    let recipe_nodes = types
        .type_nodes()
        .values()
        .filter(|t| t.type_name == "Recipe")
        .count();
    assert_eq!(recipe_nodes, 2);

    // ── Read ────────────────────────────────────────────────────────
    let mut recipes = UdtNode::open(session.clone(), node("Recipes"), &types)
        .await
        .unwrap();
    assert_eq!(recipes.type_name(), "Recipe");
    assert!(!recipes.is_scalar());
    assert_eq!(recipes.len(), 3);
    let values = recipes.read().await.unwrap();
    let d = recipes.dictionary().clone();
    assert_eq!(d.get_str(&values[2], "Count").unwrap(), &Value::Int32(2));
    assert_eq!(
        d.get_str(&values[2], "SubItems[1].Description").unwrap(),
        &Value::from("Sub item 1")
    );
    assert_eq!(
        d.get_str(&values[2], "Mode")
            .unwrap()
            .display(&d)
            .to_string(),
        "Auto"
    );

    // ── Patch (byte level) → visible after a fresh read ─────────────
    recipes
        .patch(
            1,
            &[
                ("Name", "Patched from Rust".into()),
                ("SubItems[0].Value", 42.into()),
            ],
        )
        .await
        .unwrap();
    let values = recipes.read().await.unwrap();
    assert_eq!(
        d.get_str(&values[1], "Name").unwrap(),
        &Value::from("Patched from Rust")
    );
    assert_eq!(
        d.get_str(&values[1], "SubItems[0].Value").unwrap(),
        &Value::Float(42.0)
    );
    assert_eq!(
        d.get_str(&values[0], "Name").unwrap(),
        &Value::from("Recipe 0"),
        "other elements untouched"
    );

    // ── Edit / write ────────────────────────────────────────────────
    recipes
        .edit(0, |dict, v| dict.set_str(v, "Mode", "Manual".into()))
        .await
        .unwrap();
    let mut v = recipes.decode(2).unwrap();
    d.set_str(
        &mut v,
        "Tags",
        Value::Array(vec!["x".into(), "y".into(), "z".into()]),
    )
    .unwrap();
    recipes.write(2, &v).await.unwrap();
    let values = recipes.read().await.unwrap();
    assert_eq!(
        d.get_str(&values[0], "Mode")
            .unwrap()
            .display(&d)
            .to_string(),
        "Manual"
    );
    assert_eq!(d.get_str(&values[2], "Tags[2]").unwrap(), &Value::from("z"));

    // ── Index-range writes (server permitting) ──────────────────────
    let before = recipes.read().await.unwrap();
    recipes.set_write_mode(WriteMode::IndexRange);
    match recipes.patch(2, &[("Count", 99.into())]).await {
        Ok(()) => {
            let values = recipes.read().await.unwrap();
            if values.len() == before.len() {
                assert_eq!(d.get_str(&values[2], "Count").unwrap(), &Value::Int32(99));
            } else {
                // Some servers (e.g. Python asyncua) answer Good but replace the whole array.
                eprintln!(
                    "server mishandles index-range writes ({} elements left), restoring",
                    values.len()
                );
            }
        }
        Err(Error::BadStatus { status, .. }) => {
            eprintln!("server refuses index-range writes: {status}")
        }
        Err(e) => panic!("{e}"),
    }
    recipes.set_write_mode(WriteMode::FullArray);
    recipes.write_all(&before).await.unwrap();

    // ── write_all, then restore ─────────────────────────────────────
    let original = recipes.read().await.unwrap();
    recipes.write_all(&original[..2]).await.unwrap();
    assert_eq!(recipes.read().await.unwrap().len(), 2);
    recipes.write_all(&original).await.unwrap();
    assert_eq!(recipes.read().await.unwrap(), original);

    // ── Scalar variable ─────────────────────────────────────────────
    let mut single = UdtNode::open(session.clone(), node("SingleRecipe"), &types)
        .await
        .unwrap();
    assert!(single.is_scalar());
    single.patch(0, &[("Name", "Single".into())]).await.unwrap();
    let v = single.read().await.unwrap();
    assert_eq!(d.get_str(&v[0], "Name").unwrap(), &Value::from("Single"));

    // ── Big array ───────────────────────────────────────────────────
    let mut big = UdtNode::open(session.clone(), node("BigRecipes"), &types)
        .await
        .unwrap();
    let values = big.read().await.unwrap();
    assert_eq!(values.len(), 1000);
    assert_eq!(
        d.get_str(&values[999], "SubItems[9].Description").unwrap(),
        &Value::from("Sub item 9")
    );

    // ── Cache: save, reload, open without browsing ──────────────────
    let path = std::env::temp_dir().join(format!("uaudt-types-{}.json", std::process::id()));
    types.save(&path).unwrap();
    let cached = TypeSystem::load(&path).unwrap();
    assert_eq!(cached.type_nodes(), types.type_nodes());
    let reopened = UdtNode::open(session.clone(), node("Recipes"), &cached)
        .await
        .unwrap();
    assert_eq!(reopened.type_name(), "Recipe");
    std::fs::remove_file(&path).ok();

    // ── Errors ──────────────────────────────────────────────────────
    assert!(matches!(
        recipes.patch(10, &[("Name", "x".into())]).await,
        Err(Error::IndexOutOfRange { .. })
    ));
    assert!(matches!(
        recipes.patch(0, &[("Nope", 1.into())]).await,
        Err(Error::Codec(_))
    ));
    assert!(
        UdtNode::open(session.clone(), node("DoesNotExist"), &types)
            .await
            .is_err()
    );

    session.disconnect().await.unwrap();
    handle.await.unwrap();
}
