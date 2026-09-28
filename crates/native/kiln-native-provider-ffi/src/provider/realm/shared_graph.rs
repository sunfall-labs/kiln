//! Process-local leases for C facades over sealed user graphs (ADR 0073).
//!
//! C and message tokens exchange generation-checked graph IDs. A native GC facade owns one
//! integer lease; the table owns the strong graph reference and never exposes an Arc pointer.

use super::reclaim;
use kiln_shared_graph::{Handle, OpaqueHandle};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock, PoisonError};

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct SharedInfo {
    pub array: u32,
    pub null_prototype: u32,
    pub length: u32,
    pub field_count: u64,
}

#[derive(Default)]
struct Leases {
    next: u64,
    held: HashMap<u64, Handle>,
}

fn leases() -> &'static Mutex<Leases> {
    static LEASES: OnceLock<Mutex<Leases>> = OnceLock::new();
    LEASES.get_or_init(|| Mutex::new(Leases::default()))
}

fn insert(handle: Handle) -> u64 {
    let mut table = leases().lock().unwrap_or_else(PoisonError::into_inner);
    let Some(next) = table.next.checked_add(1) else {
        return 0;
    };
    table.next = next;
    table.held.insert(next, handle);
    next
}

/// Acquire a strong lease on a graph node by its process-local opaque ID. A stale generation or
/// a graph whose last owner was released returns zero. A queued message already owns a strong
/// handle independently and therefore keeps this lookup live until delivery or queue discard.
#[unsafe(no_mangle)]
pub extern "C" fn kiln_shared_graph_retain(graph: u32, generation: u32, node: u32) -> u64 {
    let token = OpaqueHandle {
        graph,
        generation,
        node,
    };
    Handle::resolve(token).map_or(0, insert)
}

/// Release the lease the native collector's finalizer held, after a scheduler grace period.
/// A duplicate or invalid release returns -1 and cannot affect another graph.
#[unsafe(no_mangle)]
pub extern "C" fn kiln_shared_graph_release(lease: u64) -> i32 {
    let held = leases()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .held
        .remove(&lease);
    let Some(handle) = held else {
        return -1;
    };
    reclaim::retire_shared(handle);
    0
}

/// Read the realm-neutral node header. No pointer into the arena crosses the ABI.
///
/// # Safety
///
/// `out` must point to a writable `SharedInfo` when non-null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_shared_graph_info(lease: u64, out: *mut SharedInfo) -> i32 {
    if out.is_null() {
        return -1;
    }
    let handle = leases()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .held
        .get(&lease)
        .cloned();
    let Some(handle) = handle else {
        return -1;
    };
    let Some((array, null_prototype, length, field_count)) =
        handle.graph().node_info(handle.index())
    else {
        return -1;
    };
    let Ok(field_count) = u64::try_from(field_count) else {
        return -1;
    };
    // SAFETY: caller's writable pointer was checked for null.
    unsafe {
        *out = SharedInfo {
            array: u32::from(array),
            null_prototype: u32::from(null_prototype),
            length,
            field_count,
        };
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use kiln_shared_graph::{InputValue, Inspector, Snapshot, SourceId, SourceNode};

    struct One;
    impl Inspector for One {
        type Error = ();
        fn inspect(&mut self, _: SourceId) -> Result<SourceNode, Self::Error> {
            Ok(SourceNode::Array {
                length: 3,
                properties: Vec::new(),
            })
        }
    }

    #[test]
    fn facade_lease_retains_graph_and_rejects_stale_generation() {
        let Snapshot::Shared(root) =
            kiln_shared_graph::share(InputValue::Object(1), &mut One).expect("array graph seals")
        else {
            panic!("an object must return a graph")
        };
        let opaque = root.opaque();
        let lease = kiln_shared_graph_retain(opaque.graph, opaque.generation, opaque.node);
        assert_ne!(lease, 0);
        drop(root);
        let mut info = SharedInfo::default();
        assert_eq!(unsafe { kiln_shared_graph_info(lease, &mut info) }, 0);
        assert_eq!((info.array, info.length, info.field_count), (1, 3, 0));
        assert_eq!(kiln_shared_graph_release(lease), 0);
        assert_eq!(kiln_shared_graph_release(lease), -1);
        assert_eq!(
            kiln_shared_graph_retain(opaque.graph, opaque.generation, opaque.node),
            0,
            "last release invalidates the token before any grace-period unmap"
        );
        let Snapshot::Shared(reused) =
            kiln_shared_graph::share(InputValue::Object(1), &mut One).expect("second graph seals")
        else {
            panic!("an object must return a graph")
        };
        if reused.opaque().graph == opaque.graph {
            assert_ne!(reused.opaque().generation, opaque.generation);
        }
    }
}
