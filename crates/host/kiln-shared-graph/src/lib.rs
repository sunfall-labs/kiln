//! Realm-neutral, immutable graph snapshots (ADR 0073).
//!
//! An engine adapter supplies own descriptors and collection entries without invoking
//! JavaScript. The builder rejects accessors, symbols, proxies and other internal-slot objects
//! before allocating a published arena.
//! It preserves aliases and cycles by assigning a node index before inspecting that node's
//! children. The resulting arena owns every string unit and graph edge; it has no realm pointer.
//! This crate intentionally exposes no JavaScript API. Both engines must wire reads, mutation,
//! messages and GC before `kiln:shared` can be installed.

use kiln_string::StringUnits;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::{Arc, Mutex, OnceLock, PoisonError, Weak};

pub mod arena;

/// The `TypeError` message for a write to a sealed `Map` or `Set`: the one text both engines
/// throw from `set`, `add`, `delete`, `clear` and `getOrInsert`.
pub fn sealed_collection_write_message(is_map: bool) -> &'static str {
    if is_map {
        "Cannot modify a shared Map"
    } else {
        "Cannot modify a shared Set"
    }
}

/// A source object's identity, meaningful only for one snapshot walk.
pub type SourceId = u64;

/// A value returned by an engine's side-effect-free descriptor inspector.
#[derive(Clone, Debug)]
pub enum InputValue {
    Undefined,
    Null,
    Boolean(bool),
    Number(f64),
    BigInt(String),
    String(StringUnits),
    Object(SourceId),
    /// A graph already sealed by an earlier `share` call. Its handle is retained, not copied.
    Shared(Handle),
}

#[derive(Clone, Debug)]
pub enum Key {
    String(StringUnits),
    Symbol,
}

#[derive(Clone, Debug)]
pub enum Descriptor {
    Data(InputValue),
    Accessor,
}

#[derive(Clone, Debug)]
pub struct Property {
    pub key: Key,
    pub descriptor: Descriptor,
    pub enumerable: bool,
}

/// Every variant but `Unsupported` can be published. That one makes rejection explicit at the
/// semantic owner, even when an adapter recognizes an object kind before reading its keys.
///
/// A `Map` or `Set` is published only when it is an ordinary instance of the reading realm's
/// constructor with no own properties: its entries are its whole observable state. A Set's
/// entries carry no value.
#[derive(Clone, Debug)]
pub enum SourceNode {
    Record {
        null_prototype: bool,
        properties: Vec<Property>,
    },
    Array {
        length: u32,
        properties: Vec<Property>,
    },
    Map {
        entries: Vec<(InputValue, InputValue)>,
    },
    Set {
        values: Vec<InputValue>,
    },
    Unsupported(UnsupportedKind),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnsupportedKind {
    Proxy,
    Function,
    CustomPrototype,
    InternalSlots,
    Symbol,
    CollectionProperties,
}

impl UnsupportedKind {
    /// The stable number an engine's inspector reports a refusal by, across the provider ABI.
    pub fn code(self) -> u32 {
        match self {
            Self::Proxy => 1,
            Self::Function => 2,
            Self::CustomPrototype => 3,
            Self::InternalSlots => 4,
            Self::Symbol => 5,
            Self::CollectionProperties => 6,
        }
    }

    pub fn from_code(code: u32) -> Option<Self> {
        Some(match code {
            1 => Self::Proxy,
            2 => Self::Function,
            3 => Self::CustomPrototype,
            4 => Self::InternalSlots,
            5 => Self::Symbol,
            6 => Self::CollectionProperties,
            _ => return None,
        })
    }

    /// What a refused value is, in the words a program's author uses: the one text both engines
    /// put in `share`'s `TypeError`.
    pub fn describe(self) -> &'static str {
        match self {
            Self::Proxy => "a Proxy",
            Self::Function => "a function",
            Self::CustomPrototype => {
                "an object whose prototype is not Object.prototype or null (a class instance)"
            }
            Self::InternalSlots => {
                "a built-in object with internal state (a Date, WeakMap, RegExp, buffer or the like)"
            }
            Self::Symbol => "a symbol",
            Self::CollectionProperties => "a Map or Set with its own properties",
        }
    }
}

/// `inspect` must read the object's kind and own descriptors directly from engine storage. A
/// proxy trap or getter is never a valid way to implement it.
pub trait Inspector {
    type Error;

    fn inspect(&mut self, object: SourceId) -> Result<SourceNode, Self::Error>;
}

#[derive(Debug)]
pub enum SnapshotError<E> {
    Source(E),
    Unsupported(UnsupportedKind),
    SymbolKey,
    Accessor,
    DuplicateKey,
    InvalidBigInt,
    TooLarge,
}

impl<E: fmt::Display> fmt::Display for SnapshotError<E> {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Source(error) => error.fmt(out),
            Self::Unsupported(kind) => write!(out, "cannot share {}", kind.describe()),
            Self::SymbolKey => out.write_str("cannot share a symbol-keyed property"),
            Self::Accessor => out.write_str("cannot share an accessor property"),
            Self::DuplicateKey => out.write_str("shared graph has a duplicate own property"),
            Self::InvalidBigInt => out.write_str("shared graph has an invalid BigInt"),
            Self::TooLarge => out.write_str("shared graph is too large"),
        }
    }
}

#[derive(Clone, Debug)]
pub enum Value {
    Undefined,
    Null,
    Boolean(bool),
    Number(f64),
    BigInt(String),
    String(StringUnits),
    Node(u32),
    Shared(Handle),
}

#[derive(Clone, Debug)]
pub struct Field {
    pub key: StringUnits,
    pub value: Value,
    pub enumerable: bool,
}

#[derive(Clone, Debug)]
pub enum Node {
    Record {
        null_prototype: bool,
        fields: Vec<Field>,
    },
    Array {
        length: u32,
        fields: Vec<Field>,
    },
    /// Entries in insertion order. Keys are unique under SameValueZero, and `-0` is stored as
    /// `+0`, as `Map.prototype.set` stores it.
    Map {
        entries: Vec<(Value, Value)>,
    },
    Set {
        values: Vec<Value>,
    },
}

/// What one node is, without copying its fields or entries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeKind {
    Record { null_prototype: bool },
    Array { length: u32 },
    Map,
    Set,
}

impl NodeKind {
    pub fn is_collection(self) -> bool {
        matches!(self, Self::Map | Self::Set)
    }
}

/// A key a reader looks up in a sealed `Map` or `Set`, borrowed from that reader's own value.
/// An object key can only match when it is a shared node: `Object` names one by its opaque
/// identity. Any other object is absent from every sealed collection, and a reader answers
/// that without building a probe.
#[derive(Clone, Copy, Debug)]
pub enum EntryProbe<'a> {
    Undefined,
    Null,
    Boolean(bool),
    Number(f64),
    BigInt(&'a str),
    String(&'a [u16]),
    Object(OpaqueHandle),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(C)]
pub struct OpaqueHandle {
    pub graph: u32,
    pub generation: u32,
    pub node: u32,
}

#[derive(Default)]
struct Registry {
    slots: Vec<(u32, Weak<Graph>)>,
    free: Vec<u32>,
}

impl Registry {
    fn insert(&mut self, graph: Weak<Graph>) -> (u32, u32) {
        if let Some(slot) = self.free.pop() {
            let entry = &mut self.slots[slot as usize];
            entry.1 = graph;
            (slot, entry.0)
        } else {
            let slot = u32::try_from(self.slots.len()).expect("too many live shared graphs");
            self.slots.push((1, graph));
            (slot, 1)
        }
    }

    fn remove(&mut self, slot: u32, generation: u32) {
        let Some(entry) = self.slots.get_mut(slot as usize) else {
            return;
        };
        if entry.0 == generation {
            entry.0 = entry.0.wrapping_add(1).max(1);
            entry.1 = Weak::new();
            self.free.push(slot);
        }
    }
}

fn registry() -> &'static Mutex<Registry> {
    static REGISTRY: OnceLock<Mutex<Registry>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(Registry::default()))
}

/// A sealed graph allocation. All nodes, descriptors and string units live in page-aligned,
/// read-only bytes. The only sidecar is a vector of strong handles for earlier sealed arenas.
/// The native value seam must defer final unmapping until every reader passes a turn boundary.
#[derive(Debug)]
pub struct Graph {
    slot: u32,
    generation: u32,
    arena: arena::SealedArena,
    node_count: u32,
    externals: Vec<Handle>,
}

impl Graph {
    pub fn id(&self) -> u64 {
        (u64::from(self.generation) << 32) | u64::from(self.slot)
    }

    /// Whether `token` names a node of this graph rather than of another one.
    pub fn owns(&self, token: OpaqueHandle) -> bool {
        token.graph == self.slot && token.generation == self.generation
    }

    pub fn len(&self) -> usize {
        self.node_count as usize
    }

    pub fn is_empty(&self) -> bool {
        self.node_count == 0
    }
}

impl Drop for Graph {
    fn drop(&mut self) {
        registry()
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(self.slot, self.generation);
    }
}

/// A strong reference to one node of a graph. Cloning this retains the graph; a queued message
/// owns its own copy and discarding it releases that copy. Identity includes both graph and node.
#[derive(Clone, Debug)]
pub struct Handle {
    graph: Arc<Graph>,
    index: u32,
}

impl Handle {
    pub fn graph(&self) -> &Graph {
        &self.graph
    }

    pub fn index(&self) -> u32 {
        self.index
    }

    pub fn child(&self, index: u32) -> Option<Self> {
        (index < self.graph.node_count).then(|| Self {
            graph: Arc::clone(&self.graph),
            index,
        })
    }

    pub fn same_object(&self, other: &Self) -> bool {
        self.index == other.index && Arc::ptr_eq(&self.graph, &other.graph)
    }

    pub fn opaque(&self) -> OpaqueHandle {
        OpaqueHandle {
            graph: self.graph.slot,
            generation: self.graph.generation,
            node: self.index,
        }
    }

    /// Invalidate the opaque ID before deferring the last strong handle for a grace period.
    /// Resolution takes the same registry lock, so it either acquires a real strong handle
    /// first or sees the generation change; a stale ID cannot resurrect a retired arena.
    pub fn invalidate_if_last(&self) {
        let mut table = registry().lock().unwrap_or_else(PoisonError::into_inner);
        if Arc::strong_count(&self.graph) == 1 {
            table.remove(self.graph.slot, self.graph.generation);
        }
    }

    /// Resolve a token only inside this process. A stale generation, including one whose slot
    /// has since been reused by another graph, never becomes a pointer to the new graph.
    pub fn resolve(token: OpaqueHandle) -> Option<Self> {
        let table = registry().lock().unwrap_or_else(PoisonError::into_inner);
        let (generation, weak) = table.slots.get(token.graph as usize)?;
        if *generation != token.generation {
            return None;
        }
        let graph = weak.upgrade()?;
        (token.node < graph.node_count).then_some(Self {
            graph,
            index: token.node,
        })
    }
}

#[derive(Clone, Debug)]
pub enum Snapshot {
    Primitive(Value),
    Shared(Handle),
}

/// Build the complete graph privately. Any failure drops the draft and publishes nothing.
pub fn share<I: Inspector>(
    root: InputValue,
    inspector: &mut I,
) -> Result<Snapshot, SnapshotError<I::Error>> {
    let root_id = match root {
        InputValue::Shared(handle) => return Ok(Snapshot::Shared(handle)),
        InputValue::Object(id) => id,
        scalar => return Ok(Snapshot::Primitive(scalar_value(scalar))),
    };

    let mut indices = HashMap::new();
    let mut pending = Vec::new();
    let mut nodes = Vec::<Option<Node>>::new();
    indices.insert(root_id, 0u32);
    pending.push(root_id);
    nodes.push(None);

    while let Some(id) = pending.pop() {
        let at = indices[&id] as usize;
        let source = inspector.inspect(id).map_err(SnapshotError::Source)?;
        let mut edge = |value: InputValue| -> Result<Value, SnapshotError<I::Error>> {
            Ok(match value {
                InputValue::Object(child) => match indices.get(&child) {
                    Some(index) => Value::Node(*index),
                    None => {
                        let index =
                            u32::try_from(nodes.len()).map_err(|_| SnapshotError::TooLarge)?;
                        indices.insert(child, index);
                        pending.push(child);
                        nodes.push(None);
                        Value::Node(index)
                    }
                },
                InputValue::Shared(handle) => Value::Shared(handle),
                InputValue::BigInt(text) => {
                    let digits = text.strip_prefix('-').unwrap_or(&text);
                    if digits.is_empty() || !digits.bytes().all(|digit| digit.is_ascii_digit()) {
                        return Err(SnapshotError::InvalidBigInt);
                    }
                    Value::BigInt(text)
                }
                scalar => scalar_value(scalar),
            })
        };
        let (array_length, null_prototype, properties) = match source {
            SourceNode::Record {
                null_prototype,
                properties,
            } => (None, null_prototype, properties),
            SourceNode::Array { length, properties } => (Some(length), false, properties),
            SourceNode::Map { entries } => {
                let mut keys = HashSet::with_capacity(entries.len());
                let mut sealed = Vec::with_capacity(entries.len());
                for (key, value) in entries {
                    let key = edge(key)?;
                    if !keys.insert(EntryKey::of(&key)) {
                        return Err(SnapshotError::DuplicateKey);
                    }
                    sealed.push((key.canonical_key(), edge(value)?));
                }
                nodes[at] = Some(Node::Map { entries: sealed });
                continue;
            }
            SourceNode::Set { values } => {
                let mut keys = HashSet::with_capacity(values.len());
                let mut sealed = Vec::with_capacity(values.len());
                for value in values {
                    let value = edge(value)?;
                    if !keys.insert(EntryKey::of(&value)) {
                        return Err(SnapshotError::DuplicateKey);
                    }
                    sealed.push(value.canonical_key());
                }
                nodes[at] = Some(Node::Set { values: sealed });
                continue;
            }
            SourceNode::Unsupported(kind) => return Err(SnapshotError::Unsupported(kind)),
        };
        let mut fields = Vec::with_capacity(properties.len());
        let mut keys = HashSet::with_capacity(properties.len());
        for property in properties {
            let Key::String(key) = property.key else {
                return Err(SnapshotError::SymbolKey);
            };
            if !keys.insert(key.clone()) {
                return Err(SnapshotError::DuplicateKey);
            }
            let Descriptor::Data(value) = property.descriptor else {
                return Err(SnapshotError::Accessor);
            };
            let value = edge(value)?;
            fields.push(Field {
                key,
                value,
                enumerable: property.enumerable,
            });
        }
        nodes[at] = Some(match array_length {
            Some(length) => Node::Array { length, fields },
            None => Node::Record {
                null_prototype,
                fields,
            },
        });
    }

    let nodes: Box<[Node]> = nodes
        .into_iter()
        .map(|node| node.expect("each discovered object is inspected"))
        .collect();
    let node_count = u32::try_from(nodes.len()).map_err(|_| SnapshotError::TooLarge)?;
    let (arena, externals) = arena::seal(&nodes).map_err(|()| SnapshotError::TooLarge)?;
    let graph = Arc::new_cyclic(|weak| {
        let (slot, generation) = registry()
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(weak.clone());
        Graph {
            slot,
            generation,
            arena,
            node_count,
            externals,
        }
    });
    Ok(Snapshot::Shared(Handle { graph, index: 0 }))
}

/// A sealed collection key's SameValueZero identity, for rejecting a duplicate before publication.
#[derive(Clone, PartialEq, Eq, Hash)]
enum EntryKey {
    Undefined,
    Null,
    Boolean(bool),
    Number(u64),
    BigInt(String),
    String(StringUnits),
    Node(u32),
    Shared(u64, u32),
}

impl EntryKey {
    fn of(value: &Value) -> Self {
        match value {
            Value::Undefined => Self::Undefined,
            Value::Null => Self::Null,
            Value::Boolean(flag) => Self::Boolean(*flag),
            Value::Number(number) => Self::Number(arena::same_value_zero_bits(*number)),
            Value::BigInt(text) => Self::BigInt(text.clone()),
            Value::String(text) => Self::String(text.clone()),
            Value::Node(index) => Self::Node(*index),
            Value::Shared(handle) => Self::Shared(handle.graph().id(), handle.index()),
        }
    }
}

impl Value {
    /// `-0` becomes `+0`, as `Map.prototype.set` and `Set.prototype.add` store it.
    fn canonical_key(self) -> Self {
        match self {
            // A float pattern compares with `==`, so this arm takes `-0` as well as `+0`.
            Self::Number(0.0) => Self::Number(0.0),
            other => other,
        }
    }
}

fn scalar_value(value: InputValue) -> Value {
    match value {
        InputValue::Undefined => Value::Undefined,
        InputValue::Null => Value::Null,
        InputValue::Boolean(value) => Value::Boolean(value),
        InputValue::Number(value) => Value::Number(value),
        InputValue::BigInt(value) => Value::BigInt(value),
        InputValue::String(value) => Value::String(value),
        InputValue::Object(_) | InputValue::Shared(_) => unreachable!(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field(name: &str, value: InputValue) -> Property {
        Property {
            key: Key::String(name.into()),
            descriptor: Descriptor::Data(value),
            enumerable: true,
        }
    }

    struct Table(HashMap<SourceId, SourceNode>);

    impl Inspector for Table {
        type Error = &'static str;

        fn inspect(&mut self, object: SourceId) -> Result<SourceNode, Self::Error> {
            self.0.get(&object).cloned().ok_or("missing object")
        }
    }

    #[test]
    fn cycles_aliases_holes_and_nested_handles() {
        let mut first = Table(HashMap::from([(
            1,
            SourceNode::Record {
                null_prototype: true,
                properties: vec![field("self", InputValue::Object(1))],
            },
        )]));
        let Snapshot::Shared(older) = share(InputValue::Object(1), &mut first).unwrap() else {
            panic!("object must produce a graph")
        };
        let mut next = Table(HashMap::from([
            (
                2,
                SourceNode::Array {
                    length: 4,
                    properties: vec![
                        field("0", InputValue::Object(3)),
                        field("2", InputValue::Object(3)),
                        field("extra", InputValue::Shared(older.clone())),
                    ],
                },
            ),
            (
                3,
                SourceNode::Record {
                    null_prototype: false,
                    properties: vec![field("parent", InputValue::Object(2))],
                },
            ),
        ]));
        let Snapshot::Shared(root) = share(InputValue::Object(2), &mut next).unwrap() else {
            panic!("object must produce a graph")
        };
        let Node::Array { length, fields } = root.graph().node(0).unwrap() else {
            panic!("expected array")
        };
        assert_eq!(length, 4);
        assert_eq!(fields.len(), 3, "the hole at index 1 stays absent");
        assert!(matches!(
            (&fields[0].value, &fields[1].value),
            (Value::Node(1), Value::Node(1))
        ));
        let Value::Shared(nested) = &fields[2].value else {
            panic!("existing graph must remain a handle")
        };
        assert!(nested.same_object(&older));
        let Node::Record { fields, .. } = root.graph().node(1).unwrap() else {
            panic!("expected record")
        };
        assert!(matches!(fields[0].value, Value::Node(0)));
        let Snapshot::Shared(echo) = share(InputValue::Shared(root.clone()), &mut next).unwrap()
        else {
            panic!("shared input must remain shared")
        };
        assert!(echo.same_object(&root));
        let Snapshot::Shared(distinct) = share(InputValue::Object(2), &mut next).unwrap() else {
            panic!("ordinary source must produce a graph")
        };
        assert!(!distinct.same_object(&root));
    }

    #[test]
    fn sealed_index_finds_large_sparse_array_fields_without_changing_order() {
        let mut properties = Vec::new();
        for index in (0..80).step_by(2) {
            properties.push(field(&index.to_string(), InputValue::Number(index as f64)));
        }
        properties.push(field("note", InputValue::String("kept".into())));
        let mut source = Table(HashMap::from([(
            1,
            SourceNode::Array {
                length: 80,
                properties,
            },
        )]));
        let Snapshot::Shared(root) = share(InputValue::Object(1), &mut source).unwrap() else {
            panic!("expected shared array")
        };
        for (position, index) in (0..80).step_by(2).enumerate() {
            let key: StringUnits = index.to_string().into();
            assert_eq!(root.graph().find_field_index(0, &key), Some(position));
            assert_eq!(
                root.graph().find_field_index_units(0, &key.to_units()),
                Some(position)
            );
        }
        assert_eq!(root.graph().find_field_index(0, &"note".into()), Some(40));
        assert_eq!(root.graph().find_field_index(0, &"1".into()), None);
        assert_eq!(root.graph().find_field_index_units(0, &[0xd800]), None);
        let Node::Array { fields, .. } = root.graph().node(0).unwrap() else {
            panic!("expected shared array")
        };
        assert_eq!(fields[0].key, "0");
        assert_eq!(fields[40].key, "note");
    }

    #[test]
    fn rejects_invalid_graph_without_inspecting_a_proxy_or_accessor_value() {
        let mut source = Table(HashMap::from([
            (
                1,
                SourceNode::Record {
                    null_prototype: false,
                    properties: vec![field("child", InputValue::Object(2))],
                },
            ),
            (2, SourceNode::Unsupported(UnsupportedKind::Proxy)),
        ]));
        assert!(matches!(
            share(InputValue::Object(1), &mut source),
            Err(SnapshotError::Unsupported(UnsupportedKind::Proxy))
        ));
        source.0.insert(
            2,
            SourceNode::Record {
                null_prototype: false,
                properties: vec![Property {
                    key: Key::String("secret".into()),
                    descriptor: Descriptor::Accessor,
                    enumerable: false,
                }],
            },
        );
        assert!(matches!(
            share(InputValue::Object(1), &mut source),
            Err(SnapshotError::Accessor)
        ));
        source.0.insert(
            2,
            SourceNode::Record {
                null_prototype: false,
                properties: vec![Property {
                    key: Key::Symbol,
                    descriptor: Descriptor::Data(InputValue::Number(1.0)),
                    enumerable: false,
                }],
            },
        );
        assert!(matches!(
            share(InputValue::Object(1), &mut source),
            Err(SnapshotError::SymbolKey)
        ));
    }

    #[test]
    fn rejects_duplicate_keys_and_invalid_bigints_before_publication() {
        let mut source = Table(HashMap::from([(
            1,
            SourceNode::Record {
                null_prototype: false,
                properties: vec![
                    field("same", InputValue::Number(1.0)),
                    field("same", InputValue::Number(2.0)),
                ],
            },
        )]));
        assert!(matches!(
            share(InputValue::Object(1), &mut source),
            Err(SnapshotError::DuplicateKey)
        ));
        source.0.insert(
            1,
            SourceNode::Record {
                null_prototype: false,
                properties: vec![field("number", InputValue::BigInt("1e8".into()))],
            },
        );
        assert!(matches!(
            share(InputValue::Object(1), &mut source),
            Err(SnapshotError::InvalidBigInt)
        ));
    }

    fn probe_units(text: &str) -> Vec<u16> {
        text.encode_utf16().collect()
    }

    /// Every key kind, in a collection small enough to scan and in one large enough to index.
    #[test]
    fn collections_find_every_key_kind_under_same_value_zero() {
        let mut older = Table(HashMap::from([(
            9,
            SourceNode::Record {
                null_prototype: false,
                properties: Vec::new(),
            },
        )]));
        let Snapshot::Shared(external) = share(InputValue::Object(9), &mut older).unwrap() else {
            panic!("expected graph")
        };
        for padding in [0usize, 40] {
            let mut entries = vec![
                (InputValue::Undefined, InputValue::Number(1.0)),
                (InputValue::Null, InputValue::Number(2.0)),
                (InputValue::Boolean(true), InputValue::Number(3.0)),
                (InputValue::Number(-0.0), InputValue::Number(4.0)),
                (InputValue::Number(f64::NAN), InputValue::Number(5.0)),
                (InputValue::String("acme".into()), InputValue::Object(2)),
                (InputValue::BigInt("-12".into()), InputValue::Number(7.0)),
                (InputValue::Object(2), InputValue::String("node".into())),
                (
                    InputValue::Shared(external.clone()),
                    InputValue::Shared(external.clone()),
                ),
            ];
            for extra in 0..padding {
                entries.push((
                    InputValue::String(format!("k{extra}").as_str().into()),
                    InputValue::Number(extra as f64),
                ));
            }
            let mut source = Table(HashMap::from([
                (1, SourceNode::Map { entries }),
                (
                    2,
                    SourceNode::Record {
                        null_prototype: true,
                        properties: Vec::new(),
                    },
                ),
            ]));
            let Snapshot::Shared(map) = share(InputValue::Object(1), &mut source).unwrap() else {
                panic!("expected graph")
            };
            let graph = map.graph();
            assert_eq!(graph.node_kind(0), Some(NodeKind::Map));
            assert_eq!(
                graph.node_info(0),
                Some((false, false, 0, 0)),
                "no own fields"
            );
            assert_eq!(graph.collection_len(0), Some(9 + padding));
            let node = map.child(1).unwrap().opaque();
            let acme = probe_units("acme");
            let probes = [
                (EntryProbe::Undefined, 0),
                (EntryProbe::Null, 1),
                (EntryProbe::Boolean(true), 2),
                (EntryProbe::Number(0.0), 3),
                (EntryProbe::Number(f64::NAN), 4),
                (EntryProbe::String(&acme), 5),
                (EntryProbe::BigInt("-12"), 6),
                (EntryProbe::Object(node), 7),
                (EntryProbe::Object(external.opaque()), 8),
            ];
            for (probe, expected) in probes {
                assert_eq!(graph.find_entry(0, probe), Some(expected), "{probe:?}");
            }
            for (probe, label) in [
                (EntryProbe::Boolean(false), "false"),
                (EntryProbe::Number(1.0), "one"),
                (EntryProbe::BigInt("12"), "positive bigint"),
                (EntryProbe::Object(map.opaque()), "the map itself"),
            ] {
                assert_eq!(graph.find_entry(0, probe), None, "{label}");
            }
            if padding != 0 {
                let last = probe_units("k39");
                assert_eq!(graph.find_entry(0, EntryProbe::String(&last)), Some(48));
            }
            let (key, value) = graph.entry(0, 3).unwrap();
            assert!(
                matches!(key, Value::Number(zero) if zero == 0.0 && zero.is_sign_positive()),
                "-0 is stored as +0"
            );
            assert!(matches!(value, Value::Number(four) if four == 4.0));
            assert!(
                matches!(graph.entry(0, 8).unwrap().0, Value::Shared(ref key) if key.same_object(&external))
            );
        }
    }

    #[test]
    fn sets_report_each_value_twice_and_refuse_same_value_zero_duplicates() {
        let mut source = Table(HashMap::from([(
            1,
            SourceNode::Set {
                values: vec![InputValue::String("a".into()), InputValue::Number(1.0)],
            },
        )]));
        let Snapshot::Shared(set) = share(InputValue::Object(1), &mut source).unwrap() else {
            panic!("expected graph")
        };
        assert_eq!(set.graph().node_kind(0), Some(NodeKind::Set));
        let (key, value) = set.graph().entry(0, 1).unwrap();
        assert!(
            matches!((key, value), (Value::Number(a), Value::Number(b)) if a == 1.0 && b == 1.0)
        );
        assert_eq!(set.graph().find_entry(0, EntryProbe::Number(1.0)), Some(1));
        source.0.insert(
            1,
            SourceNode::Set {
                values: vec![InputValue::Number(0.0), InputValue::Number(-0.0)],
            },
        );
        assert!(matches!(
            share(InputValue::Object(1), &mut source),
            Err(SnapshotError::DuplicateKey)
        ));
    }

    #[test]
    fn opaque_handle_checks_generation_and_page_backed_data_outlives_sender() {
        let mut source = Table(HashMap::from([(
            1,
            SourceNode::Record {
                null_prototype: false,
                properties: vec![field("label", InputValue::String("sealed".into()))],
            },
        )]));
        let Snapshot::Shared(handle) = share(InputValue::Object(1), &mut source).unwrap() else {
            panic!("expected graph")
        };
        let token = handle.opaque();
        let retained_by_queue = Handle::resolve(token).expect("live opaque handle");
        #[cfg(unix)]
        {
            let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) } as usize;
            assert_eq!((handle.graph().arena_ptr() as usize) % page, 0);
        }
        assert!(handle.graph().arena_len() >= std::mem::size_of::<arena::NodeRecord>());
        drop(handle);
        let Node::Record { fields, .. } = retained_by_queue.graph().node(token.node).unwrap()
        else {
            panic!("expected record")
        };
        assert!(matches!(&fields[0].value, Value::String(text) if text == "sealed"));
        drop(retained_by_queue);
        assert!(
            Handle::resolve(token).is_none(),
            "last strong handle retired"
        );
        let Snapshot::Shared(replacement) = share(InputValue::Object(1), &mut source).unwrap()
        else {
            panic!("expected graph")
        };
        if replacement.opaque().graph == token.graph {
            assert_ne!(replacement.opaque().generation, token.generation);
        }
        assert!(
            Handle::resolve(token).is_none(),
            "stale token cannot name a new graph"
        );
    }
}
