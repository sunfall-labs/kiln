//! A source guard for audited reference fields. Pointer/value helpers remain the authority;
//! this catches direct writes (including macro RHSs) before an executable oracle can miss them.
//! It deliberately does not claim coverage of arbitrary pointer aliases or bulk-copy calls.

fn tokens(source: &str) -> Vec<(&str, usize)> {
    let bytes = source.as_bytes();
    let mut output = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        let start = index;
        if bytes[index].is_ascii_whitespace() {
            index += 1;
        } else if bytes[index..].starts_with(b"//") {
            while index < bytes.len() && bytes[index] != b'\n' {
                index += 1;
            }
        } else if bytes[index..].starts_with(b"/*") {
            index += 2;
            while index + 1 < bytes.len() && !bytes[index..].starts_with(b"*/") {
                index += 1;
            }
            index = (index + 2).min(bytes.len());
        } else if matches!(bytes[index], b'\'' | b'"') {
            let quote = bytes[index];
            index += 1;
            while index < bytes.len() {
                if bytes[index] == b'\\' {
                    index = (index + 2).min(bytes.len());
                } else if bytes[index] == quote {
                    index += 1;
                    break;
                } else {
                    index += 1;
                }
            }
        } else {
            if bytes[index].is_ascii_alphanumeric() || bytes[index] == b'_' {
                index += 1;
                while index < bytes.len()
                    && (bytes[index].is_ascii_alphanumeric() || bytes[index] == b'_')
                {
                    index += 1;
                }
            } else if index + 2 < bytes.len() && matches!(&bytes[index..index + 3], b"<<=" | b">>=")
            {
                index += 3;
            } else if index + 1 < bytes.len()
                && matches!(
                    &bytes[index..index + 2],
                    b"->"
                        | b"=="
                        | b"!="
                        | b"<="
                        | b">="
                        | b"++"
                        | b"--"
                        | b"+="
                        | b"-="
                        | b"*="
                        | b"/="
                        | b"%="
                        | b"&="
                        | b"|="
                        | b"^="
                )
            {
                index += 2;
            } else {
                // C punctuation is ASCII; skip non-ASCII source text outside tokens as a unit.
                index += source[index..].chars().next().unwrap().len_utf8();
            }
            output.push((&source[start..index], start));
        }
    }
    output
}

fn direct_reference_writes(source: &str) -> Vec<usize> {
    let words = tokens(source);
    let mut found = Vec::new();
    for index in 0..words.len().saturating_sub(2) {
        if words[index].0 != "->" {
            continue;
        }
        let field = words[index + 1].0;
        let protected = matches!(
            field,
            "captured_this"
                | "captured_new_target"
                | "captured_arguments"
                | "own_prototype"
                | "own_store"
                | "source_value"
                | "description_value"
                | "match_groups"
                | "slots"
                | "elements"
                | "properties"
                | "buffer"
                | "upvalues"
        );
        let collection_entry =
            field == "entries" && index > 0 && words[index - 1].0 == "collection";
        if !protected && !collection_entry {
            continue;
        }
        let mut next = index + 2;
        if words.get(next).is_some_and(|w| w.0 == "[") {
            let mut depth = 1;
            next += 1;
            while next < words.len() && depth != 0 {
                match words[next].0 {
                    "[" => depth += 1,
                    "]" => depth -= 1,
                    _ => {}
                }
                next += 1;
            }
        }
        if collection_entry {
            if words.get(next).is_none_or(|w| w.0 != ".")
                || words
                    .get(next + 1)
                    .is_none_or(|w| !matches!(w.0, "key" | "value"))
            {
                continue;
            }
            next += 2;
        }
        if words.get(next).is_some_and(|w| {
            matches!(
                w.0,
                "=" | "+="
                    | "-="
                    | "*="
                    | "/="
                    | "%="
                    | "&="
                    | "|="
                    | "^="
                    | "<<="
                    | ">>="
                    | "++"
                    | "--"
            )
        }) {
            found.push(words[index].1);
        }
    }
    found
}

#[test]
fn audited_reference_fields_use_store_authorities() {
    let mut failures = Vec::new();
    for (path, source) in crate::prelude_source_parts() {
        for offset in direct_reference_writes(source) {
            let line = source[..offset]
                .bytes()
                .filter(|byte| *byte == b'\n')
                .count()
                + 1;
            failures.push(format!("{path}:{line}"));
        }
    }
    assert!(
        failures.is_empty(),
        "direct writes bypass reference-store authorities:\n{}",
        failures.join("\n")
    );
}

#[test]
fn reference_guard_catches_macro_and_indexed_writes_without_matching_reads() {
    let source = r#"
        // array->elements[0] = value;
        const char *note = "object->slots = pointer";
        if (array->elements[index] == value && view->buffer != NULL) {}
        array->elements[index + nested[0]] = KILN_VAL_TAG_PTR(TAG, target);
        collection->entries[index].value = value;
        object->slots += 1;
        array->elements[index] |= KILN_TAG_OBJECT_PREFIX;
        array->elements[index] <<= 1;
        kiln_c99_heap_store_val(array, &array->elements[0], value);
    "#;
    assert_eq!(direct_reference_writes(source).len(), 5);
}
