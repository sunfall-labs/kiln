use kiln_json::{TAG_ARRAY, TAG_OBJECT, TAG_STRING};

fn iterations() -> usize {
    std::env::var("KILN_FUZZ_ITERATIONS")
        .map(|n| n.parse().expect("positive iteration count"))
        .unwrap_or(10_000)
}

fn inspect(tape: &kiln_json::Tape<'_>) {
    let mut pending = vec![1usize];
    for node in tape.nodes() {
        while pending.last() == Some(&0) {
            pending.pop();
        }
        let remaining = pending.last_mut().expect("no trailing nodes after root");
        *remaining -= 1;
        match node.tag {
            TAG_ARRAY => pending.push(node.len as usize),
            TAG_OBJECT => pending.push(2 * node.len as usize),
            TAG_STRING => {
                assert!(tape.string_units(node).is_some(), "string outside arena");
            }
            0..=3 => {}
            tag => panic!("invalid tape tag {tag}"),
        }
    }
    assert!(pending.iter().all(|&n| n == 0), "truncated tape");
}

#[test]
fn json_bytes_and_utf16_preserve_tape_invariants() {
    bolero::check!()
        .with_iterations(iterations())
        .for_each(|bytes| {
            let bytes = &bytes[..bytes.len().min(16_384)];
            let parsed = kiln_json::parse_utf8(bytes, |tape| {
                inspect(tape);
                (
                    tape.nodes().to_vec(),
                    tape.nodes()
                        .iter()
                        .filter(|n| n.tag == TAG_STRING)
                        .map(|n| tape.string_units(n).unwrap())
                        .collect::<Vec<_>>(),
                )
            });
            if let Ok((nodes, strings)) = parsed {
                // The borrowed reader uses thread-local scratch: compare after its callback exits.
                let owned = kiln_json::parse_owned(bytes).expect("owned reader agrees");
                assert_eq!(nodes, owned.nodes());
                let owned_strings: Vec<_> = owned
                    .nodes()
                    .iter()
                    .filter(|n| n.tag == TAG_STRING)
                    .map(|n| owned.string_units(n).unwrap())
                    .collect();
                assert_eq!(strings, owned_strings);
            }
            let units: Vec<u16> = bytes
                .chunks(2)
                .map(|b| u16::from_le_bytes([b[0], *b.get(1).unwrap_or(&0)]))
                .collect();
            let _ = kiln_json::parse_units(&kiln_string::StringUnits::from_units(&units), |tape| {
                inspect(tape)
            });
        });
}

fn compare_value(tape: &kiln_json::Tape<'_>, at: &mut usize, value: &serde_json::Value) {
    let node = &tape.nodes()[*at];
    *at += 1;
    match value {
        serde_json::Value::Null => assert_eq!(node.tag, kiln_json::TAG_NULL),
        serde_json::Value::Bool(b) => assert_eq!(node.tag, if *b { 2 } else { 1 }),
        serde_json::Value::Number(n) => assert_eq!(node.as_number(), n.as_f64()),
        serde_json::Value::String(s) => {
            assert_eq!(node.tag, TAG_STRING);
            assert_eq!(
                tape.string_units(node).unwrap().units().collect::<Vec<_>>(),
                s.encode_utf16().collect::<Vec<_>>()
            );
        }
        serde_json::Value::Array(values) => {
            assert_eq!(node.tag, TAG_ARRAY);
            assert_eq!(node.len as usize, values.len());
            for value in values {
                compare_value(tape, at, value);
            }
        }
        serde_json::Value::Object(fields) => {
            assert_eq!(node.tag, TAG_OBJECT);
            assert_eq!(node.len as usize, fields.len());
            for (key, value) in fields {
                compare_value(tape, at, &serde_json::Value::String(key.clone()));
                compare_value(tape, at, value);
            }
        }
    }
}

#[test]
fn generated_json_agrees_with_independent_reader() {
    bolero::check!()
        .with_iterations(iterations())
        .for_each(|bytes| {
            // Valid grammar keeps the campaign reaching values instead of only syntax rejection.
            let bytes = &bytes[..bytes.len().min(4096)];
            let values: Vec<serde_json::Value> = bytes
                .chunks(8)
                .map(|chunk| match chunk[0] % 5 {
                    0 => serde_json::Value::Null,
                    1 => (chunk[0] & 1 == 0).into(),
                    2 => (i64::from_le_bytes({
                        let mut word = [0u8; 8];
                        word[..chunk.len()].copy_from_slice(chunk);
                        word
                    }))
                    .into(),
                    3 => {
                        let mut word = [0u8; 8];
                        word[..chunk.len()].copy_from_slice(chunk);
                        let number = f64::from_bits(u64::from_le_bytes(word));
                        serde_json::Number::from_f64(number)
                            .map(serde_json::Value::Number)
                            .unwrap_or(serde_json::Value::Null)
                    }
                    _ => String::from_utf8_lossy(chunk).into_owned().into(),
                })
                .collect();
            let mut value = serde_json::json!({"data": values, "empty": {}, "list": [],
                "text": String::from_utf8_lossy(bytes)});
            for _ in 0..bytes.first().copied().unwrap_or(0) % 8 {
                value = serde_json::Value::Array(vec![value]);
            }
            let source = serde_json::to_vec(&value).unwrap();
            kiln_json::parse_utf8(&source, |tape| {
                inspect(tape);
                let mut at = 0;
                compare_value(tape, &mut at, &value);
                assert_eq!(at, tape.nodes().len());
            })
            .expect("valid JSON must parse");
        });
}
