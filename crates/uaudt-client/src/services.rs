//! Batched service helpers: every helper sends as few requests as possible
//! (chunked to stay below typical server operation limits, chunks in flight
//! concurrently) and follows browse continuation points.

use futures::future::try_join_all;
use opcua::client::Session;
use opcua::types::{
    AttributeId, BrowseDescription, BrowseDirection, BrowseResultMask, DataValue, NodeClassMask,
    NodeId, NumericRange, QualifiedName, ReadValueId, ReferenceDescription, TimestampsToReturn,
};

use crate::error::{Result, check};

/// Max operations per request (well below common server limits).
const CHUNK: usize = 500;

/// Reads one attribute of many nodes.
pub(crate) async fn read_attribute(
    session: &Session,
    nodes: &[NodeId],
    attribute: AttributeId,
) -> Result<Vec<DataValue>> {
    let requests = nodes.chunks(CHUNK).map(|chunk| async move {
        let ids: Vec<ReadValueId> = chunk
            .iter()
            .map(|n| ReadValueId {
                node_id: n.clone(),
                attribute_id: attribute as u32,
                index_range: NumericRange::None,
                data_encoding: QualifiedName::null(),
            })
            .collect();
        session.read(&ids, TimestampsToReturn::Neither, 0.0).await
    });
    Ok(try_join_all(requests)
        .await?
        .into_iter()
        .flatten()
        .collect())
}

/// Browses many nodes and returns, per node, all its references matching the filter.
pub(crate) async fn browse(
    session: &Session,
    nodes: &[NodeId],
    direction: BrowseDirection,
    reference_type: NodeId,
    node_class_mask: NodeClassMask,
) -> Result<Vec<Vec<ReferenceDescription>>> {
    let requests = nodes.chunks(CHUNK).map(|chunk| {
        let reference_type = reference_type.clone();
        async move {
            let descs: Vec<BrowseDescription> = chunk
                .iter()
                .map(|n| BrowseDescription {
                    node_id: n.clone(),
                    browse_direction: direction,
                    reference_type_id: reference_type.clone(),
                    include_subtypes: true,
                    node_class_mask: node_class_mask.bits(),
                    result_mask: BrowseResultMask::All as u32,
                })
                .collect();
            let results = session.browse(&descs, 0, None).await?;
            let mut out: Vec<Vec<ReferenceDescription>> = Vec::with_capacity(results.len());
            let mut pending: Vec<(usize, opcua::types::ByteString)> = Vec::new();
            for (i, r) in results.into_iter().enumerate() {
                check(r.status_code, || format!("browse {}", chunk[i]))?;
                out.push(r.references.unwrap_or_default());
                if !r.continuation_point.is_null() {
                    pending.push((i, r.continuation_point));
                }
            }
            while !pending.is_empty() {
                let points: Vec<_> = pending.iter().map(|(_, cp)| cp.clone()).collect();
                let results = session.browse_next(false, &points).await?;
                let mut next = Vec::new();
                for ((i, _), r) in pending.into_iter().zip(results) {
                    check(r.status_code, || format!("browse_next {}", chunk[i]))?;
                    out[i].extend(r.references.unwrap_or_default());
                    if !r.continuation_point.is_null() {
                        next.push((i, r.continuation_point));
                    }
                }
                pending = next;
            }
            Ok::<_, crate::Error>(out)
        }
    });
    Ok(try_join_all(requests)
        .await?
        .into_iter()
        .flatten()
        .collect())
}
