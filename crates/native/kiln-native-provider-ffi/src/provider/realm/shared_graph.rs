//! Process-local leases for C facades over sealed user graphs (ADR 0073).
//!
//! C and message tokens exchange generation-checked graph IDs. A native GC facade owns one
//! integer lease; the table owns the strong graph reference and never exposes an Arc pointer.

use super::reclaim;
use kiln_shared_graph::{Handle, OpaqueHandle};
use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::{Mutex, OnceLock, PoisonError};

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct SharedInfo {
    pub array: u32,
    pub null_prototype: u32,
    pub length: u32,
    pub field_count: u64,
}

/// A copied descriptor for one sealed own field. `bits` is a boolean, IEEE-754 number, or the
/// opaque child graph slot; `generation` and `node` complete a child handle. Text length is in
/// UTF-16 units. No pointer into a protected graph page crosses this boundary.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct SharedFieldInfo {
    pub kind: u32,
    pub enumerable: u32,
    pub key_length: u64,
    pub value_length: u64,
    pub bits: u64,
    pub generation: u32,
    pub node: u32,
}

/// A borrowed view of the sealed, realm-neutral arena. The facade's `lease` owns the graph for
/// the entire time native code may read this pointer. No pointer is valid after that lease is
/// released, and the arena contains no pointer into a realm heap.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct SharedArenaView {
    pub base: *const u8,
    pub length: u64,
    pub node_count: u32,
    pub layout_version: u32,
}

const _: () = {
    assert!(std::mem::size_of::<SharedArenaView>() == 24);
    assert!(std::mem::offset_of!(SharedArenaView, layout_version) == 20);
};

pub const FIELD_UNDEFINED: u32 = 0;
pub const FIELD_NULL: u32 = 1;
pub const FIELD_BOOLEAN: u32 = 2;
pub const FIELD_NUMBER: u32 = 3;
pub const FIELD_BIGINT: u32 = 4;
pub const FIELD_STRING: u32 = 5;
pub const FIELD_NODE: u32 = 6;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct SharedBuildResult {
    pub lease: u64,
    pub graph: u32,
    pub generation: u32,
    pub node: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct SharedSourceNode {
    pub kind: u32, // 1 ordinary record, 2 ordinary array
    pub null_prototype: u32,
    pub length: u32,
    pub field_count: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct SharedSourceField {
    pub key_kind: u32, // 0 string, 1 symbol, 2 decimal array index
    pub key_id: u64,
    pub key_length: u64,
    pub value_kind: u32, // 0..6 scalars; 7 ordinary node, 8 earlier graph
    pub value_bits: u64,
    pub value_length: u64,
    pub value_generation: u32,
    pub value_node: u32,
    pub enumerable: u32,
    pub accessor: u32,
}

pub type SourceInspect = unsafe extern "C" fn(
    context: *mut c_void,
    object: u64,
    field: u64,
    out_node: *mut SharedSourceNode,
    out_field: *mut SharedSourceField,
) -> i32;
pub type SourceUnit =
    unsafe extern "C" fn(context: *mut c_void, string: u64, index: u64, out: *mut u16) -> i32;

struct NativeInspector {
    context: *mut c_void,
    inspect: SourceInspect,
    unit: SourceUnit,
}

#[derive(Debug)]
enum NativeInspectError {
    InvalidSource,
    Unsupported,
    TooLarge,
}

impl std::fmt::Display for NativeInspectError {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "{self:?}")
    }
}

impl NativeInspector {
    fn units(
        &self,
        string: u64,
        length: u64,
    ) -> Result<kiln_string::StringUnits, NativeInspectError> {
        let length = usize::try_from(length).map_err(|_| NativeInspectError::TooLarge)?;
        let mut units = Vec::new();
        units
            .try_reserve_exact(length)
            .map_err(|_| NativeInspectError::TooLarge)?;
        for index in 0..length {
            let mut unit = 0;
            // SAFETY: the source callback runs synchronously while C owns a rooted source value.
            let status = unsafe { (self.unit)(self.context, string, index as u64, &mut unit) };
            if status != 0 {
                return Err(NativeInspectError::InvalidSource);
            }
            units.push(unit);
        }
        Ok(kiln_string::StringUnits::from_units(&units))
    }
}

impl kiln_shared_graph::Inspector for NativeInspector {
    type Error = NativeInspectError;

    fn inspect(&mut self, object: u64) -> Result<kiln_shared_graph::SourceNode, Self::Error> {
        use kiln_shared_graph::{Descriptor, InputValue, Key, Property, SourceNode};
        let mut node = SharedSourceNode::default();
        // `u64::MAX` asks for the kind and field count; a callback never invokes JS traps.
        let status = unsafe {
            (self.inspect)(
                self.context,
                object,
                u64::MAX,
                &mut node,
                std::ptr::null_mut(),
            )
        };
        if status != 0 {
            return Err(NativeInspectError::Unsupported);
        }
        if node.kind != 1 && node.kind != 2 {
            return Err(NativeInspectError::Unsupported);
        }
        let count = usize::try_from(node.field_count).map_err(|_| NativeInspectError::TooLarge)?;
        let mut properties = Vec::new();
        properties
            .try_reserve_exact(count)
            .map_err(|_| NativeInspectError::TooLarge)?;
        for index in 0..count {
            let mut field = SharedSourceField::default();
            let status = unsafe {
                (self.inspect)(
                    self.context,
                    object,
                    index as u64,
                    std::ptr::null_mut(),
                    &mut field,
                )
            };
            if status != 0 {
                return Err(NativeInspectError::InvalidSource);
            }
            let key = match field.key_kind {
                0 => Key::String(self.units(field.key_id, field.key_length)?),
                1 => Key::Symbol,
                2 => Key::String(field.key_id.to_string().into()),
                3 => continue, // a hole in a dense element vector
                _ => return Err(NativeInspectError::InvalidSource),
            };
            let descriptor = if field.accessor != 0 {
                Descriptor::Accessor
            } else {
                let value = match field.value_kind {
                    0 => InputValue::Undefined,
                    1 => InputValue::Null,
                    2 => InputValue::Boolean(field.value_bits != 0),
                    3 => InputValue::Number(f64::from_bits(field.value_bits)),
                    4 => {
                        #[cfg(feature = "bigint")]
                        {
                            let bigint = field.value_bits as *const num_bigint::BigInt;
                            if bigint.is_null() {
                                return Err(NativeInspectError::InvalidSource);
                            }
                            // SAFETY: C passes the provider's live immutable BigInt handle.
                            InputValue::BigInt(unsafe { &*bigint }.to_string())
                        }
                        #[cfg(not(feature = "bigint"))]
                        {
                            return Err(NativeInspectError::Unsupported);
                        }
                    }
                    5 => InputValue::String(self.units(field.value_bits, field.value_length)?),
                    7 => InputValue::Object(field.value_bits),
                    8 => {
                        let token = OpaqueHandle {
                            graph: u32::try_from(field.value_bits)
                                .map_err(|_| NativeInspectError::InvalidSource)?,
                            generation: field.value_generation,
                            node: field.value_node,
                        };
                        InputValue::Shared(
                            Handle::resolve(token).ok_or(NativeInspectError::InvalidSource)?,
                        )
                    }
                    _ => return Err(NativeInspectError::Unsupported),
                };
                Descriptor::Data(value)
            };
            properties.push(Property {
                key,
                descriptor,
                enumerable: field.enumerable != 0,
            });
        }
        Ok(if node.kind == 2 {
            SourceNode::Array {
                length: node.length,
                properties,
            }
        } else {
            SourceNode::Record {
                null_prototype: node.null_prototype != 0,
                properties,
            }
        })
    }
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

/// Expose fixed-layout, read-only bytes while the caller owns a strong lease. The C facade
/// keeps the lease until finalization; its property reader keeps that facade live across every
/// allocation. The pointer is never sent in a message or retained after the lease is dropped.
///
/// # Safety
///
/// `out` must point to a writable `SharedArenaView`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_shared_graph_view(lease: u64, out: *mut SharedArenaView) -> i32 {
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
    let graph = handle.graph();
    let Ok(length) = u64::try_from(graph.arena_len()) else {
        return -1;
    };
    let Ok(node_count) = u32::try_from(graph.len()) else {
        return -1;
    };
    // SAFETY: `out` was checked and the immutable arena is held by both this temporary handle
    // and the caller's lease. The caller must release the view before releasing that lease.
    unsafe {
        *out = SharedArenaView {
            base: graph.arena_ptr(),
            length,
            node_count,
            layout_version: kiln_shared_graph::arena::LAYOUT_VERSION,
        };
    }
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

/// Find an own string field without materializing its key or scanning across the C boundary.
/// Returns 0 with its ordinal, 1 when absent, and -1 for an invalid lease or argument.
/// The caller retains the lease while using the returned ordinal.
///
/// # Safety
///
/// `key` must point to `key_length` readable UTF-16 units when nonempty; `out` must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_shared_graph_find_field(
    lease: u64,
    key: *const u16,
    key_length: u64,
    out: *mut u64,
) -> i32 {
    if out.is_null() || (key.is_null() && key_length != 0) {
        return -1;
    }
    let Ok(length) = usize::try_from(key_length) else {
        return -1;
    };
    let units = if length == 0 {
        &[][..]
    } else {
        // SAFETY: the caller supplies a readable buffer of the checked length.
        unsafe { std::slice::from_raw_parts(key, length) }
    };
    let handle = leases()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .held
        .get(&lease)
        .cloned();
    let Some(handle) = handle else { return -1 };
    let Some(index) = handle.graph().find_field_index_units(handle.index(), units) else {
        return 1;
    };
    let Ok(index) = u64::try_from(index) else {
        return -1;
    };
    // SAFETY: the caller's writable output was checked above.
    unsafe { *out = index };
    0
}

/// Copy one field by index. A null buffer queries its required length; a non-null buffer must
/// have enough UTF-16 elements. The caller keeps the facade's lease for the whole call. Returns
/// -1 for an invalid lease/index or buffer, -2 for insufficient capacity, and zero on success.
///
/// # Safety
///
/// `out` must be writable. Non-null `key_out` and `value_out` must be writable for their stated
/// capacities. They must not overlap `out` or each other.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_shared_graph_field(
    lease: u64,
    index: u64,
    out: *mut SharedFieldInfo,
    key_out: *mut u16,
    key_capacity: u64,
    value_out: *mut u16,
    value_capacity: u64,
) -> i32 {
    use kiln_shared_graph::Value;
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
    let Ok(index) = usize::try_from(index) else {
        return -1;
    };
    let Some(field) = handle.graph().field(handle.index(), index) else {
        return -1;
    };
    let mut info = SharedFieldInfo {
        enumerable: u32::from(field.enumerable),
        key_length: field.key.len() as u64,
        ..SharedFieldInfo::default()
    };
    let text: Option<Vec<u16>> = match field.value {
        Value::Undefined => {
            info.kind = FIELD_UNDEFINED;
            None
        }
        Value::Null => {
            info.kind = FIELD_NULL;
            None
        }
        Value::Boolean(flag) => {
            info.kind = FIELD_BOOLEAN;
            info.bits = u64::from(flag);
            None
        }
        Value::Number(number) => {
            info.kind = FIELD_NUMBER;
            info.bits = number.to_bits();
            None
        }
        Value::BigInt(decimal) => {
            info.kind = FIELD_BIGINT;
            Some(decimal.encode_utf16().collect())
        }
        Value::String(text) => {
            info.kind = FIELD_STRING;
            Some(text.units().collect())
        }
        Value::Node(node) => {
            info.kind = FIELD_NODE;
            let Some(child) = handle.child(node) else {
                return -1;
            };
            let opaque = child.opaque();
            info.bits = u64::from(opaque.graph);
            info.generation = opaque.generation;
            info.node = opaque.node;
            None
        }
        Value::Shared(child) => {
            info.kind = FIELD_NODE;
            let opaque = child.opaque();
            info.bits = u64::from(opaque.graph);
            info.generation = opaque.generation;
            info.node = opaque.node;
            None
        }
    };
    info.value_length = text.as_ref().map_or(0, |units| units.len() as u64);
    // SAFETY: the caller owns `out`; all derived lengths and capacities were checked before
    // reading or writing either optional output slice.
    unsafe { *out = info };
    if (!key_out.is_null() && key_capacity < info.key_length)
        || (!value_out.is_null() && value_capacity < info.value_length)
    {
        return -2;
    }
    if !key_out.is_null() {
        let Ok(length) = usize::try_from(info.key_length) else {
            return -1;
        };
        for (at, unit) in field.key.units().enumerate().take(length) {
            unsafe { *key_out.add(at) = unit };
        }
    }
    if let Some(text) = text
        && !value_out.is_null()
    {
        for (at, unit) in text.into_iter().enumerate() {
            unsafe { *value_out.add(at) = unit };
        }
    }
    0
}

/// Seal a native object graph by synchronously inspecting C's own storage. The returned build
/// lease keeps the graph alive until C installs a realm-local facade and releases this lease.
/// An inspector refusal returns -2; allocation/size failure returns -3; bad ABI arguments -1.
///
/// # Safety
///
/// `out` must be writable and both callbacks must uphold their pointer/output contracts. The
/// source graph must remain rooted and unmutated until this call returns; callbacks may read
/// descriptors directly but must not execute JavaScript or trigger a native collection.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_shared_graph_build(
    root: u64,
    inspect: Option<SourceInspect>,
    unit: Option<SourceUnit>,
    context: *mut c_void,
    out: *mut SharedBuildResult,
) -> i32 {
    let (Some(inspect), Some(unit)) = (inspect, unit) else {
        return -1;
    };
    if root == 0 || out.is_null() {
        return -1;
    }
    let mut source = NativeInspector {
        context,
        inspect,
        unit,
    };
    let handle =
        match kiln_shared_graph::share(kiln_shared_graph::InputValue::Object(root), &mut source) {
            Ok(kiln_shared_graph::Snapshot::Shared(handle)) => handle,
            Ok(kiln_shared_graph::Snapshot::Primitive(_)) => return -1,
            Err(
                kiln_shared_graph::SnapshotError::TooLarge
                | kiln_shared_graph::SnapshotError::Source(NativeInspectError::TooLarge),
            ) => return -3,
            Err(_) => return -2,
        };
    let opaque = handle.opaque();
    let lease = insert(handle);
    if lease == 0 {
        return -3;
    }
    unsafe {
        *out = SharedBuildResult {
            lease,
            graph: opaque.graph,
            generation: opaque.generation,
            node: opaque.node,
        };
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use kiln_shared_graph::{
        Descriptor, InputValue, Inspector, Key, Property, Snapshot, SourceId, SourceNode,
    };

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

    #[test]
    fn field_reads_copy_units_and_child_ids_without_exposing_arena_pointers() {
        struct Record;
        impl Inspector for Record {
            type Error = ();
            fn inspect(&mut self, _: SourceId) -> Result<SourceNode, Self::Error> {
                Ok(SourceNode::Record {
                    null_prototype: false,
                    properties: vec![
                        Property {
                            key: Key::String("wide".into()),
                            descriptor: Descriptor::Data(InputValue::String("𝄞".into())),
                            enumerable: false,
                        },
                        Property {
                            key: Key::String("self".into()),
                            descriptor: Descriptor::Data(InputValue::Object(1)),
                            enumerable: true,
                        },
                    ],
                })
            }
        }
        let Snapshot::Shared(root) =
            kiln_shared_graph::share(InputValue::Object(1), &mut Record).unwrap()
        else {
            panic!("record must seal")
        };
        let opaque = root.opaque();
        let lease = kiln_shared_graph_retain(opaque.graph, opaque.generation, opaque.node);
        let mut info = SharedFieldInfo::default();
        assert_eq!(
            unsafe {
                kiln_shared_graph_field(
                    lease,
                    0,
                    &mut info,
                    std::ptr::null_mut(),
                    0,
                    std::ptr::null_mut(),
                    0,
                )
            },
            0
        );
        assert_eq!(
            (
                info.kind,
                info.enumerable,
                info.key_length,
                info.value_length
            ),
            (FIELD_STRING, 0, 4, 2)
        );
        let mut key = [0u16; 4];
        let mut value = [0u16; 2];
        assert_eq!(
            unsafe {
                kiln_shared_graph_field(
                    lease,
                    0,
                    &mut info,
                    key.as_mut_ptr(),
                    key.len() as u64,
                    value.as_mut_ptr(),
                    value.len() as u64,
                )
            },
            0
        );
        assert_eq!(String::from_utf16(&key).unwrap(), "wide");
        assert_eq!(String::from_utf16(&value).unwrap(), "𝄞");
        assert_eq!(
            unsafe {
                kiln_shared_graph_field(
                    lease,
                    0,
                    &mut info,
                    key.as_mut_ptr(),
                    3,
                    value.as_mut_ptr(),
                    2,
                )
            },
            -2
        );
        assert_eq!(
            unsafe {
                kiln_shared_graph_field(
                    lease,
                    1,
                    &mut info,
                    std::ptr::null_mut(),
                    0,
                    std::ptr::null_mut(),
                    0,
                )
            },
            0
        );
        assert_eq!(info.kind, FIELD_NODE);
        assert_eq!(
            (info.bits as u32, info.generation, info.node),
            (opaque.graph, opaque.generation, opaque.node)
        );
        assert_eq!(kiln_shared_graph_release(lease), 0);
    }

    #[test]
    fn native_inspector_skips_holes_and_holds_a_build_lease_until_facade_install() {
        unsafe extern "C" fn inspect(
            _: *mut c_void,
            object: u64,
            index: u64,
            node: *mut SharedSourceNode,
            field: *mut SharedSourceField,
        ) -> i32 {
            if object != 1 {
                return -1;
            }
            if index == u64::MAX {
                unsafe {
                    *node = SharedSourceNode {
                        kind: 2,
                        length: 3,
                        field_count: 3,
                        ..Default::default()
                    };
                }
                return 0;
            }
            let answer = match index {
                0 => SharedSourceField {
                    key_kind: 2,
                    key_id: 0,
                    value_kind: 3,
                    value_bits: 7.0f64.to_bits(),
                    enumerable: 1,
                    ..Default::default()
                },
                1 => SharedSourceField {
                    key_kind: 3,
                    ..Default::default()
                },
                2 => SharedSourceField {
                    key_kind: 2,
                    key_id: 2,
                    value_kind: 5,
                    value_bits: 11,
                    value_length: 2,
                    enumerable: 1,
                    ..Default::default()
                },
                _ => return -1,
            };
            unsafe { *field = answer };
            0
        }
        unsafe extern "C" fn unit(_: *mut c_void, string: u64, index: u64, out: *mut u16) -> i32 {
            if string != 11 || index >= 2 {
                return -1;
            }
            unsafe { *out = [b'o' as u16, b'k' as u16][index as usize] };
            0
        }
        let mut built = SharedBuildResult::default();
        assert_eq!(
            unsafe {
                kiln_shared_graph_build(
                    1,
                    Some(inspect),
                    Some(unit),
                    std::ptr::null_mut(),
                    &mut built,
                )
            },
            0
        );
        assert_ne!(built.lease, 0);
        let token = OpaqueHandle {
            graph: built.graph,
            generation: built.generation,
            node: built.node,
        };
        let facade_lease = kiln_shared_graph_retain(token.graph, token.generation, token.node);
        assert_ne!(facade_lease, 0);
        assert_eq!(kiln_shared_graph_release(built.lease), 0);
        let mut info = SharedInfo::default();
        assert_eq!(
            unsafe { kiln_shared_graph_info(facade_lease, &mut info) },
            0
        );
        assert_eq!((info.array, info.length, info.field_count), (1, 3, 2));
        assert_eq!(kiln_shared_graph_release(facade_lease), 0);
        // Other concurrently running realm tests can hold the reclamation epoch open. The
        // released lease must disappear at once; the graph ID may remain resolvable until the
        // deferred strong handle reaches its turn-boundary grace period.
        assert_eq!(
            unsafe { kiln_shared_graph_info(facade_lease, &mut info) },
            -1
        );
    }
}
