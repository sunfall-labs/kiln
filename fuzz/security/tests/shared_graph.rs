use kiln_shared_graph::{
    Descriptor, InputValue, Inspector, Key, Property, Snapshot, SourceNode, Value,
};
use kiln_string::StringUnits;

struct Source(Vec<SourceNode>);
impl Inspector for Source {
    type Error = &'static str;
    fn inspect(&mut self, id: u64) -> Result<SourceNode, Self::Error> {
        self.0.get(id as usize).cloned().ok_or("bad source ID")
    }
}

fn value(byte: u8, count: usize) -> InputValue {
    match byte % 6 {
        0 => InputValue::Object(usize::from(byte) as u64 % count as u64),
        1 => InputValue::Number(f64::from_bits(u64::from(byte) << 56)),
        2 => InputValue::String(StringUnits::from_units(&[
            u16::from(byte),
            0xd800,
            0,
            0xdc00,
        ])),
        3 => InputValue::BigInt(format!("-{}", byte)),
        4 => InputValue::Boolean(byte & 1 == 0),
        _ => InputValue::Null,
    }
}

fn check_value(value: &Value, handle: &kiln_shared_graph::Handle) {
    if let Value::Node(index) = value {
        assert!(handle.child(*index).is_some(), "edge outside graph");
    }
}

#[test]
fn shared_graph_readers_preserve_edges_and_reject_stale_handles() {
    let iterations = std::env::var("KILN_FUZZ_ITERATIONS")
        .map(|n| n.parse().expect("positive iteration count"))
        .unwrap_or(10_000);
    bolero::check!()
        .with_iterations(iterations)
        .for_each(|bytes| {
            let bytes = &bytes[..bytes.len().min(512)];
            let count = bytes.len().div_ceil(32).max(1);
            let mut source = Source(Vec::new());
            for (id, chunk) in bytes.chunks(32).enumerate() {
                let values = chunk.iter().map(|&b| value(b, count)).collect::<Vec<_>>();
                let properties = values
                    .iter()
                    .enumerate()
                    .map(|(i, v)| Property {
                        key: Key::String(StringUnits::from_units(&[i as u16, 0xd800, id as u16])),
                        descriptor: Descriptor::Data(v.clone()),
                        enumerable: i % 2 == 0,
                    })
                    .collect();
                source.0.push(match chunk[0] % 4 {
                    0 => SourceNode::Record {
                        null_prototype: chunk[0] & 4 != 0,
                        properties,
                    },
                    1 => SourceNode::Array {
                        length: u32::from(chunk[0]),
                        properties,
                    },
                    2 => SourceNode::Map {
                        entries: values
                            .iter()
                            .cloned()
                            .enumerate()
                            .map(|(i, v)| {
                                (
                                    InputValue::String(StringUnits::from(i.to_string().as_str())),
                                    v,
                                )
                            })
                            .collect(),
                    },
                    _ => SourceNode::Set {
                        values: std::iter::once(InputValue::Object(id as u64))
                            .chain(values.iter().enumerate().map(|(i, _)| {
                                InputValue::String(StringUnits::from_units(&[
                                    i as u16,
                                    0xd800,
                                    chunk[i] as u16,
                                ]))
                            }))
                            .collect(),
                    },
                });
            }
            if source.0.is_empty() {
                source.0.push(SourceNode::Record {
                    null_prototype: false,
                    properties: vec![],
                });
            }
            let Snapshot::Shared(handle) =
                kiln_shared_graph::share(InputValue::Object(0), &mut source).unwrap()
            else {
                panic!("object did not produce graph")
            };
            let graph = handle.graph();
            for node in 0..graph.len() as u32 {
                assert!(graph.node_kind(node).is_some());
                let (_, _, _, fields) = graph.node_info(node).unwrap();
                for index in 0..fields {
                    let field = graph.field(node, index).expect("missing field");
                    assert_eq!(graph.find_field_index(node, &field.key), Some(index));
                    check_value(&field.value, &handle);
                }
                assert!(graph.field(node, fields).is_none());
                if let Some(entries) = graph.collection_len(node) {
                    for index in 0..entries {
                        let (key, value) = graph.entry(node, index).expect("missing entry");
                        check_value(&key, &handle);
                        check_value(&value, &handle);
                    }
                    assert!(graph.entry(node, entries).is_none());
                }
                assert!(graph.field(node, usize::MAX).is_none());
                assert!(graph.entry(node, usize::MAX).is_none());
            }
            assert!(graph.node_kind(u32::MAX).is_none());
            let token = handle.opaque();
            assert!(
                kiln_shared_graph::Handle::resolve(token)
                    .unwrap()
                    .same_object(&handle)
            );
            drop(handle);
            // Allocate another graph so the registry also exercises generation changes on reuse.
            let replacement = kiln_shared_graph::share(InputValue::Object(0), &mut source).unwrap();
            assert!(
                kiln_shared_graph::Handle::resolve(token).is_none(),
                "stale token resurrected a graph"
            );
            drop(replacement);
        });
}
