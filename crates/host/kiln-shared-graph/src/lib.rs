//! Realm-neutral, immutable graph snapshots (ADR 0073).
//!
//! An engine adapter supplies own descriptors without invoking JavaScript. The builder rejects
//! accessors, symbols, proxies and internal-slot objects before allocating a published arena.
//! It preserves aliases and cycles by assigning a node index before inspecting that node's
//! children. The resulting arena owns every string unit and graph edge; it has no realm pointer.
//! This crate intentionally exposes no JavaScript API. Both engines must wire reads, mutation,
//! messages and GC before `kiln:shared` can be installed.

use kiln_string::StringUnits;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::{Arc, Mutex, OnceLock, PoisonError, Weak};

pub mod arena;

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

/// Only the first two variants can be published. The other variants make rejection explicit at
/// the semantic owner, even when an adapter recognizes an object kind before reading its keys.
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
    Unsupported(UnsupportedKind),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnsupportedKind {
    Proxy,
    Function,
    CustomPrototype,
    InternalSlots,
    Symbol,
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
            Self::Unsupported(kind) => write!(out, "cannot share {kind:?}"),
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
        let (array_length, null_prototype, properties) = match source {
            SourceNode::Record {
                null_prototype,
                properties,
            } => (None, null_prototype, properties),
            SourceNode::Array { length, properties } => (Some(length), false, properties),
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
            let value = match value {
                InputValue::Object(child) => {
                    let index = match indices.get(&child) {
                        Some(index) => *index,
                        None => {
                            let index =
                                u32::try_from(nodes.len()).map_err(|_| SnapshotError::TooLarge)?;
                            indices.insert(child, index);
                            pending.push(child);
                            nodes.push(None);
                            index
                        }
                    };
                    Value::Node(index)
                }
                InputValue::Shared(handle) => Value::Shared(handle),
                InputValue::BigInt(text) => {
                    let digits = text.strip_prefix('-').unwrap_or(&text);
                    if digits.is_empty() || !digits.bytes().all(|digit| digit.is_ascii_digit()) {
                        return Err(SnapshotError::InvalidBigInt);
                    }
                    Value::BigInt(text)
                }
                scalar => scalar_value(scalar),
            };
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
