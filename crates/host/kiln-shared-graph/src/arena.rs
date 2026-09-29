//! Page-backed bytes for a published graph. Offsets are validated before every Rust read; a
//! native reader receives the same fixed-layout records and may never store into these pages.

use super::{Graph, Handle, Node, Value};
use kiln_string::StringUnits;
use std::mem::size_of;

pub const RECORD: u32 = 1;
pub const ARRAY: u32 = 2;
pub const UNDEFINED: u32 = 0;
pub const NULL: u32 = 1;
pub const BOOLEAN: u32 = 2;
pub const NUMBER: u32 = 3;
pub const BIGINT: u32 = 4;
pub const STRING: u32 = 5;
pub const NODE: u32 = 6;
pub const SHARED: u32 = 7;

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

fn key_hash(units: impl IntoIterator<Item = u16>) -> u64 {
    units
        .into_iter()
        .fold(0xcbf2_9ce4_8422_2325u64, |hash, unit| {
            (hash ^ u64::from(unit)).wrapping_mul(0x100_0000_01b3)
        })
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

pub(super) fn seal(nodes: &[Node]) -> Result<(SealedArena, Vec<Handle>), ()> {
    let mut writer = Writer::new(nodes.len())?;
    for (index, node) in nodes.iter().enumerate() {
        let (kind, null_prototype, length, fields) = match node {
            Node::Record {
                null_prototype,
                fields,
            } => (RECORD, u32::from(*null_prototype), 0, fields),
            Node::Array { length, fields } => (ARRAY, 0, u64::from(*length), fields),
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

    pub fn node_info(&self, index: u32) -> Option<(bool, bool, u32, usize)> {
        let record = self.record(index)?;
        let array = match record.kind {
            ARRAY => true,
            RECORD => false,
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
        let value = match field.kind {
            UNDEFINED => Value::Undefined,
            NULL => Value::Null,
            BOOLEAN => Value::Boolean(field.payload != 0),
            NUMBER => Value::Number(f64::from_bits(field.payload)),
            BIGINT => Value::BigInt(self.arena.text(field.payload, field.value_length)?),
            STRING => Value::String(self.arena.units(field.payload, field.value_length)?),
            NODE => Value::Node(u32::try_from(field.payload).ok()?),
            SHARED => Value::Shared(
                self.externals
                    .get(usize::try_from(field.payload).ok()?)?
                    .clone(),
            ),
            _ => return None,
        };
        Some(super::Field {
            key,
            value,
            enumerable: field.enumerable != 0,
        })
    }

    fn find_field_index_matching(
        &self,
        node: u32,
        hash: u64,
        mut matches: impl FnMut(&SealedArena, &FieldRecord) -> bool,
    ) -> Option<usize> {
        let record = self.record(node)?;
        let count = usize::try_from(record.field_count).ok()?;
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
