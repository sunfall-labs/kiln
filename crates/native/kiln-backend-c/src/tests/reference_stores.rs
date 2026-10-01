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
            "hash_next"
                | "captured_this"
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
                | "inline_upvalues"
                | "locals"
                | "queue_promises"
                | "queue_sent"
                | "handlers"
                | "handler_stack"
                | "reactions"
                | "cached_elements"
                | "cached_subject_ptr"
                | "diagnostic_source"
                | "description"
                | "on_fulfilled"
                | "on_rejected"
        );
        let collection_entry =
            field == "entries" && index > 0 && words[index - 1].0 == "collection";
        let bound_reference = index > 0
            && words[index - 1].0 == "bound"
            && matches!(field, "target" | "receiver" | "arguments");
        let require_reference = index > 0
            && words[index - 1].0 == "require"
            && matches!(field, "context" | "root_context");
        let facade_reference = matches!(field, "owner" | "previous" | "next" | "hash_next")
            && words[..index]
                .iter()
                .rev()
                .take(5)
                .any(|word| word.0 == "facade");
        if !protected
            && !collection_entry
            && !bound_reference
            && !require_reference
            && !facade_reference
        {
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
        if collection_entry || matches!(field, "properties" | "reactions") {
            if words.get(next).is_some_and(|w| w.0 == ".") {
                let allowed_fields: &[&str] = match field {
                    "reactions" => &["on_fulfilled", "on_rejected", "derived"],
                    "properties" => &["key", "value", "getter", "setter"],
                    _ => &["key", "value"],
                };
                if words
                    .get(next + 1)
                    .is_none_or(|w| !allowed_fields.contains(&w.0))
                {
                    continue;
                }
                next += 2;
            } else if collection_entry {
                continue;
            }
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

fn reference_sources() -> Vec<(&'static str, &'static str)> {
    let mut sources = crate::prelude_source_parts();
    sources.extend([
        (
            "runtime/prelude_types.h",
            include_str!("../runtime/prelude_types.h"),
        ),
        (
            "runtime/prelude_declarations.h",
            include_str!("../runtime/prelude_declarations.h"),
        ),
    ]);
    sources
}

#[test]
fn audited_reference_fields_use_store_authorities() {
    let mut failures = Vec::new();
    for (path, source) in reference_sources() {
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
        promise->reactions[index].derived = value;
        object->properties[index].getter = value;
        object->properties[index].key = pointer;
        promise->reactions[index].continuation = 1;
        kiln_c99_heap_store_val(array, &array->elements[0], value);
    "#;
    assert_eq!(direct_reference_writes(source).len(), 8);
}

fn raw_reference_copies(source: &str) -> Vec<usize> {
    let words = tokens(source);
    let mut found = Vec::new();
    for index in 0..words.len().saturating_sub(1) {
        if !matches!(words[index].0, "memcpy" | "memmove") || words[index + 1].0 != "(" {
            continue;
        }
        let mut depth = 1;
        let mut next = index + 2;
        while next < words.len() && depth != 0 {
            match words[next].0 {
                "(" => depth += 1,
                ")" => depth -= 1,
                _ => {}
            }
            next += 1;
        }
        let call = &words[index + 2..next];
        if call.iter().any(|w| {
            matches!(
                w.0,
                "kiln_val" | "kiln_value" | "kiln_property" | "kiln_reaction"
            )
        }) || call.windows(2).any(|w| {
            w[0].0 == "->"
                && matches!(
                    w[1].0,
                    "slots"
                        | "elements"
                        | "properties"
                        | "upvalues"
                        | "inline_upvalues"
                        | "locals"
                        | "queue_promises"
                        | "queue_sent"
                        | "reactions"
                        | "cached_elements"
                )
        }) {
            found.push(words[index].1);
        }
    }
    found
}

#[test]
fn audited_reference_copies_use_bulk_store_authorities() {
    let mut failures = Vec::new();
    for (path, source) in reference_sources() {
        for offset in raw_reference_copies(source) {
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
        "raw copies bypass reference-store authorities:\n{}",
        failures.join("\n")
    );
}

#[test]
fn reference_copy_guard_distinguishes_value_buffers_from_bytes() {
    let source = r#"
        // memcpy(destination, array->elements, count);
        memcpy(destination, bytes, length);
        memcpy(&bits, &number, sizeof bits);
        memcpy(destination, source, count * sizeof(kiln_val));
        memcpy(destination, promise->reactions, count * sizeof *destination);
        memmove(array->elements + offset[0], source, count * sizeof *source);
        kiln_c99_heap_copy_vals(array, destination, source, count);
    "#;
    assert_eq!(raw_reference_copies(source).len(), 3);
}

fn foreign_owner_relocations(source: &str) -> Vec<usize> {
    let words = tokens(source);
    let mut found = Vec::new();
    for index in 0..words.len().saturating_sub(1) {
        if !matches!(
            words[index].0,
            "kiln_c99_heap_move_vals" | "kiln_c99_heap_relocate_vals"
        ) || words[index + 1].0 != "("
        {
            continue;
        }
        let mut arguments = Vec::new();
        let mut start = index + 2;
        let mut next = start;
        let mut depth = 0;
        while next < words.len() {
            match words[next].0 {
                "(" | "[" => depth += 1,
                ")" if depth == 0 => {
                    arguments.push(&words[start..next]);
                    break;
                }
                ")" | "]" => depth -= 1,
                "," if depth == 0 => {
                    arguments.push(&words[start..next]);
                    start = next + 1;
                }
                _ => {}
            }
            next += 1;
        }
        // The authority definitions declare a const void* owner; they are not calls.
        if arguments
            .first()
            .is_some_and(|arg| arg.first().is_some_and(|w| w.0 == "const"))
        {
            continue;
        }
        let valid = arguments.len() == 4
            && arguments[0].len() == 1
            && arguments[2].len() >= 3
            && arguments[2][0].0 == arguments[0][0].0
            && arguments[2][1].0 == "->"
            && matches!(arguments[2][2].0, "elements" | "slots");
        if !valid {
            found.push(words[index].1);
        }
    }
    found
}

#[test]
fn relocation_sources_belong_to_the_same_owner() {
    let mut failures = Vec::new();
    for (path, source) in reference_sources() {
        for offset in foreign_owner_relocations(source) {
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
        "relocation requires same-owner source storage:\n{}",
        failures.join("\n")
    );
}

#[test]
fn relocation_guard_rejects_foreign_and_aliased_sources() {
    let source = r#"
        kiln_c99_heap_move_vals(array, array->elements, array->elements + 1, count);
        kiln_c99_heap_relocate_vals(object, moved, object->slots, count);
        kiln_c99_heap_move_vals(array, array->elements, other->elements, count);
        kiln_c99_heap_relocate_vals(object, moved, arbitrary_pointer, count);
        kiln_c99_heap_copy_vals(object, moved, other->slots, count);
    "#;
    assert_eq!(foreign_owner_relocations(source).len(), 2);
}

fn weak_resource_producers(source: &str) -> Vec<String> {
    let words = tokens(source);
    let mut producers = Vec::new();
    for index in 0..words.len().saturating_sub(2) {
        if words[index].0 != "kiln_c99_gc_weak_resource_register" || words[index + 1].0 != "(" {
            continue;
        }
        // Definitions/prototypes start with the heap type, whereas calls pass a value.
        if words[index + 2].0 == "kiln_gc_heap" {
            continue;
        }
        let mut depth = 1;
        let mut next = index + 2;
        while next < words.len() && depth != 0 {
            match words[next].0 {
                "(" => depth += 1,
                ")" => depth -= 1,
                _ => {}
            }
            next += 1;
        }
        producers.push(
            words[index + 2..next.saturating_sub(1)]
                .iter()
                .map(|w| w.0)
                .collect(),
        );
    }
    producers
}

#[test]
fn native_weak_resource_payload_producers_are_audited() {
    let mut producers = Vec::new();
    for (path, source) in reference_sources() {
        for producer in weak_resource_producers(source) {
            producers.push((path.to_owned(), producer));
        }
    }
    // The managed-registration wrapper delegates once to the primitive registration routine;
    // the prepared graph is the only direct payload producer. Keep both sites explicit so a
    // new raw registration call still requires a hidden-reference/lifetime inventory.
    assert_eq!(
        producers,
        vec![(
            "runtime/spread.c".to_owned(),
            "heap,handle,arena,kiln_c99_prepared_graph_drop".to_owned()
        ), (
            "runtime/heap.c".to_owned(),
            "heap,handle,payload,dispose".to_owned()
        )]
    );
}

#[test]
fn weak_resource_guard_finds_new_payload_seams_and_ignores_comments() {
    let source = r#"
        // kiln_c99_gc_weak_resource_register(heap, handle, hidden, drop);
        static int kiln_c99_gc_weak_resource_register(kiln_gc_heap *heap, kiln_value handle,
            void *payload, void (*dispose)(void *));
        kiln_c99_gc_weak_resource_register(heap, handle, hidden, drop);
    "#;
    assert_eq!(
        weak_resource_producers(source),
        vec!["heap,handle,hidden,drop"]
    );
}

// These accessors return either a GC owner's field or a separately enumerated realm root.
// Direct writes through their named result would evade a field-name guard. This checks the
// immediate alias within its function; aliases copied again and output parameters remain a
// separate inventory obligation.
fn callable_slot_alias_writes(source: &str) -> Vec<usize> {
    let words = tokens(source);
    let mut found = Vec::new();
    let mut depth = 0usize;
    let mut body_start = None;
    for (index, word) in words.iter().enumerate() {
        match word.0 {
            "{" => {
                if depth == 0 && index > 0 && words[index - 1].0 == ")" {
                    body_start = Some(index + 1);
                }
                depth += 1;
            }
            "}" => {
                depth = depth.saturating_sub(1);
                if depth != 0 {
                    continue;
                }
                let Some(start) = body_start.take() else {
                    continue;
                };
                let body = &words[start..index];
                let mut aliases = Vec::new();
                for window in body.windows(4) {
                    if window[1].0 == "="
                        && matches!(
                            window[2].0,
                            "kiln_c99_closure_prototype_slot"
                                | "kiln_c99_closure_store_slot"
                                | "kiln_c99_exotic_store"
                        )
                        && window[3].0 == "("
                    {
                        aliases.push(window[0].0);
                    }
                }
                // Copies preserve the storage authority. Iterate to include aliases declared
                // earlier than their source's assignment in a branch or loop.
                loop {
                    let before = aliases.len();
                    for window in body.windows(3) {
                        if window[1].0 == "="
                            && aliases.contains(&window[2].0)
                            && !aliases.contains(&window[0].0)
                            && window[0]
                                .0
                                .as_bytes()
                                .first()
                                .is_some_and(u8::is_ascii_alphabetic)
                        {
                            aliases.push(window[0].0);
                        }
                    }
                    if aliases.len() == before {
                        break;
                    }
                }
                for (at, word) in body.iter().enumerate() {
                    if !aliases.contains(&word.0) {
                        continue;
                    }
                    let dereference = at > 0
                        && body[at - 1].0 == "*"
                        && (at < 2 || !matches!(body[at - 2].0, "kiln_value" | "kiln_val"));
                    let mut next = at + 1;
                    let indexed = body.get(next).is_some_and(|word| word.0 == "[");
                    if indexed {
                        let mut depth = 1;
                        next += 1;
                        while next < body.len() && depth != 0 {
                            match body[next].0 {
                                "[" => depth += 1,
                                "]" => depth -= 1,
                                _ => {}
                            }
                            next += 1;
                        }
                    }
                    if (dereference || indexed)
                        && body.get(next).is_some_and(|word| {
                            matches!(word.0, "=" | "+=" | "-=" | "|=" | "&=" | "^=" | "++" | "--")
                        })
                    {
                        found.push(word.1);
                    }
                }
            }
            _ => {}
        }
    }
    found
}

#[test]
fn callable_reference_slots_use_store_authorities() {
    let mut failures = Vec::new();
    for (path, source) in reference_sources() {
        for offset in callable_slot_alias_writes(source) {
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
        "callable slot aliases bypass reference-store authorities:\n{}",
        failures.join("\n")
    );
}

#[test]
fn callable_slot_guard_checks_aliases_within_their_function() {
    let source = r#"
        int write_function(void) {
            kiln_value *held = kiln_c99_closure_prototype_slot(closure);
            *held = candidate;
            kiln_value *copied = held;
            *copied = candidate;
            copied[0] = candidate;
        }
        int write_exotic(void) {
            slot = kiln_c99_exotic_store(value);
            *slot = candidate;
        }
        int safe(void) {
            slot = kiln_c99_closure_store_slot(closure);
            kiln_c99_heap_store_value(closure, slot, candidate);
            result = *slot;
        }
        int unrelated(void) { *slot = number; }
    "#;
    assert_eq!(callable_slot_alias_writes(source).len(), 4);
}

#[test]
fn generated_template_and_namespace_values_use_store_authorities() {
    let mut program = super::program(vec![], vec![kiln_ir::Instruction::Return], 0);
    program.template_sites.push(kiln_ir::TemplateSite {
        raw: vec!["raw".into(), "invalid escape".into()],
        cooked: vec![Some("cooked".into()), None],
    });
    let mut source = String::new();
    crate::render_template_sites(&mut source, &program, "store_guard", false);
    crate::render_namespace_wrapper(&mut source, "store_guard");
    assert!(source.contains("raw_slot") && source.contains("namespace_wrapper"));
    assert!(
        direct_reference_writes(&source).is_empty(),
        "generated array storage bypasses the owner authority"
    );
    let words = tokens(&source);
    assert!(
        !words.windows(3).any(|part| part[0].0 == "->"
            && matches!(part[1].0, "value" | "getter" | "setter" | "prototype")
            && part[2].0 == "="),
        "generated descriptor aliases bypass the owner authority"
    );
}
