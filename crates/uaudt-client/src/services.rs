//! Batched service helpers: every helper sends as few requests as possible
//! (chunked to stay below typical server operation limits, chunks in flight
//! concurrently) and follows browse continuation points.

use futures::future::try_join_all;
use opcua::client::Session;
use opcua::types::{
    AttributeId, BrowseDescription, BrowseDirection, BrowseResultMask, DataValue, NodeClassMask,
    NodeId, NumericRange, QualifiedName, ReadValueId, ReferenceDescription, StatusCode,
    TimestampsToReturn, VariableId, Variant,
};

use crate::error::{Error, Result, check};

/// Max operations per request when the server announces no limit.
const CHUNK: usize = 500;

/// `true` when the server refused a request for carrying too many operations.
fn too_many_operations(e: &Error) -> bool {
    match e {
        Error::Opcua(e) => e.status() == StatusCode::BadTooManyOperations,
        Error::BadStatus { status, .. } => *status == StatusCode::BadTooManyOperations,
        _ => false,
    }
}

/// Server operation limit (`Server.ServerCapabilities.OperationLimits.*`),
/// `CHUNK` when unknown or 0 (no limit).
async fn operation_limit(session: &Session, limit: VariableId) -> usize {
    let id = ReadValueId {
        node_id: limit.into(),
        attribute_id: AttributeId::Value as u32,
        index_range: NumericRange::None,
        data_encoding: QualifiedName::null(),
    };
    let value = session
        .read(std::slice::from_ref(&id), TimestampsToReturn::Neither, 0.0)
        .await
        .ok()
        .and_then(|mut v| v.pop())
        .and_then(|dv| dv.value);
    match value {
        Some(Variant::UInt32(n)) if n > 0 => (n as usize).min(CHUNK),
        _ => CHUNK,
    }
}

/// Runs `op` with chunks of at most `start` operations; on `BadTooManyOperations`
/// (limit not announced, or announced wrongly) retries with smaller chunks.
async fn adaptive<T, F, Fut>(nodes: usize, start: usize, op: F) -> Result<T>
where
    F: Fn(usize) -> Fut,
    Fut: std::future::Future<Output = Result<T>>,
{
    let mut chunk = start.max(1);
    loop {
        match op(chunk).await {
            Err(e) if too_many_operations(&e) && chunk > 1 && nodes > 1 => {
                chunk = (chunk.min(nodes) / 4).max(1);
            }
            other => return other,
        }
    }
}

/// Reads one attribute of many nodes.
pub(crate) async fn read_attribute(
    session: &Session,
    nodes: &[NodeId],
    attribute: AttributeId,
) -> Result<Vec<DataValue>> {
    let start = if nodes.len() > 1 {
        operation_limit(session, VariableId::Server_ServerCapabilities_OperationLimits_MaxNodesPerRead).await
    } else {
        CHUNK
    };
    adaptive(nodes.len(), start, |chunk| read_chunked(session, nodes, attribute, chunk)).await
}

async fn read_chunked(
    session: &Session,
    nodes: &[NodeId],
    attribute: AttributeId,
    size: usize,
) -> Result<Vec<DataValue>> {
    let requests = nodes.chunks(size).map(|chunk| async move {
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
    let start = if nodes.len() > 1 {
        operation_limit(session, VariableId::Server_ServerCapabilities_OperationLimits_MaxNodesPerBrowse).await
    } else {
        CHUNK
    };
    adaptive(nodes.len(), start, |chunk| {
        browse_chunked(session, nodes, direction, reference_type.clone(), node_class_mask, chunk)
    })
    .await
}

async fn browse_chunked(
    session: &Session,
    nodes: &[NodeId],
    direction: BrowseDirection,
    reference_type: NodeId,
    node_class_mask: NodeClassMask,
    size: usize,
) -> Result<Vec<Vec<ReferenceDescription>>> {
    let requests = nodes.chunks(size).map(|chunk| {
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
