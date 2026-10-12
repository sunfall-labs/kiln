//! Page-backed bytes for a published graph. Offsets are validated before every Rust read; a
//! native reader receives the same fixed-layout records and may never store into these pages.

use super::{EntryProbe, Graph, Handle, Node, NodeKind, OpaqueHandle, Value};
use kiln_string::StringUnits;
use std::mem::size_of;

pub const RECORD: u32 = 1;
pub const ARRAY: u32 = 2;
/// A `Map` or `Set` node has no own fields (`field_count` is zero, so every own-property reader
/// sees an empty record). `length` counts its entries, `fields` is the offset of its
/// `EntryRecord` table, and `index` is a SameValueZero hash index over the entry keys whose
/// slots hold `entry_slot`'s fingerprint and ordinal.
pub const MAP: u32 = 3;
pub const SET: u32 = 4;
pub const UNDEFINED: u32 = 0;
pub const NULL: u32 = 1;
pub const BOOLEAN: u32 = 2;
pub const NUMBER: u32 = 3;
pub const BIGINT: u32 = 4;
pub const STRING: u32 = 5;
pub const NODE: u32 = 6;
pub const SHARED: u32 = 7;
pub const LAYOUT_VERSION: u32 = 2;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct NodeRecord {
    pub kind: u32,
    pub null_prototype: u32,
    pub length: u64,
    pub fields: u64,
    pub field_count: u64,
    pub index: u64,
    pub index_capacity: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct FieldRecord {
    pub key_offset: u64,
    pub key_length: u64,
    pub kind: u32,
    pub enumerable: u32,
    pub payload: u64,
    pub value_length: u64,
}

/// One sealed collection entry. Each half uses `FieldRecord`'s value encoding, except that a
/// `SHARED` half also records the other graph's opaque identity: its length is that graph's ID
/// and its aux word the node. A native reader therefore compares keys and builds facades for
/// either half without resolving the external handle. A Set's value half is unused.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct EntryRecord {
    pub key_kind: u32,
    pub value_kind: u32,
    pub key_payload: u64,
    pub key_length: u64,
    pub key_aux: u64,
    pub value_payload: u64,
    pub value_length: u64,
    pub value_aux: u64,
}

const _: () = {
    assert!(size_of::<NodeRecord>() == 48);
    assert!(std::mem::offset_of!(NodeRecord, index_capacity) == 40);
    assert!(size_of::<FieldRecord>() == 40);
    assert!(std::mem::offset_of!(FieldRecord, value_length) == 32);
    assert!(size_of::<EntryRecord>() == 56);
    assert!(std::mem::offset_of!(EntryRecord, value_aux) == 48);
};

/// A collection holds at most this many entries before its keys get a hash index; below it, a
/// scan of the contiguous entry table is cheaper than hashing.
const ENTRY_INDEX_THRESHOLD: usize = 8;

fn key_hash(units: impl IntoIterator<Item = u16>) -> u64 {
    units
        .into_iter()
        .fold(0xcbf2_9ce4_8422_2325u64, |hash, unit| {
            (hash ^ u64::from(unit)).wrapping_mul(0x100_0000_01b3)
        })
}

/// The 64-bit finalizer of SplitMix64. The C reader repeats it; both must agree bit for bit.
const fn mix(mut x: u64) -> u64 {
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

/// SameValueZero's identity for a number: `-0` is `+0`, and every NaN is one value.
pub fn same_value_zero_bits(number: f64) -> u64 {
    if number == 0.0 {
        0
    } else if number.is_nan() {
        0x7ff8_0000_0000_0000
    } else {
        number.to_bits()
    }
}

const HASH_UNDEFINED: u64 = 1;
const HASH_NULL: u64 = 2;
const HASH_FALSE: u64 = 3;
const HASH_TRUE: u64 = 4;
const HASH_NUMBER: u64 = 0x6e75_6d62_6572;
const HASH_BIGINT: u64 = 0x6269_6769_6e74;
const HASH_NODE: u64 = 0x6e6f_6465;
const HASH_EXTERNAL: u64 = 0x6578_7465_726e;

/// The SameValueZero hash of a sealed collection key. Strings use the field index's UTF-16 FNV;
/// a BigInt hashes its canonical decimal text; an object key is its node in this graph, or the
/// opaque identity of a node in another sealed graph.
fn entry_hash(probe: EntryProbe<'_>, local: impl Fn(OpaqueHandle) -> Option<u32>) -> u64 {
    match probe {
        EntryProbe::Undefined => mix(HASH_UNDEFINED),
        EntryProbe::Null => mix(HASH_NULL),
        EntryProbe::Boolean(flag) => mix(if flag { HASH_TRUE } else { HASH_FALSE }),
        EntryProbe::Number(number) => mix(same_value_zero_bits(number) ^ HASH_NUMBER),
        EntryProbe::String(units) => key_hash(units.iter().copied()),
        EntryProbe::BigInt(text) => mix(key_hash(text.bytes().map(u16::from)) ^ HASH_BIGINT),
        EntryProbe::Object(token) => match local(token) {
            Some(node) => mix(u64::from(node) ^ (HASH_NODE << 32)),
            None => external_hash(
                (u64::from(token.generation) << 32) | u64::from(token.graph),
                token.node,
            ),
        },
    }
}

/// An entry index slot: the key hash's high half beside the one-based entry ordinal, so a probe
/// that lands on another key's slot moves on without reading that entry. Zero is empty.
fn entry_slot(hash: u64, ordinal: u32) -> u64 {
    (hash & 0xffff_ffff_0000_0000) | u64::from(ordinal)
}

fn external_hash(graph: u64, node: u32) -> u64 {
    mix(mix(graph) ^ u64::from(node) ^ (HASH_EXTERNAL << 16))
}

pub struct SealedArena {
    pointer: *mut u8,
    used: usize,
    mapped: usize,
    #[cfg(not(unix))]
    backing: Box<[u8]>,
}

// SAFETY: publication happens after mprotect; all readers use immutable methods and the region
// is unmapped only after its owning Arc and any external scheduler grace period have ended.
unsafe impl Send for SealedArena {}
unsafe impl Sync for SealedArena {}

impl std::fmt::Debug for SealedArena {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.debug_struct("SealedArena")
            .field("used", &self.used)
            .field("mapped", &self.mapped)
            .finish()
    }
}

impl SealedArena {
    fn new(bytes: &[u8]) -> Result<Self, ()> {
        #[cfg(unix)]
        {
            let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
            let page = usize::try_from(page).map_err(|_| ())?;
            if page == 0 || !page.is_power_of_two() {
                return Err(());
            }
            let mapped = bytes.len().max(1).checked_add(page - 1).ok_or(())? & !(page - 1);
            // SAFETY: a new private, anonymous mapping is owned by this allocation alone.
            let pointer = unsafe {
                libc::mmap(
                    std::ptr::null_mut(),
                    mapped,
                    libc::PROT_READ | libc::PROT_WRITE,
                    libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                    -1,
                    0,
                )
            };
            if pointer == libc::MAP_FAILED {
                return Err(());
            }
            // SAFETY: the mapping is at least `bytes.len()` bytes, and no reader can see it yet.
            unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), pointer.cast(), bytes.len()) };
            // SAFETY: this exact mapping is still private and its full page-aligned length is used.
            if unsafe { libc::mprotect(pointer, mapped, libc::PROT_READ) } != 0 {
                unsafe { libc::munmap(pointer, mapped) };
                return Err(());
            }
            Ok(Self {
                pointer: pointer.cast(),
                used: bytes.len(),
                mapped,
            })
        }
        #[cfg(not(unix))]
        {
            let mut backing = bytes.to_vec().into_boxed_slice();
            let pointer = backing.as_mut_ptr();
            Ok(Self {
                pointer,
                used: bytes.len(),
                mapped: backing.len(),
                backing,
            })
        }
    }

    pub fn as_ptr(&self) -> *const u8 {
        self.pointer
    }

    pub fn used(&self) -> usize {
        self.used
    }

    fn range(&self, offset: u64, len: usize) -> Option<&[u8]> {
        let start = usize::try_from(offset).ok()?;
        let end = start.checked_add(len)?;
        if end > self.used {
            return None;
        }
        // SAFETY: bounds were checked and the mapping is read-only for this arena's life.
        Some(unsafe { std::slice::from_raw_parts(self.pointer.add(start), len) })
    }

    pub fn read<T: Copy>(&self, offset: u64) -> Option<T> {
        let bytes = self.range(offset, size_of::<T>())?;
        // SAFETY: a copy is read from validated bytes; alignment is intentionally not assumed.
        Some(unsafe { std::ptr::read_unaligned(bytes.as_ptr().cast::<T>()) })
    }

    fn units(&self, offset: u64, length: u64) -> Option<StringUnits> {
        let count = usize::try_from(length).ok()?;
        let bytes = self.range(offset, count.checked_mul(2)?)?;
        let units: Vec<u16> = bytes
            .chunks_exact(2)
            .map(|pair| u16::from_ne_bytes([pair[0], pair[1]]))
            .collect();
        Some(StringUnits::from_units(&units))
    }

    fn units_equal(&self, offset: u64, length: u64, wanted: &StringUnits) -> bool {
        if length != wanted.len() as u64 {
            return false;
        }
        let Some(bytes) = usize::try_from(length)
            .ok()
            .and_then(|count| count.checked_mul(2))
            .and_then(|size| self.range(offset, size))
        else {
            return false;
        };
        bytes
            .chunks_exact(2)
            .map(|pair| u16::from_ne_bytes([pair[0], pair[1]]))
            .eq(wanted.units())
    }

    fn units_equal_slice(&self, offset: u64, length: u64, wanted: &[u16]) -> bool {
        if length != wanted.len() as u64 {
            return false;
        }
        let Some(bytes) = wanted
            .len()
            .checked_mul(2)
            .and_then(|size| self.range(offset, size))
        else {
            return false;
        };
        bytes
            .chunks_exact(2)
            .map(|pair| u16::from_ne_bytes([pair[0], pair[1]]))
            .eq(wanted.iter().copied())
    }

    fn text(&self, offset: u64, length: u64) -> Option<String> {
        let bytes = self.range(offset, usize::try_from(length).ok()?)?;
        String::from_utf8(bytes.to_vec()).ok()
    }
}

impl Drop for SealedArena {
    fn drop(&mut self) {
        #[cfg(unix)]
        // SAFETY: this arena alone owns this mapping and cannot drop until its last Arc goes.
        unsafe {
            libc::munmap(self.pointer.cast(), self.mapped);
        }
    }
}

struct Writer {
    bytes: Vec<u8>,
    externals: Vec<Handle>,
}

impl Writer {
    fn new(node_count: usize) -> Result<Self, ()> {
        let header_bytes = node_count.checked_mul(size_of::<NodeRecord>()).ok_or(())?;
        let mut bytes = Vec::new();
        bytes.try_reserve_exact(header_bytes).map_err(|_| ())?;
        bytes.resize(header_bytes, 0);
        Ok(Self {
            bytes,
            externals: Vec::new(),
        })
    }

    fn align(&mut self, alignment: usize) -> Result<(), ()> {
        let extra = alignment - 1;
        let end = self.bytes.len().checked_add(extra).ok_or(())? & !extra;
        self.bytes.resize(end, 0);
        Ok(())
    }

    fn reserve(&mut self, count: usize) -> Result<u64, ()> {
        self.align(8)?;
        let at = self.bytes.len();
        let end = at.checked_add(count).ok_or(())?;
        self.bytes.try_reserve(count).map_err(|_| ())?;
        self.bytes.resize(end, 0);
        u64::try_from(at).map_err(|_| ())
    }

    fn put<T: Copy>(&mut self, at: u64, value: T) {
        let at = at as usize;
        // SAFETY: callers reserve a correctly sized slot before writing; data is still private.
        unsafe { std::ptr::write_unaligned(self.bytes.as_mut_ptr().add(at).cast::<T>(), value) };
    }

    fn units(&mut self, units: &StringUnits) -> Result<(u64, u64), ()> {
        let length = u64::try_from(units.len()).map_err(|_| ())?;
        let at = self.reserve(units.len().checked_mul(2).ok_or(())?)?;
        for (index, unit) in units.units().enumerate() {
            let start = at as usize + index * 2;
            self.bytes[start..start + 2].copy_from_slice(&unit.to_ne_bytes());
        }
        Ok((at, length))
    }

    fn text(&mut self, text: &str) -> Result<(u64, u64), ()> {
        let at = self.reserve(text.len())?;
        let start = at as usize;
        self.bytes[start..start + text.len()].copy_from_slice(text.as_bytes());
        Ok((at, u64::try_from(text.len()).map_err(|_| ())?))
    }

    fn field(&mut self, value: &Value) -> Result<(u32, u64, u64), ()> {
        Ok(match value {
            Value::Undefined => (UNDEFINED, 0, 0),
            Value::Null => (NULL, 0, 0),
            Value::Boolean(flag) => (BOOLEAN, u64::from(*flag), 0),
            Value::Number(number) => (NUMBER, number.to_bits(), 0),
            Value::BigInt(number) => {
                let (at, length) = self.text(number)?;
                (BIGINT, at, length)
            }
            Value::String(text) => {
                let (at, length) = self.units(text)?;
                (STRING, at, length)
            }
            Value::Node(index) => (NODE, u64::from(*index), 0),
            Value::Shared(handle) => {
                let index = u64::try_from(self.externals.len()).map_err(|_| ())?;
                self.externals.push(handle.clone());
                (SHARED, index, 0)
            }
        })
    }
}

impl Writer {
    /// The hash a collection key is indexed under, computed from the value the writer seals.
    fn entry_hash(value: &Value) -> u64 {
        let units;
        let probe = match value {
            Value::Undefined => EntryProbe::Undefined,
            Value::Null => EntryProbe::Null,
            Value::Boolean(flag) => EntryProbe::Boolean(*flag),
            Value::Number(number) => EntryProbe::Number(*number),
            Value::BigInt(text) => EntryProbe::BigInt(text),
            Value::String(text) => {
                units = text.to_units();
                EntryProbe::String(&units)
            }
            Value::Node(node) => return mix(u64::from(*node) ^ (HASH_NODE << 32)),
            Value::Shared(handle) => return external_hash(handle.graph().id(), handle.index()),
        };
        entry_hash(probe, |_| None)
    }

    /// One collection: its entry table, then its key index.
    fn collection(
        &mut self,
        index: usize,
        kind: u32,
        entries: &[(&Value, Option<&Value>)],
    ) -> Result<(), ()> {
        let table = self.reserve(
            entries
                .len()
                .checked_mul(size_of::<EntryRecord>())
                .ok_or(())?,
        )?;
        let index_capacity = if entries.len() >= ENTRY_INDEX_THRESHOLD {
            entries
                .len()
                .checked_mul(2)
                .and_then(usize::checked_next_power_of_two)
                .ok_or(())?
        } else {
            0
        };
        let index_offset = self.reserve(index_capacity.checked_mul(size_of::<u64>()).ok_or(())?)?;
        for (ordinal, (key, _)) in entries.iter().enumerate() {
            if index_capacity == 0 {
                break;
            }
            let hash = Self::entry_hash(key);
            let ordinal = u32::try_from(ordinal + 1).map_err(|_| ())?;
            let mut slot = hash as usize & (index_capacity - 1);
            loop {
                let at = index_offset + (slot * size_of::<u64>()) as u64;
                if self.read_u64(at) == 0 {
                    self.put(at, entry_slot(hash, ordinal));
                    break;
                }
                slot = (slot + 1) & (index_capacity - 1);
            }
        }
        self.put(
            u64::try_from(index.checked_mul(size_of::<NodeRecord>()).ok_or(())?).map_err(|_| ())?,
            NodeRecord {
                kind,
                null_prototype: 0,
                length: u64::try_from(entries.len()).map_err(|_| ())?,
                fields: table,
                field_count: 0,
                index: index_offset,
                index_capacity: u64::try_from(index_capacity).map_err(|_| ())?,
            },
        );
        for (ordinal, (key, value)) in entries.iter().enumerate() {
            let (key_kind, key_payload, key_length, key_aux) = self.entry_half(key)?;
            let (value_kind, value_payload, value_length, value_aux) = match value {
                Some(value) => self.entry_half(value)?,
                None => (UNDEFINED, 0, 0, 0),
            };
            self.put(
                table + (ordinal * size_of::<EntryRecord>()) as u64,
                EntryRecord {
                    key_kind,
                    value_kind,
                    key_payload,
                    key_length,
                    key_aux,
                    value_payload,
                    value_length,
                    value_aux,
                },
            );
        }
        Ok(())
    }

    /// `field`'s encoding plus, for a `SHARED` value, the external node's opaque identity.
    fn entry_half(&mut self, value: &Value) -> Result<(u32, u64, u64, u64), ()> {
        let (kind, payload, length) = self.field(value)?;
        Ok(match value {
            Value::Shared(handle) => (
                kind,
                payload,
                handle.graph().id(),
                u64::from(handle.index()),
            ),
            _ => (kind, payload, length, 0),
        })
    }

    fn read_u64(&self, at: u64) -> u64 {
        let at = at as usize;
        u64::from_ne_bytes(
            self.bytes[at..at + 8]
                .try_into()
                .expect("eight reserved bytes"),
        )
    }
}

pub(super) fn seal(nodes: &[Node]) -> Result<(SealedArena, Vec<Handle>), ()> {
    let mut writer = Writer::new(nodes.len())?;
    for (index, node) in nodes.iter().enumerate() {
        let (kind, null_prototype, length, fields) = match node {
            Node::Record {
                null_prototype,
                fields,
            } => (RECORD, u32::from(*null_prototype), 0, fields),
            Node::Array { length, fields } => (ARRAY, 0, u64::from(*length), fields),
            Node::Map { entries } => {
                let pairs: Vec<_> = entries
                    .iter()
                    .map(|(key, value)| (key, Some(value)))
                    .collect();
                writer.collection(index, MAP, &pairs)?;
                continue;
            }
            Node::Set { values } => {
                let keys: Vec<_> = values.iter().map(|value| (value, None)).collect();
                writer.collection(index, SET, &keys)?;
                continue;
            }
        };
        let table = writer.reserve(
            fields
                .len()
                .checked_mul(size_of::<FieldRecord>())
                .ok_or(())?,
        )?;
        // Small records are cheaper to scan. Larger records and arrays get an immutable open
        // addressed index in the same protected mapping; no reader learns or writes a cache.
        let mut index_slots = Vec::new();
        let index_capacity = if fields.len() >= 16 {
            fields
                .len()
                .checked_mul(2)
                .and_then(usize::checked_next_power_of_two)
                .ok_or(())?
        } else {
            0
        };
        if index_capacity != 0 {
            index_slots
                .try_reserve_exact(index_capacity)
                .map_err(|_| ())?;
            index_slots.resize(index_capacity, 0u64);
            for (field_index, field) in fields.iter().enumerate() {
                let mut slot = key_hash(field.key.units()) as usize & (index_capacity - 1);
                while index_slots[slot] != 0 {
                    slot = (slot + 1) & (index_capacity - 1);
                }
                index_slots[slot] = u64::try_from(field_index + 1).map_err(|_| ())?;
            }
        }
        let index_offset =
            writer.reserve(index_capacity.checked_mul(size_of::<u64>()).ok_or(())?)?;
        for (slot, value) in index_slots.into_iter().enumerate() {
            let offset = index_offset
                .checked_add(
                    u64::try_from(slot.checked_mul(size_of::<u64>()).ok_or(())?).map_err(|_| ())?,
                )
                .ok_or(())?;
            writer.put(offset, value);
        }
        writer.put(
            u64::try_from(index.checked_mul(size_of::<NodeRecord>()).ok_or(())?).map_err(|_| ())?,
            NodeRecord {
                kind,
                null_prototype,
                length,
                fields: table,
                field_count: u64::try_from(fields.len()).map_err(|_| ())?,
                index: index_offset,
                index_capacity: u64::try_from(index_capacity).map_err(|_| ())?,
            },
        );
        for (field_index, field) in fields.iter().enumerate() {
            let (key_offset, key_length) = writer.units(&field.key)?;
            let (kind, payload, value_length) = writer.field(&field.value)?;
            let slot = table
                .checked_add(
                    u64::try_from(
                        field_index
                            .checked_mul(size_of::<FieldRecord>())
                            .ok_or(())?,
                    )
                    .map_err(|_| ())?,
                )
                .ok_or(())?;
            writer.put(
                slot,
                FieldRecord {
                    key_offset,
                    key_length,
                    kind,
                    enumerable: u32::from(field.enumerable),
                    payload,
                    value_length,
                },
            );
        }
    }
    let arena = SealedArena::new(&writer.bytes)?;
    Ok((arena, writer.externals))
}

impl Graph {
    fn record(&self, index: u32) -> Option<NodeRecord> {
        if index >= self.node_count {
            return None;
        }
        let offset = usize::try_from(index)
            .ok()?
            .checked_mul(size_of::<NodeRecord>())?;
        self.arena.read(u64::try_from(offset).ok()?)
    }

    pub fn node_kind(&self, index: u32) -> Option<NodeKind> {
        let record = self.record(index)?;
        Some(match record.kind {
            RECORD => NodeKind::Record {
                null_prototype: record.null_prototype != 0,
            },
            ARRAY => NodeKind::Array {
                length: u32::try_from(record.length).ok()?,
            },
            MAP => NodeKind::Map,
            SET => NodeKind::Set,
            _ => return None,
        })
    }

    /// `(array, null_prototype, length, own field count)`. A collection reads as an ordinary
    /// record with no own fields, which is what every own-property question asks of one.
    pub fn node_info(&self, index: u32) -> Option<(bool, bool, u32, usize)> {
        let record = self.record(index)?;
        let array = match record.kind {
            ARRAY => true,
            RECORD => false,
            MAP | SET => return Some((false, false, 0, 0)),
            _ => return None,
        };
        Some((
            array,
            record.null_prototype != 0,
            u32::try_from(record.length).ok()?,
            usize::try_from(record.field_count).ok()?,
        ))
    }

    fn raw_field(&self, node: u32, index: usize) -> Option<FieldRecord> {
        let record = self.record(node)?;
        if index >= usize::try_from(record.field_count).ok()? {
            return None;
        }
        let offset = record
            .fields
            .checked_add(u64::try_from(index.checked_mul(size_of::<FieldRecord>())?).ok()?)?;
        self.arena.read(offset)
    }

    pub fn field(&self, node: u32, index: usize) -> Option<super::Field> {
        let field = self.raw_field(node, index)?;
        let key = self.arena.units(field.key_offset, field.key_length)?;
        Some(super::Field {
            key,
            value: self.value(field.kind, field.payload, field.value_length)?,
            enumerable: field.enumerable != 0,
        })
    }

    fn value(&self, kind: u32, payload: u64, length: u64) -> Option<Value> {
        Some(match kind {
            UNDEFINED => Value::Undefined,
            NULL => Value::Null,
            BOOLEAN => Value::Boolean(payload != 0),
            NUMBER => Value::Number(f64::from_bits(payload)),
            BIGINT => Value::BigInt(self.arena.text(payload, length)?),
            STRING => Value::String(self.arena.units(payload, length)?),
            NODE => Value::Node(u32::try_from(payload).ok()?),
            SHARED => Value::Shared(self.externals.get(usize::try_from(payload).ok()?)?.clone()),
            _ => return None,
        })
    }

    /// The collection node's record, or `None` for a record, an array or a bad index.
    fn collection(&self, node: u32) -> Option<NodeRecord> {
        self.record(node)
            .filter(|record| matches!(record.kind, MAP | SET))
    }

    /// How many entries a `Map` or `Set` node holds.
    pub fn collection_len(&self, node: u32) -> Option<usize> {
        usize::try_from(self.collection(node)?.length).ok()
    }

    fn raw_entry(&self, record: &NodeRecord, index: usize) -> Option<EntryRecord> {
        if index >= usize::try_from(record.length).ok()? {
            return None;
        }
        let offset = record
            .fields
            .checked_add(u64::try_from(index.checked_mul(size_of::<EntryRecord>())?).ok()?)?;
        self.arena.read(offset)
    }

    /// Entry `index` of a `Map` (key and value) or `Set` (the value twice, as `Set` iteration
    /// reports it), in insertion order.
    pub fn entry(&self, node: u32, index: usize) -> Option<(Value, Value)> {
        let record = self.collection(node)?;
        let entry = self.raw_entry(&record, index)?;
        let key_length = if entry.key_kind == SHARED {
            0
        } else {
            entry.key_length
        };
        let key = self.value(entry.key_kind, entry.key_payload, key_length)?;
        let value = if record.kind == SET {
            key.clone()
        } else {
            self.value(entry.value_kind, entry.value_payload, entry.value_length)?
        };
        Some((key, value))
    }

    /// The entry whose key is SameValueZero-equal to `probe`.
    pub fn find_entry(&self, node: u32, probe: EntryProbe<'_>) -> Option<usize> {
        let record = self.collection(node)?;
        let count = usize::try_from(record.length).ok()?;
        let local = |token: OpaqueHandle| self.owns(token).then_some(token.node);
        let matches = |entry: &EntryRecord| -> bool {
            match probe {
                EntryProbe::Undefined => entry.key_kind == UNDEFINED,
                EntryProbe::Null => entry.key_kind == NULL,
                EntryProbe::Boolean(flag) => {
                    entry.key_kind == BOOLEAN && (entry.key_payload != 0) == flag
                }
                EntryProbe::Number(number) => {
                    entry.key_kind == NUMBER
                        && same_value_zero_bits(f64::from_bits(entry.key_payload))
                            == same_value_zero_bits(number)
                }
                EntryProbe::String(units) => {
                    entry.key_kind == STRING
                        && self
                            .arena
                            .units_equal_slice(entry.key_payload, entry.key_length, units)
                }
                EntryProbe::BigInt(text) => {
                    entry.key_kind == BIGINT
                        && self
                            .arena
                            .text(entry.key_payload, entry.key_length)
                            .is_some_and(|stored| stored == text)
                }
                EntryProbe::Object(token) => match local(token) {
                    Some(node) => entry.key_kind == NODE && entry.key_payload == u64::from(node),
                    None => {
                        entry.key_kind == SHARED
                            && entry.key_length
                                == (u64::from(token.generation) << 32) | u64::from(token.graph)
                            && entry.key_aux == u64::from(token.node)
                    }
                },
            }
        };
        let capacity = usize::try_from(record.index_capacity).ok()?;
        if capacity == 0 {
            return (0..count).find(|&index| {
                self.raw_entry(&record, index)
                    .is_some_and(|entry| matches(&entry))
            });
        }
        if !capacity.is_power_of_two() || capacity < count.checked_mul(2)? {
            return None;
        }
        let hash = entry_hash(probe, local);
        let mut slot = hash as usize & (capacity - 1);
        for _ in 0..capacity {
            let offset = record
                .index
                .checked_add(u64::try_from(slot.checked_mul(size_of::<u64>())?).ok()?)?;
            let stored = self.arena.read::<u64>(offset)?;
            let ordinal = stored & 0xffff_ffff;
            if ordinal == 0 {
                return None;
            }
            if stored == entry_slot(hash, ordinal as u32) {
                let index = usize::try_from(ordinal - 1).ok()?;
                if matches(&self.raw_entry(&record, index)?) {
                    return Some(index);
                }
            }
            slot = (slot + 1) & (capacity - 1);
        }
        None
    }

    fn find_field_index_matching(
        &self,
        node: u32,
        hash: u64,
        mut matches: impl FnMut(&SealedArena, &FieldRecord) -> bool,
    ) -> Option<usize> {
        let record = self.record(node)?;
        let count = usize::try_from(record.field_count).ok()?;
        // A collection's index is over its entries; it has no own field to find.
        if count == 0 {
            return None;
        }
        let capacity = usize::try_from(record.index_capacity).ok()?;
        if capacity != 0 {
            if !capacity.is_power_of_two() || capacity < count.checked_mul(2)? {
                return None;
            }
            let mut slot = hash as usize & (capacity - 1);
            for _ in 0..capacity {
                let offset = record
                    .index
                    .checked_add(u64::try_from(slot.checked_mul(size_of::<u64>())?).ok()?)?;
                let entry = self.arena.read::<u64>(offset)?;
                if entry == 0 {
                    return None;
                }
                let index = usize::try_from(entry - 1).ok()?;
                let raw = self.raw_field(node, index)?;
                if matches(&self.arena, &raw) {
                    return Some(index);
                }
                slot = (slot + 1) & (capacity - 1);
            }
            return None;
        }
        for index in 0..count {
            let raw = self.raw_field(node, index)?;
            if matches(&self.arena, &raw) {
                return Some(index);
            }
        }
        None
    }

    pub fn find_field_index(&self, node: u32, key: &StringUnits) -> Option<usize> {
        self.find_field_index_matching(node, key_hash(key.units()), |arena, raw| {
            arena.units_equal(raw.key_offset, raw.key_length, key)
        })
    }

    pub fn find_field_index_units(&self, node: u32, key: &[u16]) -> Option<usize> {
        self.find_field_index_matching(node, key_hash(key.iter().copied()), |arena, raw| {
            arena.units_equal_slice(raw.key_offset, raw.key_length, key)
        })
    }

    pub fn find_field(&self, node: u32, key: &StringUnits) -> Option<super::Field> {
        self.field(node, self.find_field_index(node, key)?)
    }

    pub fn arena_ptr(&self) -> *const u8 {
        self.arena.as_ptr()
    }

    pub fn arena_len(&self) -> usize {
        self.arena.used()
    }

    pub fn node(&self, index: u32) -> Option<Node> {
        let record = self.record(index)?;
        if matches!(record.kind, MAP | SET) {
            let count = usize::try_from(record.length).ok()?;
            let entries = (0..count)
                .map(|at| self.entry(index, at))
                .collect::<Option<Vec<_>>>()?;
            return Some(if record.kind == MAP {
                Node::Map { entries }
            } else {
                Node::Set {
                    values: entries.into_iter().map(|(key, _)| key).collect(),
                }
            });
        }
        let count = usize::try_from(record.field_count).ok()?;
        let mut fields = Vec::with_capacity(count);
        for at in 0..count {
            fields.push(self.field(index, at)?);
        }
        match record.kind {
            RECORD => Some(Node::Record {
                null_prototype: record.null_prototype != 0,
                fields,
            }),
            ARRAY => Some(Node::Array {
                length: u32::try_from(record.length).ok()?,
                fields,
            }),
            _ => None,
        }
    }
}
