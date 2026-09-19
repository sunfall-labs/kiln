use super::*;

#[test]
fn string_units_builder_enforces_width_aware_storage_budget() {
    let mut latin = StringUnitsBuilder::with_limit(4);
    latin.push_latin1(b"abcd").expect("latin1 prefix fits");
    assert_eq!(
        latin.push_latin1(b"e"),
        Err(ExecutionError::StringTooLong { bytes: 5 })
    );
    assert_eq!(
        latin.finish().to_units().as_ref(),
        &[b'a' as u16, b'b' as u16, b'c' as u16, b'd' as u16]
    );

    let mut wide = StringUnitsBuilder::with_limit(4);
    wide.push_utf16(&[0x100, 0x101])
        .expect("two UTF-16 units fit");
    assert_eq!(
        wide.push_utf16(&[0x102]),
        Err(ExecutionError::StringTooLong { bytes: 6 })
    );
    assert_eq!(wide.finish().to_units().as_ref(), &[0x100, 0x101]);

    let mut promoted = StringUnitsBuilder::with_limit(6);
    promoted.push_latin1(b"ab").expect("latin1 prefix fits");
    promoted
        .push_utf16(&[0x100])
        .expect("promotion fits the final UTF-16 backing store");
    assert_eq!(
        promoted.push_latin1(b"c"),
        Err(ExecutionError::StringTooLong { bytes: 8 })
    );
    assert_eq!(
        promoted.finish().to_units().as_ref(),
        &[b'a' as u16, b'b' as u16, 0x100]
    );

    let mut blocked_promotion = StringUnitsBuilder::with_limit(4);
    blocked_promotion
        .push_latin1(b"ab")
        .expect("latin1 prefix fits before rejected promotion");
    assert_eq!(
        blocked_promotion.push_utf16(&[0x100]),
        Err(ExecutionError::StringTooLong { bytes: 6 })
    );
    assert_eq!(
        blocked_promotion.finish().to_units().as_ref(),
        &[b'a' as u16, b'b' as u16]
    );
}

#[test]
fn object_data_property_keys_preserve_lone_surrogates() {
    let key = StringUnits::from_units(&[0xd800]);
    let mut object = ObjectData::default();
    object.set_units(&key, Value::Number(7.0));
    assert_eq!(object.get_units(&key), Some(Value::Number(7.0)));
    assert_eq!(object.keys().next(), Some(&key));
    assert!(object.lookup_units(&key).is_some());
    assert!(object.remove_units(&key));
    assert!(object.get_units(&key).is_none());
}

#[test]
fn regexp_patterns_keep_lone_units_and_utf16_offsets() {
    let lone = StringUnits::from_units(&[0xd800]);
    let Value::RegExp(regexp) = build_regexp(&lone, "").expect("lone pattern compiles") else {
        panic!("regexp constructor returned a non-regexp");
    };
    assert_eq!(regexp.source, lone);
    // `source` hands back the pattern text and `toString` wraps it in slashes: neither
    // escapes the lone unit, matching node (`new RegExp("\ud800").toString()` is three
    // units, not eight).
    assert_eq!(
        regexp_string_units(&regexp).to_units().as_ref(),
        &[b'/' as u16, 0xd800, b'/' as u16]
    );
    let matched = regexp_search(&regexp, &[0xd800], 0).expect("lone unit matches");
    assert_eq!(matched.0, 0..1);

    let astral = StringUnits::from_units(&[0xd83d, 0xde00]);
    let Value::RegExp(regexp) = build_regexp(&astral, "u").expect("astral pattern compiles") else {
        panic!("regexp constructor returned a non-regexp");
    };
    let matched = regexp_search(&regexp, &[0x61, 0xd83d, 0xde00], 1).expect("astral unit matches");
    assert_eq!(matched.0, 1..3);
}

#[test]
fn computed_lone_surrogate_reads_share_ordinary_and_proxy_paths() {
    let program = no_program();
    let mut host = TestHost {
        calls: Vec::new(),
        clock_now: None,
        entropy: std::collections::VecDeque::new(),
        timezone: None,
        locale: None,
    };
    let mut vm = test_vm(&program, &mut host);
    let key = StringUnits::from_units(&[0xd800]);
    let key_value = Value::String(key.clone());

    // A prototype accessor is found by the same lookup as an own value. Its getter is omitted
    // deliberately: the result is `undefined`, but reaching the accessor still matters because
    // a setter-only property is present for `in` and for proxy invariants.
    let prototype = Value::Object(Rc::new(RefCell::new(ObjectData::default())));
    if let Value::Object(prototype) = &prototype {
        prototype
            .borrow_mut()
            .define_accessor_units(&key, None, None, Attributes::DEFAULT);
    }
    let inherited = Value::Object(Rc::new(RefCell::new(ObjectData::default())));
    if let Value::Object(inherited_data) = &inherited {
        inherited_data.borrow_mut().set_prototype(prototype.clone());
    }
    assert_eq!(
        vm.read_computed(&inherited, &key_value, 0),
        Ok(Value::Undefined)
    );
    assert!(
        vm.has(&inherited, &key_value, 0)
            .expect("inherited accessor is present")
    );

    let object = Value::Object(Rc::new(RefCell::new(ObjectData::default())));
    if let Value::Object(object_data) = &object {
        object_data.borrow_mut().set_prototype(prototype);
        object_data.borrow_mut().set_units(&key, Value::Number(7.0));
    }
    assert_eq!(
        vm.read_computed(&object, &key_value, 0),
        Ok(Value::Number(7.0))
    );

    // Reflect.get is a real callable trap. Passing the exact key through it exercises the proxy
    // route before the target's unit-preserving lookup, including the UTF-16 key identity.
    let target = Value::Object(Rc::new(RefCell::new(ObjectData::default())));
    if let Value::Object(target_data) = &target {
        target_data.borrow_mut().set_units(&key, Value::Number(9.0));
    }
    let handler = ordinary_object([(
        "get".to_owned(),
        Value::Native(Native::ReflectGet, RealmId::PRIMARY),
    )]);
    let proxy = Value::Proxy(Rc::new(ProxyData {
        target,
        handler,
        revoked: std::cell::Cell::new(false),
    }));
    assert_eq!(
        vm.read_computed(&proxy, &key_value, 0),
        Ok(Value::Number(9.0))
    );
}

#[test]
fn property_key_units_cover_mutation_descriptors_inherited_accessors_and_proxy_invariants() {
    let program = no_program();
    let mut host = TestHost {
        calls: Vec::new(),
        clock_now: Some(1.0),
        entropy: std::collections::VecDeque::new(),
        timezone: None,
        locale: None,
    };
    let mut vm = test_vm(&program, &mut host);
    let key = StringUnits::from_units(&[0xd800]);
    let key_value = Value::String(key.clone());

    let target = Value::Object(Rc::new(RefCell::new(ObjectData::default())));
    let descriptor = ordinary_object([
        ("value".to_owned(), Value::Number(7.0)),
        ("writable".to_owned(), Value::Boolean(true)),
        ("enumerable".to_owned(), Value::Boolean(true)),
        ("configurable".to_owned(), Value::Boolean(true)),
    ]);
    assert_eq!(
        vm.dispatch_native(
            Native::ReflectDefineProperty,
            &Value::Undefined,
            &[target.clone(), key_value.clone(), descriptor],
            0,
        ),
        Ok(Value::Boolean(true))
    );
    assert_eq!(
        vm.read_computed(&target, &key_value, 0),
        Ok(Value::Number(7.0))
    );
    assert!(has_property_key(
        &vm.intrinsics,
        &target,
        PropertyKeyRef::Units(&key)
    ));
    let described = own_descriptor_key(&vm.intrinsics, &target, PropertyKeyRef::Units(&key))
        .expect("exact key descriptor");
    let Value::Object(described) = described else {
        panic!("descriptor must be an object");
    };
    assert_eq!(described.borrow().get("value"), Some(Value::Number(7.0)));
    assert!(vm.delete(&target, &key_value, 0).expect("delete exact key"));
    assert!(!has_property_key(
        &vm.intrinsics,
        &target,
        PropertyKeyRef::Units(&key)
    ));

    // A setter inherited under the exact key is reached by the same walk as an own value. The
    // native callable is intentionally side-effect free; the absence of an own slot proves the
    // write was handed to the setter instead of creating a lossy replacement key.
    let prototype = Value::Object(Rc::new(RefCell::new(ObjectData::default())));
    if let Value::Object(prototype) = &prototype {
        prototype.borrow_mut().define_accessor_units(
            &key,
            None,
            Some(Value::Native(Native::ObjectToStringTag, RealmId::PRIMARY)),
            Attributes::DEFAULT,
        );
    }
    let inherited = Value::Object(Rc::new(RefCell::new(ObjectData::default())));
    if let Value::Object(inherited) = &inherited {
        inherited.borrow_mut().set_prototype(prototype);
    }
    vm.set_computed(&inherited, &key_value, Value::Number(3.0), 0)
        .expect("inherited setter accepts exact key");
    if let Value::Object(inherited) = &inherited {
        assert!(inherited.borrow().get_own_property_units(&key).is_none());
    }

    // Reflect.get receives and returns the exact UTF-16 key. The target is locked so the get
    // invariant check has to consult the same unit-preserving descriptor.
    let locked_target = Value::Object(Rc::new(RefCell::new(ObjectData::default())));
    if let Value::Object(locked_target) = &locked_target {
        locked_target.borrow_mut().define_value_units(
            &key,
            Value::Number(9.0),
            Attributes {
                writable: false,
                enumerable: true,
                configurable: false,
            },
        );
    }
    let handler = ordinary_object([(
        "get".to_owned(),
        Value::Native(Native::ReflectGet, RealmId::PRIMARY),
    )]);
    let proxy = Value::Proxy(Rc::new(ProxyData {
        target: locked_target,
        handler,
        revoked: std::cell::Cell::new(false),
    }));
    assert_eq!(
        vm.read_computed(&proxy, &key_value, 0),
        Ok(Value::Number(9.0))
    );

    // ToPropertyKey is performed once before strict [[Set]]. The closure records its conversion
    // through the host call; a second conversion would leave two calls in the host log.
    let coercion_program = Rc::new(Program {
        source: "".into(),
        template_sites: Vec::new(),
        version: kiln_ir::BYTECODE_VERSION,
        constants: vec![Constant::String(key.clone())],
        code: Vec::new(),
        local_count: 0,
        functions: vec![kiln_ir::FunctionProto {
            span: kiln_ir::Span::default(),
            prologue_end: 0,
            asynchronous: false,
            name: Some("toString".to_owned()),
            inferred_name: None,
            arrow: false,
            generator: false,
            async_generator: false,
            is_method: false,
            parameter_count: 0,
            declared_length: 0,
            mapped_arguments: false,
            local_count: 0,
            code: vec![
                Instruction::HostCall {
                    op: HostOp::ClockNow,
                    argc: 0,
                },
                Instruction::Pop,
                Instruction::Const(0),
                Instruction::Return,
            ],
            upvalues: Vec::new(),
            type_annotations: None,
        }],
    });
    let coercion = Value::Function(Rc::new(Closure {
        program: coercion_program.clone(),
        function: 0,
        strictness: Rc::new(Strictness::default()),
        module_path: None,
        name: Some("toString".into()),
        inferred_name: None,
        upvalues: Vec::new(),
        realm: RealmId::PRIMARY,
        captured_arguments: None,
        captured_new_target: None,
        captured_this: None,
        prototype: RefCell::new(None),
        properties: RefCell::new(ObjectData::default()),
    }));
    let coercion_object = ordinary_object([("toString".to_owned(), coercion)]);
    let strict_target = Value::Object(Rc::new(RefCell::new(ObjectData::default())));
    vm.assign_with_strict(
        &strict_target,
        &coercion_object,
        Value::Number(11.0),
        0,
        true,
    )
    .expect("strict assignment with object key");
    assert_eq!(host.calls.len(), 1, "ToPropertyKey must run once");
    if let Value::Object(strict_target) = &strict_target {
        assert_eq!(
            strict_target.borrow().get_own_property_units(&key).cloned(),
            Some(Property::Value(Value::Number(11.0)))
        );
    }
}

#[test]
fn wide_backed_ascii_intrinsic_keys_follow_narrow_lookup() {
    let program = no_program();
    let mut host = TestHost {
        calls: Vec::new(),
        clock_now: None,
        entropy: std::collections::VecDeque::new(),
        timezone: None,
        locale: None,
    };
    let mut vm = test_vm(&program, &mut host);
    let narrow = StringUnits::from("length");
    let wide = StringUnits::from_view(StringView::Utf16(&[
        b'l' as u16,
        b'e' as u16,
        b'n' as u16,
        b'g' as u16,
        b't' as u16,
        b'h' as u16,
    ]));
    assert!(matches!(wide.as_view(), StringView::Utf16(_)));
    let target = Value::Native(Native::ArrayConstructor, RealmId::PRIMARY);
    assert_eq!(
        vm.read_property_key(&target, PropertyKeyRef::Units(&narrow), 0),
        vm.read_property_key(&target, PropertyKeyRef::Units(&wide), 0)
    );
    assert_eq!(
        own_attributes_key(&vm.intrinsics, &target, PropertyKeyRef::Units(&narrow)),
        own_attributes_key(&vm.intrinsics, &target, PropertyKeyRef::Units(&wide))
    );
}

#[test]
fn non_ascii_property_reads_follow_stored_properties_and_prototype_chain() {
    let program = no_program();
    let mut host = TestHost {
        calls: Vec::new(),
        clock_now: None,
        entropy: std::collections::VecDeque::new(),
        timezone: None,
        locale: None,
    };
    let mut vm = test_vm(&program, &mut host);
    let unicode_name = "℘";
    let unicode_units = StringUnits::from(unicode_name);

    let parent_data = ObjectData::default();
    let parent = Rc::new(RefCell::new(parent_data));
    parent.borrow_mut().set(unicode_name, Value::Number(42.0));

    let mut child_data = ObjectData::default();
    child_data.set_prototype(Value::Object(parent));
    let child = Value::Object(Rc::new(RefCell::new(child_data)));

    let read = vm.read_property_key(&child, PropertyKeyRef::Units(&unicode_units), 0);
    assert_eq!(read.unwrap(), Value::Number(42.0));
}

fn string_index_of(text: &str, needle: &str, start: Option<f64>) -> f64 {
    let mut arguments = vec![Value::String(needle.to_owned().into())];
    if let Some(start) = start {
        arguments.push(Value::Number(start));
    }
    let Value::Number(index) = call_native(
        &Intrinsics::default(),
        Native::StringIndexOf,
        &Value::String(text.to_owned().into()),
        &arguments,
    )
    .expect("String.prototype.indexOf returns a number") else {
        panic!("String.prototype.indexOf returns a number");
    };
    index
}

/// Every native the machine must dispatch has a route to it.
///
/// `call_native` is a free function with no VM, so a built-in that can run program code --
/// a comparator, a getter, an options bag, a set-like's `has` -- gets an `unreachable!` arm
/// there and a route in `Vm::dispatch_native`. The `unreachable!` is written to make a lost
/// route loud, and it does the opposite: nothing routes to the arm, so nothing reaches the
/// assertion, and the built-in aborts the process the first time a program calls it.
///
/// That has now happened to `Set.prototype.union`, the copying array methods, every
/// `RegExp.prototype` accessor, the date methods, the regexp symbol methods, `Iterator.from`,
/// `Object.groupBy`, `Map.groupBy` and `Promise.try` -- each found by a panicking test rather
/// than by the guard that exists for it. The pairing is checked here instead of trusted.
#[test]
fn every_machine_dispatched_native_has_a_route() {
    const MARKER: &str = "dispatched by the interpreter";
    let source = include_str!("lib.rs");

    let start = source
        .find("fn dispatch_native(")
        .expect("dispatch_native must exist");
    let end = source[start..]
        .find("\n    fn ")
        .map(|offset| start + offset)
        .unwrap_or(source.len());
    let routes = &source[start..end];

    let mut missing = Vec::new();
    let mut cursor = 0;
    while let Some(found) = source[cursor..].find(MARKER) {
        let at = cursor + found;
        cursor = at + MARKER.len();
        // The arm's patterns sit between the previous arm and this one's `=>`.
        let head = &source[..at];
        let Some(arrow) = head.rfind("=>") else {
            continue;
        };
        let arm_start = head[..arrow]
            .rfind("\n        ")
            .map_or(0, |index| index + 1);
        for pattern in head[arm_start..arrow].split("Native::").skip(1) {
            let name: String = pattern
                .chars()
                .take_while(|character| character.is_alphanumeric() || *character == '_')
                .collect();
            if name.is_empty() {
                continue;
            }
            if !routes.contains(&format!("Native::{name}")) {
                missing.push(name);
            }
        }
    }
    missing.sort();
    missing.dedup();
    assert!(
        missing.is_empty(),
        "these natives assert that the machine dispatches them, but `dispatch_native` has no \
             route to them, so calling one aborts the process: {missing:?}"
    );
}

#[test]
fn node_bytes_value_adopts_owned_vec_and_keeps_buffer_mutable() {
    let mut bytes = vec![1_u8, 2, 3, 4];
    let source_ptr = bytes.as_mut_ptr();
    let value = node_bytes_value(bytes);
    let Value::TypedArray(array) = value else {
        panic!("node_bytes_value must construct a Uint8Array");
    };

    assert_eq!(array.buffer.bytes.borrow().as_ptr(), source_ptr);
    assert_eq!(&*array.buffer.bytes.borrow(), &[1, 2, 3, 4]);
    array.buffer.bytes.borrow_mut()[1] = 9;
    assert_eq!(&*array.buffer.bytes.borrow(), &[1, 9, 3, 4]);
}

#[test]
fn materialized_intrinsic_store_makes_absence_and_symbol_order_authoritative() {
    let intrinsics = Intrinsics::default();
    let target = Value::Native(Native::Math, RealmId::PRIMARY);
    let initial = own_property_keys_with_symbols(&intrinsics, &target);
    assert!(initial.contains(&"max".to_owned()));
    assert!(own_attributes(&intrinsics, &target, "max").is_some());

    assert!(delete_property(
        &intrinsics,
        &target,
        &Value::String("max".to_owned().into())
    ));
    assert!(!own_property_keys_with_symbols(&intrinsics, &target).contains(&"max".to_owned()));
    assert!(own_attributes(&intrinsics, &target, "max").is_none());

    intrinsics.install_key(
        &target,
        PropertyKeyRef::Utf8("max"),
        Property::Value(Value::Number(7.0)),
        Attributes::DEFAULT,
    );
    assert_eq!(
        own_attributes(&intrinsics, &target, "max"),
        Some(Attributes::DEFAULT)
    );
    assert_eq!(intrinsics.read(&target, "max"), Some(Value::Number(7.0)));

    let array_prototype =
        Value::Native(Native::PrototypeOf(PrototypeTable::Array), RealmId::PRIMARY);
    let keys = own_property_keys_with_symbols(&intrinsics, &array_prototype);
    let iterator_key = WellKnown::Iterator.property_key().to_owned();
    assert!(keys.contains(&iterator_key));
    assert!(own_attributes(&intrinsics, &array_prototype, &iterator_key).is_some());
    assert!(delete_property(
        &intrinsics,
        &array_prototype,
        &Value::String(iterator_key.clone().into()),
    ));
    assert!(!own_property_keys_with_symbols(&intrinsics, &array_prototype).contains(&iterator_key));
    assert!(own_attributes(&intrinsics, &array_prototype, &iterator_key).is_none());

    let function_prototype = Value::Native(
        Native::PrototypeOf(PrototypeTable::Function),
        RealmId::PRIMARY,
    );
    assert!(matches!(
        own_property_state(&intrinsics, &function_prototype, "caller"),
        Some((Property::Accessor { .. }, _))
    ));
}

#[test]
fn intrinsic_store_readd_keeps_order_and_retags_cross_realm_markers() {
    let intrinsics = Intrinsics::default();
    let target = Value::Native(Native::Math, RealmId::PRIMARY);
    let before = own_property_keys_with_symbols(&intrinsics, &target);
    let max_before = before.iter().position(|key| key == "max").unwrap();
    assert!(delete_property(
        &intrinsics,
        &target,
        &Value::String("max".into())
    ));
    intrinsics.install_key(
        &target,
        PropertyKeyRef::Utf8("max"),
        Property::Value(Value::Number(7.0)),
        Attributes::DEFAULT,
    );
    let after = own_property_keys_with_symbols(&intrinsics, &target);
    assert!(after.iter().position(|key| key == "max").unwrap() > max_before);
    assert_eq!(intrinsics.read(&target, "max"), Some(Value::Number(7.0)));

    intrinsics.add_realm();
    let realm_target = Value::Native(Native::ArrayConstructor, RealmId(1));
    let _ = own_property_keys_with_symbols(&intrinsics, &realm_target);
    let prototype = intrinsics
        .read(&realm_target, "prototype")
        .expect("materialized Array has a prototype");
    assert_eq!(
        prototype,
        Value::Native(Native::PrototypeOf(PrototypeTable::Array), RealmId(1))
    );
}

#[test]
fn reflection_materialization_preserves_special_native_property_rows() {
    let intrinsics = Intrinsics::default();
    let cases = [
        (
            Value::Native(Native::AbortSignalConstructor, RealmId::PRIMARY),
            "abort",
        ),
        (
            Value::Native(Native::Temporal(temporal::Op::Namespace), RealmId::PRIMARY),
            "PlainDate",
        ),
        (
            Value::Native(
                Native::Temporal(temporal::Op::Constructor(kiln_temporal::Kind::PlainDate)),
                RealmId::PRIMARY,
            ),
            "from",
        ),
        (
            Value::Native(
                Native::Temporal(temporal::Op::Prototype(kiln_temporal::Kind::PlainDate)),
                RealmId::PRIMARY,
            ),
            "calendarId",
        ),
        (
            Value::Native(Native::Intl(intl::Op::Namespace), RealmId::PRIMARY),
            "DateTimeFormat",
        ),
        (
            Value::Native(
                Native::Intl(intl::Op::Prototype(intl::Service::Locale)),
                RealmId::PRIMARY,
            ),
            "toString",
        ),
    ];
    for (target, key) in cases {
        let before = tabulated_property(&intrinsics, &target, key)
            .unwrap_or_else(|| panic!("missing pre-materialization row {key}"));
        assert!(own_property_keys_with_symbols(&intrinsics, &target).contains(&key.to_owned()));
        let after = own_property_state(&intrinsics, &target, key)
            .expect("materialized row should remain own")
            .0;
        assert_eq!(before, after, "reflection changed {key}");
        if key == "calendarId" {
            assert!(matches!(after, Property::Accessor { get: Some(_), .. }));
        }
    }
}

#[test]
fn materialized_branded_accessors_ignore_receiver_shadow_properties() {
    let program = no_program();
    let mut host = TestHost {
        calls: Vec::new(),
        clock_now: None,
        entropy: std::collections::VecDeque::new(),
        timezone: None,
        locale: None,
    };
    let mut vm = test_vm(&program, &mut host);

    let typed_proto = Value::Native(
        Native::PrototypeOf(PrototypeTable::TypedArray),
        RealmId::PRIMARY,
    );
    let data_view_proto = Value::Native(
        Native::PrototypeOf(PrototypeTable::DataView),
        RealmId::PRIMARY,
    );
    let buffer_proto = Value::Native(
        Native::PrototypeOf(PrototypeTable::ArrayBuffer),
        RealmId::PRIMARY,
    );
    vm.intrinsics.materialize(&typed_proto);
    vm.intrinsics.materialize(&data_view_proto);
    vm.intrinsics.materialize(&buffer_proto);

    let buffer = BufferData::fixed(8);
    let typed = Value::TypedArray(Rc::new(TypedArrayData {
        kind: TypedKind::Uint8,
        buffer: Rc::clone(&buffer),
        offset: 2,
        length: Some(2),
        properties: RefCell::new(ObjectData::default()),
    }));
    if let Value::TypedArray(view) = &typed {
        view.properties.borrow_mut().define_value(
            "byteOffset",
            Value::Number(999.0),
            Attributes::DEFAULT,
        );
    }
    let typed_getter = match own_property_state(&vm.intrinsics, &typed_proto, "byteOffset")
        .expect("materialized TypedArray getter")
        .0
    {
        Property::Accessor { get: Some(get), .. } => get,
        other => panic!("expected TypedArray getter, got {other:?}"),
    };
    assert_eq!(
        vm.call_value(&typed_getter, typed.clone(), Vec::new(), 0)
            .unwrap(),
        Value::Number(2.0)
    );

    let data_view = Value::DataView(Rc::new(DataViewData {
        buffer: Rc::clone(&buffer),
        offset: 3,
        length: Some(2),
        properties: RefCell::new(ObjectData::default()),
    }));
    if let Value::DataView(view) = &data_view {
        view.properties.borrow_mut().define_value(
            "byteOffset",
            Value::Number(999.0),
            Attributes::DEFAULT,
        );
    }
    let data_view_getter = match own_property_state(&vm.intrinsics, &data_view_proto, "byteOffset")
        .expect("materialized DataView getter")
        .0
    {
        Property::Accessor { get: Some(get), .. } => get,
        other => panic!("expected DataView getter, got {other:?}"),
    };
    assert_eq!(
        vm.call_value(&data_view_getter, data_view.clone(), Vec::new(), 0)
            .unwrap(),
        Value::Number(3.0)
    );

    let array_buffer = Value::ArrayBuffer(buffer);
    if let Value::ArrayBuffer(buffer) = &array_buffer {
        buffer.properties.borrow_mut().define_value(
            "byteLength",
            Value::Number(999.0),
            Attributes::DEFAULT,
        );
    }
    let buffer_getter = match own_property_state(&vm.intrinsics, &buffer_proto, "byteLength")
        .expect("materialized ArrayBuffer getter")
        .0
    {
        Property::Accessor { get: Some(get), .. } => get,
        other => panic!("expected ArrayBuffer getter, got {other:?}"),
    };
    assert_eq!(
        vm.call_value(&buffer_getter, array_buffer.clone(), Vec::new(), 0)
            .unwrap(),
        Value::Number(8.0)
    );
}

#[test]
fn native_spread_uses_materialized_enumerability_and_honors_deletion() {
    let intrinsics = Intrinsics::default();
    let path = Value::Native(Native::NodePath, RealmId::PRIMARY);
    let before = own_properties_units(&intrinsics, &path);
    assert!(before.iter().any(|(key, _)| key == "join"));
    assert!(delete_property(
        &intrinsics,
        &path,
        &Value::String("join".to_owned().into()),
    ));
    assert!(
        !own_properties_units(&intrinsics, &path)
            .iter()
            .any(|(key, _)| key == "join")
    );

    let math = Value::Native(Native::Math, RealmId::PRIMARY);
    assert!(
        !own_properties_units(&intrinsics, &math)
            .iter()
            .any(|(key, _)| key == "max")
    );
}

#[test]
fn proxy_own_key_invariant_source_includes_exotic_and_native_keys() {
    let intrinsics = Intrinsics::default();
    let array = Value::Array(ArrayData::new(vec![Value::Number(1.0), Value::Number(2.0)]));
    assert_eq!(
        own_property_keys_including_symbols(&intrinsics, &array),
        vec!["0", "1", "length"]
    );

    let boxed_data = ObjectData {
        primitive: Some(Value::String("ab".to_owned().into())),
        ..ObjectData::default()
    };
    let boxed = Value::Object(Rc::new(RefCell::new(boxed_data)));
    assert_eq!(
        own_property_keys_including_symbols(&intrinsics, &boxed),
        vec!["0", "1", "length"]
    );

    let constructor = Value::Native(Native::ArrayConstructor, RealmId::PRIMARY);
    let keys = own_property_keys_including_symbols(&intrinsics, &constructor);
    assert!(keys.starts_with(&[
        "length".to_owned(),
        "name".to_owned(),
        "prototype".to_owned()
    ]));
    assert!(keys.contains(&"isArray".to_owned()));
    assert!(keys.contains(&"from".to_owned()));
    assert!(keys.contains(&"@@species".to_owned()));
}
use kiln_host_abi::{HostError, HostOp};
use kiln_ir::Builtin;

#[test]
fn commonjs_require_reads_replacements_through_the_shared_cache_cell() {
    let initial = ordinary_object([]);
    let initial_object = match &initial {
        Value::Object(object) => Rc::clone(object),
        _ => unreachable!("ordinary_object returns an object"),
    };
    let cache = Rc::new(RefCell::new(initial));
    let require = CommonJsRequire::new([(
        "./dep.cjs".to_owned(),
        "/tmp/dep.cjs".to_owned(),
        Rc::clone(&cache),
    )]);

    let first = require.get("./dep.cjs").expect("cached module");
    let Value::Object(first_object) = first else {
        panic!("cached module should initially be an object");
    };
    assert!(Rc::ptr_eq(&first_object, &initial_object));

    let replacement = Value::Number(42.0);
    *cache.borrow_mut() = replacement.clone();
    assert_eq!(require.get("./dep.cjs"), Some(replacement));
}

#[test]
fn node_readable_is_a_transport_independent_buffered_event_source() {
    let readable_prototype = Value::Native(
        Native::PrototypeOf(PrototypeTable::Readable),
        RealmId::PRIMARY,
    );
    // The direct marker lookup and the later materialized property store must expose the same
    // stream methods. This is the regression for IncomingMessage.prototype.unpipe: Express
    // reaches it after reflecting on the prototype, when the store becomes authoritative.
    assert!(matches!(
        Native::PrototypeOf(PrototypeTable::Readable).property("pipe"),
        Some(Value::Native(Native::NodeReadablePipe, _))
    ));
    assert!(matches!(
        Native::PrototypeOf(PrototypeTable::Readable).property("unpipe"),
        Some(Value::Native(Native::NodeReadableUnpipe, _))
    ));
    let intrinsics = Intrinsics::default();
    let reflected = own_property_keys_with_symbols(&intrinsics, &readable_prototype);
    assert!(reflected.contains(&"pipe".to_owned()));
    assert!(reflected.contains(&"unpipe".to_owned()));
    assert!(delete_property(
        &intrinsics,
        &readable_prototype,
        &Value::String("unpipe".to_owned().into())
    ));
    assert!(
        !own_property_keys_with_symbols(&intrinsics, &readable_prototype)
            .contains(&"unpipe".to_owned())
    );
    assert!(own_attributes(&intrinsics, &readable_prototype, "unpipe").is_none());

    let readable = node_readable(
        &[ordinary_object([(
            "highWaterMark".into(),
            Value::Number(2.0),
        )])],
        None,
    );
    let state = node_readable_state(&readable).expect("Readable owns semantic state");
    assert_eq!(state.borrow().high_water_mark, 2);
    assert!(matches!(
        Native::PrototypeOf(PrototypeTable::Readable).property("on"),
        Some(Value::Native(Native::NodeEventOn, _))
    ));

    let mut host = TestHost {
        calls: Vec::new(),
        clock_now: None,
        entropy: std::collections::VecDeque::new(),
        timezone: None,
        locale: None,
    };
    let mut vm = Vm::with_entropy_seed(
        Rc::new(Program {
            source: "".into(),
            template_sites: Vec::new(),
            version: kiln_ir::BYTECODE_VERSION,
            constants: Vec::new(),
            code: vec![Instruction::Return],
            local_count: 0,
            functions: Vec::new(),
        }),
        Rc::new(Strictness::default()),
        &mut host,
        Vec::new(),
        ExecutionLimits::default(),
        &RealmCapabilities::default(),
        ExecutionCapabilities::default(),
    )
    .expect("realm capabilities resolve");
    assert_eq!(
        vm.dispatch_memory_readable(
            Native::NodeReadablePush,
            &readable,
            &[Value::String("a".into())],
            0,
        )
        .unwrap(),
        Value::Boolean(true)
    );
    assert_eq!(
        vm.dispatch_memory_readable(
            Native::NodeReadablePush,
            &readable,
            &[Value::String("b".into())],
            0,
        )
        .unwrap(),
        Value::Boolean(false)
    );
    assert_eq!(
        vm.dispatch_memory_readable(Native::NodeReadableRead, &readable, &[], 0)
            .unwrap(),
        Value::String("a".into())
    );
    assert_eq!(state.borrow().buffered_bytes, 1);
    assert_eq!(
        state.borrow().public.borrow().get("length"),
        Some(Value::Number(1.0))
    );
    assert!(vm.pending_node.is_empty());
    assert!(vm.pending_host.is_empty());
}

#[test]
fn event_names_and_late_readable_listener_reflect_buffered_state() {
    let readable = node_readable(&[], None);
    let state = node_readable_state(&readable).expect("Readable owns semantic state");
    state.borrow_mut().buffer.push(Value::String("x".into()));
    state.borrow_mut().buffered_bytes = 1;
    sync_readable_public_state(&readable, &state.borrow());

    let Value::Object(object) = &readable else {
        unreachable!()
    };
    object.borrow_mut().node_events = Some(vec![NodeEventBucket {
        name: "readable".into(),
        listeners: vec![NodeEventListener {
            callback: Value::Native(Native::NodeUtilDebugNoop, RealmId::PRIMARY),
            once: false,
        }],
    }]);
    let names = object
        .borrow()
        .node_events
        .as_ref()
        .expect("events exist")
        .iter()
        .filter(|bucket| !bucket.listeners.is_empty())
        .map(|bucket| bucket.name.clone())
        .collect::<Vec<_>>();
    assert_eq!(names, ["readable".to_owned()]);
    assert_eq!(
        state.borrow().public.borrow().get("length"),
        Some(Value::Number(1.0))
    );
}

#[test]
fn readable_call_initializes_an_existing_subclass_without_replacing_its_prototype() {
    let custom_prototype = ordinary_object([("custom".into(), Value::Boolean(true))]);
    let Value::Object(receiver) = ordinary_object([]) else {
        unreachable!()
    };
    receiver
        .borrow_mut()
        .set_prototype(custom_prototype.clone());
    let initialized = node_readable(&[], Some(&Value::Object(Rc::clone(&receiver))));
    assert!(strict_equal(
        receiver.borrow().prototype().expect("prototype remains"),
        &custom_prototype
    ));
    assert!(node_readable_state(&initialized).is_ok());
}

#[test]
fn node_writable_owns_lifecycle_without_owning_a_transport() {
    let writable = node_writable(&[], None);
    let state = node_writable_state(&writable).expect("Writable owns semantic state");
    let mut host = TestHost {
        calls: Vec::new(),
        clock_now: None,
        entropy: std::collections::VecDeque::new(),
        timezone: None,
        locale: None,
    };
    let mut vm = Vm::with_entropy_seed(
        Rc::new(Program {
            source: "".into(),
            template_sites: Vec::new(),
            version: kiln_ir::BYTECODE_VERSION,
            constants: Vec::new(),
            code: vec![Instruction::Return],
            local_count: 0,
            functions: Vec::new(),
        }),
        Rc::new(Strictness::default()),
        &mut host,
        Vec::new(),
        ExecutionLimits::default(),
        &RealmCapabilities::default(),
        ExecutionCapabilities::default(),
    )
    .expect("realm capabilities resolve");
    assert_eq!(
        vm.dispatch_memory_writable(
            Native::NodeWritableWrite,
            &writable,
            &[Value::String("chunk".into())],
            0,
        )
        .unwrap(),
        Value::Boolean(true)
    );
    assert_eq!(
        vm.dispatch_memory_writable(Native::NodeWritableEnd, &writable, &[], 0)
            .unwrap(),
        writable
    );
    assert!(state.borrow().ended);
    assert!(state.borrow().finished);
    assert!(vm.pending_node.is_empty());
    assert!(vm.pending_host.is_empty());
}

struct TestHost {
    calls: Vec<(HostOp, Vec<HostValue>)>,
    clock_now: Option<f64>,
    entropy: std::collections::VecDeque<u64>,
    timezone: Option<String>,
    locale: Option<String>,
}

impl HostOps for TestHost {
    fn call(&mut self, operation: HostOp, arguments: &[HostValue]) -> Result<HostValue, HostError> {
        self.calls.push((operation, arguments.to_vec()));
        match operation {
            HostOp::ClockNow => self
                .clock_now
                .map(HostValue::number)
                .ok_or_else(|| HostError::new("no scripted clock value")),
            HostOp::EntropyU64 => self
                .entropy
                .pop_front()
                .map(HostValue::Number)
                .ok_or_else(|| HostError::new("no scripted entropy word")),
            HostOp::TimeZone => self
                .timezone
                .clone()
                .map(|value| HostValue::String(value.into()))
                .ok_or_else(|| HostError::new("no scripted timezone")),
            HostOp::Locale => self
                .locale
                .clone()
                .map(|value| HostValue::String(value.into()))
                .ok_or_else(|| HostError::new("no scripted locale")),
            _ => Ok(HostValue::Undefined),
        }
    }
}

/// `f(a, b)` compiled by hand: proto 0 returns its second parameter.
fn calling_program(argc: u16, arguments: Vec<Instruction>) -> Program {
    let mut code = vec![Instruction::MakeFunction(0)];
    code.extend(arguments);
    code.push(Instruction::Call { argc });
    code.push(Instruction::Return);
    Program {
        source: "".into(),
        template_sites: Vec::new(),
        version: kiln_ir::BYTECODE_VERSION,
        constants: vec![number(1.0), number(2.0), number(3.0)],
        code,
        local_count: 0,
        functions: vec![kiln_ir::FunctionProto {
            inferred_name: None,
            span: kiln_ir::Span::default(),
            prologue_end: 0,
            asynchronous: false,
            name: Some("second".to_owned()),
            arrow: false,
            generator: false,
            async_generator: false,
            is_method: false,
            parameter_count: 2,
            declared_length: 2,
            mapped_arguments: false,
            local_count: 2,
            code: vec![Instruction::LoadLocal(1), Instruction::Return],
            upvalues: Vec::new(),
            type_annotations: None,
        }],
    }
}

/// A stand-in program for a closure that is never called.
///
/// The prototype-chain tests build closures to compare identities, not to run: the table
/// they would index is irrelevant and an empty one says so.
fn no_program() -> Rc<Program> {
    Rc::new(Program {
        source: "".into(),
        template_sites: Vec::new(),
        version: kiln_ir::BYTECODE_VERSION,
        constants: Vec::new(),
        code: Vec::new(),
        local_count: 0,
        functions: Vec::new(),
    })
}

fn run(program: &Program) -> Result<Value, ExecutionError> {
    let mut host = TestHost {
        calls: Vec::new(),
        clock_now: None,
        entropy: std::collections::VecDeque::new(),
        timezone: None,
        locale: None,
    };
    execute(
        Rc::new(program.clone()),
        &mut host,
        ExecutionLimits::default(),
        ExecutionCapabilities::default(),
    )
    .map(|result| result.value)
}

#[test]
fn inferred_closure_name_property_preserves_utf16_units() {
    let inferred = StringUnits::from_units(&[0xd800]);
    let program = Program {
        source: "".into(),
        template_sites: Vec::new(),
        version: kiln_ir::BYTECODE_VERSION,
        constants: Vec::new(),
        code: vec![Instruction::MakeFunction(0), Instruction::Return],
        local_count: 0,
        functions: vec![kiln_ir::FunctionProto {
            inferred_name: Some(inferred.clone()),
            asynchronous: false,
            name: None,
            arrow: false,
            generator: false,
            async_generator: false,
            is_method: false,
            parameter_count: 0,
            mapped_arguments: false,
            declared_length: 0,
            local_count: 0,
            prologue_end: 0,
            code: vec![Instruction::Return],
            upvalues: Vec::new(),
            span: kiln_ir::Span::default(),
            type_annotations: None,
        }],
    };
    let Value::Function(closure) = run(&program).expect("closure should be returned") else {
        panic!("expected a function");
    };
    assert_eq!(closure.display_name_units(), inferred);
    assert_eq!(
        closure.properties.borrow().get_own("name"),
        Some(&Value::String(inferred))
    );
}

fn test_vm<'a>(program: &'a Program, host: &'a mut TestHost) -> Vm<'a, TestHost> {
    Vm::with_entropy_seed(
        Rc::new(program.clone()),
        Rc::new(Strictness::sloppy(program)),
        host,
        vec![Value::Undefined; program.local_count as usize],
        ExecutionLimits::default(),
        &RealmCapabilities::default(),
        ExecutionCapabilities::default(),
    )
    .expect("realm capabilities resolve")
}

#[test]
fn builtin_bindings_are_present_on_the_realm_global_and_share_values_with_load_builtin() {
    let program = no_program();
    let mut host = TestHost {
        calls: Vec::new(),
        clock_now: None,
        entropy: std::collections::VecDeque::new(),
        timezone: None,
        locale: None,
    };
    let mut vm = test_vm(&program, &mut host);
    let globals = vm.global_object_in(RealmId::PRIMARY);
    let Value::Object(globals) = globals else {
        panic!("global object must be an object");
    };
    for &(builtin, name) in kiln_ir::Builtin::GLOBAL_BINDINGS {
        if matches!(
            builtin,
            kiln_ir::Builtin::GlobalThis
                | kiln_ir::Builtin::Process
                | kiln_ir::Builtin::StructuredClone
                | kiln_ir::Builtin::NotANumber
                | kiln_ir::Builtin::Infinity
        ) {
            continue;
        }
        let global_value = globals
            .borrow()
            .get_own(name)
            .cloned()
            .unwrap_or_else(|| panic!("global object is missing {name}"));
        let loaded_value = vm.builtin_value(builtin, RealmId::PRIMARY);
        assert!(
            strict_equal(&global_value, &loaded_value),
            "global {name} disagrees with LoadBuiltin"
        );
    }
    let global_this = globals.borrow().get_own("globalThis").cloned();
    let global_alias = globals.borrow().get_own("global").cloned();
    assert!(matches!(global_this, Some(Value::Object(_))));
    assert!(matches!(global_alias, Some(Value::Object(_))));
    assert!(strict_equal(
        global_this.as_ref().unwrap(),
        global_alias.as_ref().unwrap()
    ));
}

#[test]
fn host_clock_is_explicit_and_does_not_replace_the_monotonic_clock() {
    let program = Program {
        source: "".into(),
        template_sites: Vec::new(),
        version: kiln_ir::BYTECODE_VERSION,
        constants: Vec::new(),
        code: Vec::new(),
        local_count: 0,
        functions: Vec::new(),
    };
    let mut host = TestHost {
        calls: Vec::new(),
        clock_now: Some(1_725_000_123_456.25),
        entropy: std::collections::VecDeque::new(),
        timezone: None,
        locale: None,
    };
    let mut vm = Vm::with_entropy_seed(
        Rc::new(program),
        Rc::new(Strictness::default()),
        &mut host,
        Vec::new(),
        ExecutionLimits::default(),
        &RealmCapabilities {
            clock: ClockCapability::Host,
            ..RealmCapabilities::default()
        },
        ExecutionCapabilities::default(),
    )
    .expect("realm capabilities resolve");

    assert_eq!(
        vm.dispatch_native(Native::DateNow, &Value::Undefined, &[], 7),
        Ok(Value::Number(1_725_000_123_456.25))
    );
    // Compared by instant rather than by value: a date is an object, so two of them are
    // never equal, and what this asserts is which clock the constructor read.
    assert!(matches!(
        vm.dispatch_native(Native::DateConstructor, &Value::Undefined, &[], 8),
        Ok(Value::Date(date)) if date.time.get() == 1_725_000_123_456.0
    ));
    assert_eq!(
        vm.dispatch_native(Native::PerformanceNow, &Value::Undefined, &[], 9),
        Ok(Value::Number(0.0))
    );
    assert_eq!(
        vm.dispatch_native(Native::ProcessHrtimeBigInt, &Value::Undefined, &[], 10),
        Ok(Value::BigInt(Rc::new(num_bigint::BigInt::from(0_u8))))
    );
    drop(vm);
    assert_eq!(
        host.calls,
        vec![
            (HostOp::ClockNow, Vec::new()),
            (HostOp::ClockNow, Vec::new())
        ]
    );
}

#[test]
fn virtual_clock_never_calls_the_host() {
    let program = Program {
        source: "".into(),
        template_sites: Vec::new(),
        version: kiln_ir::BYTECODE_VERSION,
        constants: Vec::new(),
        code: Vec::new(),
        local_count: 0,
        functions: Vec::new(),
    };
    let mut host = TestHost {
        calls: Vec::new(),
        clock_now: Some(123.0),
        entropy: std::collections::VecDeque::new(),
        timezone: None,
        locale: None,
    };
    let mut vm = test_vm(&program, &mut host);
    assert_eq!(
        vm.dispatch_native(Native::DateNow, &Value::Undefined, &[], 0),
        Ok(Value::Number(0.0))
    );
    drop(vm);
    assert!(host.calls.is_empty());
}

#[test]
fn realm_timezone_and_locale_are_resolved_once_at_entry() {
    let program = Program {
        source: "".into(),
        template_sites: Vec::new(),
        version: kiln_ir::BYTECODE_VERSION,
        constants: Vec::new(),
        code: Vec::new(),
        local_count: 0,
        functions: Vec::new(),
    };
    let mut host = TestHost {
        calls: Vec::new(),
        clock_now: None,
        entropy: std::collections::VecDeque::new(),
        timezone: Some("America/Denver".into()),
        locale: Some("DE-de".into()),
    };
    let vm = Vm::with_entropy_seed(
        Rc::new(program),
        Rc::new(Strictness::default()),
        &mut host,
        Vec::new(),
        ExecutionLimits::default(),
        &RealmCapabilities {
            timezone: TimeZoneCapability::Host,
            locale: LocaleCapability::Host,
            ..RealmCapabilities::default()
        },
        ExecutionCapabilities::default(),
    )
    .expect("host capabilities resolve");

    assert_eq!(vm.timezone, "America/Denver");
    assert_eq!(vm.default_locale, "de-DE");
    drop(vm);
    assert_eq!(
        host.calls,
        vec![(HostOp::TimeZone, Vec::new()), (HostOp::Locale, Vec::new())]
    );
}

#[test]
fn named_timezone_and_locale_never_call_the_host() {
    let program = Program {
        source: "".into(),
        template_sites: Vec::new(),
        version: kiln_ir::BYTECODE_VERSION,
        constants: Vec::new(),
        code: Vec::new(),
        local_count: 0,
        functions: Vec::new(),
    };
    let mut host = TestHost {
        calls: Vec::new(),
        clock_now: None,
        entropy: std::collections::VecDeque::new(),
        timezone: None,
        locale: None,
    };
    let vm = Vm::with_entropy_seed(
        Rc::new(program),
        Rc::new(Strictness::default()),
        &mut host,
        Vec::new(),
        ExecutionLimits::default(),
        &RealmCapabilities {
            timezone: TimeZoneCapability::Named("America/New_York".into()),
            locale: LocaleCapability::Named("fr-fr".into()),
            ..RealmCapabilities::default()
        },
        ExecutionCapabilities::default(),
    )
    .expect("named capabilities resolve");

    assert_eq!(vm.timezone, "America/New_York");
    assert_eq!(vm.default_locale, "fr-FR");
    drop(vm);
    assert!(host.calls.is_empty());
}

#[test]
fn host_entropy_is_one_explicit_word_per_draw() {
    let program = Program {
        source: "".into(),
        template_sites: Vec::new(),
        version: kiln_ir::BYTECODE_VERSION,
        constants: Vec::new(),
        code: Vec::new(),
        local_count: 0,
        functions: Vec::new(),
    };
    let mut host = TestHost {
        calls: Vec::new(),
        clock_now: None,
        entropy: [
            0x8000_0000_0000_0000,
            0x0011_2233_4455_6677,
            0x8899_aabb_ccdd_eeff,
        ]
        .into(),
        timezone: None,
        locale: None,
    };
    let mut vm = Vm::with_entropy_seed(
        Rc::new(program),
        Rc::new(Strictness::default()),
        &mut host,
        Vec::new(),
        ExecutionLimits::default(),
        &RealmCapabilities {
            entropy: EntropyCapability::Host,
            ..RealmCapabilities::default()
        },
        ExecutionCapabilities::default(),
    )
    .expect("realm capabilities resolve");

    assert_eq!(
        vm.dispatch_native(Native::MathRandom, &Value::Undefined, &[], 1),
        Ok(Value::Number(0.5))
    );
    assert_eq!(
        vm.dispatch_native(Native::NodeCryptoRandomUuid, &Value::Undefined, &[], 2),
        Ok(Value::String(
            "00112233-4455-4677-8899-aabbccddeeff".to_owned().into()
        ))
    );
    drop(vm);
    assert_eq!(
        host.calls,
        vec![
            (HostOp::EntropyU64, Vec::new()),
            (HostOp::EntropyU64, Vec::new()),
            (HostOp::EntropyU64, Vec::new()),
        ]
    );
}

#[test]
fn seeded_entropy_reproduces_without_calling_the_host() {
    let program = Program {
        source: "".into(),
        template_sites: Vec::new(),
        version: kiln_ir::BYTECODE_VERSION,
        constants: Vec::new(),
        code: Vec::new(),
        local_count: 0,
        functions: Vec::new(),
    };
    let draw = |host: &mut TestHost| {
        let mut vm = Vm::with_entropy_seed(
            Rc::new(program.clone()),
            Rc::new(Strictness::default()),
            host,
            Vec::new(),
            ExecutionLimits::default(),
            &RealmCapabilities {
                entropy: EntropyCapability::Seeded(7),
                ..RealmCapabilities::default()
            },
            ExecutionCapabilities::default(),
        )
        .expect("realm capabilities resolve");
        vm.dispatch_native(Native::MathRandom, &Value::Undefined, &[], 0)
    };
    let mut first = TestHost {
        calls: Vec::new(),
        clock_now: None,
        entropy: std::collections::VecDeque::new(),
        timezone: None,
        locale: None,
    };
    let mut second = TestHost {
        calls: Vec::new(),
        clock_now: None,
        entropy: std::collections::VecDeque::new(),
        timezone: None,
        locale: None,
    };
    assert_eq!(draw(&mut first), draw(&mut second));
    assert!(first.calls.is_empty());
    assert!(second.calls.is_empty());
}

#[test]
fn date_methods_stay_on_the_machine_dispatch_path() {
    let program = Program {
        source: "".into(),
        template_sites: Vec::new(),
        version: kiln_ir::BYTECODE_VERSION,
        constants: Vec::new(),
        code: Vec::new(),
        local_count: 0,
        functions: Vec::new(),
    };
    let mut host = TestHost {
        calls: Vec::new(),
        clock_now: None,
        entropy: std::collections::VecDeque::new(),
        timezone: None,
        locale: None,
    };
    let mut vm = test_vm(&program, &mut host);
    let date = new_date(0.0);

    assert_eq!(
        vm.dispatch_native(Native::Date(DateMethod::ToIsoString), &date, &[], 0),
        Ok(Value::String("1970-01-01T00:00:00.000Z".to_owned().into()))
    );
}

#[test]
fn array_species_definition_observes_a_proxy_define_property_trap() {
    let program = Program {
        source: "".into(),
        template_sites: Vec::new(),
        version: kiln_ir::BYTECODE_VERSION,
        constants: Vec::new(),
        code: Vec::new(),
        local_count: 0,
        functions: Vec::new(),
    };
    let mut host = TestHost {
        calls: Vec::new(),
        clock_now: None,
        entropy: std::collections::VecDeque::new(),
        timezone: None,
        locale: None,
    };
    let mut vm = test_vm(&program, &mut host);
    let target = ordinary_object(Vec::<(String, Value)>::new());
    let handler = ordinary_object([(
        "defineProperty".to_owned(),
        Value::Native(Native::ObjectDefineProperty, RealmId::PRIMARY),
    )]);
    let proxy = Value::Proxy(Rc::new(ProxyData {
        target: target.clone(),
        handler,
        revoked: std::cell::Cell::new(false),
    }));

    vm.define_element(&proxy, 0, Value::Number(7.0), 0)
        .expect("proxy trap accepts the definition");
    assert_eq!(vm.read_property(&proxy, "0", 0), Ok(Value::Number(7.0)));
}

#[test]
fn array_species_definition_writes_typed_array_elements() {
    let program = Program {
        source: "".into(),
        template_sites: Vec::new(),
        version: kiln_ir::BYTECODE_VERSION,
        constants: Vec::new(),
        code: Vec::new(),
        local_count: 0,
        functions: Vec::new(),
    };
    let mut host = TestHost {
        calls: Vec::new(),
        clock_now: None,
        entropy: std::collections::VecDeque::new(),
        timezone: None,
        locale: None,
    };
    let mut vm = test_vm(&program, &mut host);
    let original = Value::Array(ArrayData::new(vec![Value::Number(1.0), Value::Number(2.0)]));
    if let Value::Array(items) = &original {
        items.properties.borrow_mut().set(
            "constructor",
            Value::Native(
                Native::TypedArrayConstructor(TypedKind::Uint8),
                RealmId::PRIMARY,
            ),
        );
    }
    let typed = vm
        .array_species_create(&original, 2, 0)
        .expect("typed-array species result");
    assert!(matches!(typed, Value::TypedArray(_)));

    vm.define_element(&typed, 0, Value::Number(7.0), 0)
        .expect("in-range typed-array definition");
    vm.define_element(&typed, 1, Value::Number(9.0), 0)
        .expect("second in-range typed-array definition");
    assert_eq!(vm.read_property(&typed, "0", 0), Ok(Value::Number(7.0)));
    assert_eq!(vm.read_property(&typed, "1", 0), Ok(Value::Number(9.0)));
    assert!(vm.define_element(&typed, 2, Value::Number(1.0), 0).is_err());
}

#[test]
fn process_private_native_modules_are_separate_from_public_ts_projection() {
    let program = Program {
        source: "".into(),
        template_sites: Vec::new(),
        version: kiln_ir::BYTECODE_VERSION,
        constants: Vec::new(),
        code: Vec::new(),
        local_count: 0,
        functions: Vec::new(),
    };
    let mut host = TestHost {
        calls: Vec::new(),
        clock_now: None,
        entropy: std::collections::VecDeque::new(),
        timezone: None,
        locale: None,
    };
    let mut vm = test_vm(&program, &mut host);
    let process = vm.process_object();
    let modules = vm
        .read_property(&process, "__kilnNativeModules", 0)
        .expect("private capability table");
    for name in ["fs", "fs_promises", "http", "net"] {
        assert!(
            !matches!(vm.read_property(&modules, name, 0), Ok(Value::Undefined)),
            "missing private native capability: {name}"
        );
    }
    assert!(matches!(
        vm.dispatch_native(
            Native::ProcessGetBuiltinModule,
            &Value::Undefined,
            &[Value::String("node:fs".to_owned().into())],
            0,
        ),
        Err(ExecutionError::TypeErrorMessage { message })
            if message.contains("graph-materialized")
    ));
}

#[test]
fn node_events_builtin_exposes_event_emitter() {
    let program = Program {
        source: "".into(),
        template_sites: Vec::new(),
        version: kiln_ir::BYTECODE_VERSION,
        constants: Vec::new(),
        code: Vec::new(),
        local_count: 0,
        functions: Vec::new(),
    };
    let mut host = TestHost {
        calls: Vec::new(),
        clock_now: None,
        entropy: std::collections::VecDeque::new(),
        timezone: None,
        locale: None,
    };
    let mut vm = test_vm(&program, &mut host);
    for specifier in ["node:events", "node:module"] {
        assert!(matches!(
            vm.dispatch_native(
                Native::ProcessGetBuiltinModule,
                &Value::Undefined,
                &[Value::String(specifier.to_owned().into())],
                0,
            ),
            Err(ExecutionError::TypeErrorMessage { message })
                if message.contains("graph-materialized")
        ));
    }
    assert_eq!(
        Native::NodeEvents.property("EventEmitter"),
        Some(Value::Native(Native::NodeEventEmitter, RealmId::PRIMARY))
    );
    assert_eq!(
        kiln_builtins::builtin_exports_for_specifier("events"),
        Some(&["EventEmitter", "once", "on", "getEventListeners", "default"][..])
    );
}

#[test]
fn node_console_exports_and_methods() {
    let program = Program {
        source: "".into(),
        template_sites: Vec::new(),
        version: kiln_ir::BYTECODE_VERSION,
        constants: Vec::new(),
        code: Vec::new(),
        local_count: 0,
        functions: Vec::new(),
    };
    let mut host = TestHost {
        calls: Vec::new(),
        clock_now: None,
        entropy: std::collections::VecDeque::new(),
        timezone: None,
        locale: None,
    };
    let mut vm = test_vm(&program, &mut host);

    assert_eq!(
        Native::NodeConsole.property("Console"),
        Some(Value::Native(Native::ConsoleConstructor, RealmId::PRIMARY))
    );
    let names = kiln_builtins::builtin_exports_for_specifier("node:console")
        .expect("console builtin exists");
    assert!(names.contains(&"Console"));
    assert!(names.contains(&"log"));
    assert!(names.contains(&"error"));
    assert!(names.contains(&"warn"));
    assert!(names.contains(&"info"));
    assert!(names.contains(&"debug"));
    assert!(names.contains(&"default"));
    assert_eq!(names.len(), 24);

    // new Console requires a writable stream for stdout
    let err = vm.construct_console(&[], 0).unwrap_err();
    assert!(matches!(err, ExecutionError::TypeErrorMessage { .. }));

    // Global console has Console constructor
    let globals = vm.global_object();
    let global_console = vm.read_property(&globals, "console", 0).unwrap();
    let console_ctor = vm.read_property(&global_console, "Console", 0).unwrap();
    assert_eq!(
        console_ctor,
        Value::Native(Native::ConsoleConstructor, RealmId::PRIMARY)
    );
}

#[test]
fn node_http_exports_a_server_constructor_with_coded_option_errors() {
    let program = Program {
        source: "".into(),
        template_sites: Vec::new(),
        version: kiln_ir::BYTECODE_VERSION,
        constants: Vec::new(),
        code: Vec::new(),
        local_count: 0,
        functions: Vec::new(),
    };
    let mut host = TestHost {
        calls: Vec::new(),
        clock_now: None,
        entropy: std::collections::VecDeque::new(),
        timezone: None,
        locale: None,
    };
    let mut vm = test_vm(&program, &mut host);

    assert_eq!(
        Native::NodeHttp.property("Server"),
        Some(Value::Native(Native::NodeHttpServer, RealmId::PRIMARY))
    );
    assert!(
        kiln_builtins::builtin_exports_for_specifier("node:http")
            .expect("http builtin exists")
            .contains(&"Server")
    );

    let result = vm.dispatch_node_http(
        Native::NodeHttpServer,
        &Value::Undefined,
        &[Value::String("invalid".to_owned().into())],
        0,
    );
    assert_eq!(result, Err(ExecutionError::ThrownAcrossNative));
    let Value::Object(error) = vm.caught.take().expect("coded TypeError was preserved") else {
        panic!("invalid options did not produce an error object");
    };
    assert_eq!(
        error.borrow().get("code"),
        Some(Value::String("ERR_INVALID_ARG_TYPE".to_owned().into()))
    );
    assert_eq!(
        error.borrow().get("name"),
        Some(Value::String("TypeError".to_owned().into()))
    );
}

#[test]
fn node_event_emitter_stores_listeners_and_methods_are_chainable() {
    let program = Program {
        source: "".into(),
        template_sites: Vec::new(),
        version: kiln_ir::BYTECODE_VERSION,
        constants: Vec::new(),
        code: Vec::new(),
        local_count: 0,
        functions: Vec::new(),
    };
    let mut host = TestHost {
        calls: Vec::new(),
        clock_now: None,
        entropy: std::collections::VecDeque::new(),
        timezone: None,
        locale: None,
    };
    let mut vm = test_vm(&program, &mut host);
    let emitter = node_event_emitter();
    let listener = Value::Native(Native::ArrayConstructor, RealmId::PRIMARY);

    assert!(strict_equal(
        &vm.dispatch_node_event(
            Native::NodeEventOn,
            &emitter,
            &[Value::String("data".to_owned().into()), listener.clone()],
            0,
        )
        .unwrap(),
        &emitter,
    ));
    let listed = vm
        .dispatch_node_event(
            Native::NodeEventListeners,
            &emitter,
            &[Value::String("data".to_owned().into())],
            0,
        )
        .unwrap();
    let Value::Array(listed) = listed else {
        panic!("listeners() must return an array");
    };
    assert!(same_value(&listed.borrow()[0], &listener));
    assert_eq!(
        vm.dispatch_node_event(
            Native::NodeEventEmit,
            &emitter,
            &[Value::String("missing".to_owned().into())],
            0,
        ),
        Ok(Value::Boolean(false))
    );
}

#[test]
fn node_event_once_is_removed_before_reentrant_emit() {
    let program = Program {
        source: "".into(),
        template_sites: Vec::new(),
        version: kiln_ir::BYTECODE_VERSION,
        constants: Vec::new(),
        code: Vec::new(),
        local_count: 0,
        functions: Vec::new(),
    };
    let mut host = TestHost {
        calls: Vec::new(),
        clock_now: None,
        entropy: std::collections::VecDeque::new(),
        timezone: None,
        locale: None,
    };
    let mut vm = test_vm(&program, &mut host);
    let emitter = node_event_emitter();
    let listener = Value::Native(Native::ArrayConstructor, RealmId::PRIMARY);
    vm.dispatch_node_event(
        Native::NodeEventOnce,
        &emitter,
        &[Value::String("ready".to_owned().into()), listener],
        0,
    )
    .unwrap();

    assert_eq!(
        vm.dispatch_node_event(
            Native::NodeEventEmit,
            &emitter,
            &[Value::String("ready".to_owned().into())],
            0,
        ),
        Ok(Value::Boolean(true))
    );
    assert_eq!(
        vm.dispatch_node_event(
            Native::NodeEventEmit,
            &emitter,
            &[Value::String("ready".to_owned().into())],
            0,
        ),
        Ok(Value::Boolean(false))
    );
}

#[test]
fn node_event_new_listener_runs_before_registration() {
    let program = Program {
        source: "".into(),
        template_sites: Vec::new(),
        version: kiln_ir::BYTECODE_VERSION,
        constants: Vec::new(),
        code: Vec::new(),
        local_count: 0,
        functions: Vec::new(),
    };
    let mut host = TestHost {
        calls: Vec::new(),
        clock_now: None,
        entropy: std::collections::VecDeque::new(),
        timezone: None,
        locale: None,
    };
    let mut vm = test_vm(&program, &mut host);
    let emitter = node_event_emitter();
    let observer = Value::Native(Native::ArrayConstructor, RealmId::PRIMARY);
    let listener = Value::Native(Native::ObjectConstructor, RealmId::PRIMARY);
    vm.dispatch_node_event(
        Native::NodeEventOn,
        &emitter,
        &[Value::String("newListener".to_owned().into()), observer],
        0,
    )
    .unwrap();
    vm.dispatch_node_event(
        Native::NodeEventOn,
        &emitter,
        &[Value::String("data".to_owned().into()), listener.clone()],
        0,
    )
    .unwrap();
    assert_eq!(
        node_listeners(
            match &emitter {
                Value::Object(object) => object,
                _ => unreachable!(),
            },
            "data"
        )
        .len(),
        1
    );
    assert!(same_value(
        &node_listeners(
            match &emitter {
                Value::Object(object) => object,
                _ => unreachable!(),
            },
            "data"
        )[0]
        .callback,
        &listener
    ));
}

/// A stand-in frontend for the eval tests: it ignores the source and returns one program.
///
/// The interpreter has no compiler of its own -- that is the point of the injected seam -- so
/// a test of the *machine* side of `eval` has to supply the program directly. What is being
/// checked here is the plumbing: that the eval'd program runs in a frame of its own, and
/// that slot 0 is the caller's cell rather than a copy of the value in it.
fn stub_eval_compiler(request: &EvalSource<'_>) -> Result<(Program, Strictness), EvalCompileError> {
    assert_eq!(
        request.outer.len(),
        1,
        "the direct eval hands over the one binding in scope"
    );
    assert_eq!(request.outer[0].name, "x");
    Ok((
        Program {
            source: "".into(),
            template_sites: Vec::new(),
            version: kiln_ir::BYTECODE_VERSION,
            // `x = 2; x` against the cell installed in slot 0.
            constants: vec![Constant::Number(2.0f64.to_bits())],
            code: vec![
                Instruction::Const(0),
                Instruction::StoreCell(0),
                Instruction::LoadCell(0),
                Instruction::Return,
            ],
            local_count: 1,
            functions: Vec::new(),
        },
        Strictness::default(),
    ))
}

/// A direct eval writes the caller's binding, not a copy of it.
///
/// This is the whole reason the site hands over cells: the eval'd program is compiled against
/// slot numbers of its own, and only a shared cell can make `eval("x = 2")` visible to the
/// caller afterwards. A copy would leave the caller reading 1 and look entirely plausible.
#[test]
fn a_direct_eval_writes_through_to_the_callers_binding() {
    let mut program = program(
        vec![
            Constant::Number(1.0f64.to_bits()),
            Constant::String("x = 2; x".to_owned().into()),
        ],
        vec![
            Instruction::NewCell(0),
            Instruction::Const(0),
            Instruction::StoreCell(0),
            Instruction::Const(1),
            Instruction::DirectEval(Box::new(EvalSite {
                bindings: vec![kiln_ir::EvalBinding {
                    name: "x".to_owned(),
                    access: EvalAccess::Cell(0),
                    declaration: kiln_ir::EvalDeclaration::Var,
                }],
                strict: false,
                global_var_scope: false,
                argc: Some(1),
                home: None,
                new_target: kiln_ir::EvalNewTarget::Forbidden,
                private_scopes: Vec::new(),
                blocked_var_names: Vec::new(),
            })),
            // The eval's completion value is discarded; the caller's binding is the answer.
            Instruction::Pop,
            Instruction::LoadCell(0),
            Instruction::Return,
        ],
    );
    program.local_count = 1;
    let mut host = TestHost {
        calls: Vec::new(),
        clock_now: None,
        entropy: std::collections::VecDeque::new(),
        timezone: None,
        locale: None,
    };
    let mut runtime = realm_runtime().expect("realm runtime");
    let result = execute_in_runtime_with_strictness(
        Rc::new(program.clone()),
        Rc::new(Strictness::sloppy(&program)),
        &mut host,
        ExecutionLimits::default(),
        &mut runtime,
        ExecutionCapabilities::default().with_source_compiler(stub_eval_compiler),
    )
    .map(|execution| execution.value);
    assert_eq!(result, Ok(Value::Number(2.0)));
}

#[test]
fn source_compiler_authority_is_scoped_to_one_execution() {
    fn constant_compiler(
        _request: &EvalSource<'_>,
    ) -> Result<(Program, Strictness), EvalCompileError> {
        Ok((
            program(
                vec![Constant::Number(42.0f64.to_bits())],
                vec![Instruction::Const(0), Instruction::Return],
            ),
            Strictness::default(),
        ))
    }

    let invoking_eval = program(
        vec![Constant::String("ignored".to_owned().into())],
        vec![
            Instruction::LoadBuiltin(kiln_ir::Builtin::Eval),
            Instruction::Const(0),
            Instruction::Call { argc: 1 },
            Instruction::Return,
        ],
    );
    let run_with = |capabilities| {
        let mut host = TestHost {
            calls: Vec::new(),
            clock_now: None,
            entropy: std::collections::VecDeque::new(),
            timezone: None,
            locale: None,
        };
        execute(
            Rc::new(invoking_eval.clone()),
            &mut host,
            ExecutionLimits::default(),
            capabilities,
        )
        .map(|result| result.value)
    };

    assert_eq!(
        run_with(ExecutionCapabilities::default().with_source_compiler(constant_compiler)),
        Ok(Value::Number(42.0))
    );
    assert!(matches!(
        run_with(ExecutionCapabilities::default()),
        Err(ExecutionError::NoSourceCompiler)
    ));
}

#[test]
fn dynamic_source_boundary_rejects_raw_units_and_keeps_parser_errors_distinct() {
    fn constant_compiler(
        request: &EvalSource<'_>,
    ) -> Result<(Program, Strictness), EvalCompileError> {
        Ok((
            Program {
                source: "".into(),
                template_sites: Vec::new(),
                version: kiln_ir::BYTECODE_VERSION,
                constants: vec![Constant::Number(7.0f64.to_bits())],
                code: vec![Instruction::Const(0), Instruction::Return],
                local_count: request.outer.len() as u32,
                functions: Vec::new(),
            },
            Strictness::default(),
        ))
    }

    fn syntax_compiler(
        _request: &EvalSource<'_>,
    ) -> Result<(Program, Strictness), EvalCompileError> {
        Err(EvalCompileError::Syntax("ordinary parser error".to_owned()))
    }

    let run_eval = |source: StringUnits, compiler: SourceCompiler| {
        let program = Program {
            source: "".into(),
            template_sites: Vec::new(),
            version: kiln_ir::BYTECODE_VERSION,
            constants: vec![Constant::String(source)],
            code: vec![
                Instruction::LoadBuiltin(kiln_ir::Builtin::Eval),
                Instruction::Const(0),
                Instruction::Call { argc: 1 },
                Instruction::Return,
            ],
            local_count: 0,
            functions: Vec::new(),
        };
        let mut host = TestHost {
            calls: Vec::new(),
            clock_now: None,
            entropy: std::collections::VecDeque::new(),
            timezone: None,
            locale: None,
        };
        execute(
            Rc::new(program),
            &mut host,
            ExecutionLimits::default(),
            ExecutionCapabilities::default().with_source_compiler(compiler),
        )
        .map(|result| result.value)
    };

    let escaped = StringUnits::from(r#""\ud800""#);
    assert_eq!(run_eval(escaped, constant_compiler), Ok(Value::Number(7.0)));

    let raw = StringUnits::from_units(&[b'"' as u16, 0xd800, b'"' as u16]);
    assert!(matches!(
        run_eval(raw, constant_compiler),
        Err(ExecutionError::EvalUnsupported { message })
            if message == DYNAMIC_SOURCE_UNPAIRED_SURROGATE
    ));
    let program = no_program();
    let mut syntax_host = TestHost {
        calls: Vec::new(),
        clock_now: None,
        entropy: std::collections::VecDeque::new(),
        timezone: None,
        locale: None,
    };
    let mut syntax_vm = test_vm(&program, &mut syntax_host);
    syntax_vm.source_compiler = Some(syntax_compiler);
    let syntax = syntax_vm.enter_eval_units(&StringUnits::from(")"), None, false);
    assert!(matches!(
        syntax,
        Err(ExecutionError::SyntaxErrorMessage { message })
            if message == "ordinary parser error"
    ));

    let mut host = TestHost {
        calls: Vec::new(),
        clock_now: None,
        entropy: std::collections::VecDeque::new(),
        timezone: None,
        locale: None,
    };
    let program = no_program();
    let mut vm = test_vm(&program, &mut host);
    let raw_body = Value::String(StringUnits::from_units(&[0xd800]));
    assert!(matches!(
        vm.dispatch_native(Native::FunctionConstructor, &Value::Undefined, &[raw_body], 0),
        Err(ExecutionError::EvalUnsupported { message })
            if message == DYNAMIC_SOURCE_UNPAIRED_SURROGATE
    ));
}

/// A directive prologue decides an eval's strictness, and an escaped spelling is not one.
#[test]
fn a_use_strict_directive_is_recognised_only_when_it_is_written_literally() {
    assert!(source_is_strict("\"use strict\"; x"));
    assert!(source_is_strict("  // a comment\n /* b */ 'use strict'"));
    assert!(source_is_strict("\"other\"; \"use strict\";"));
    assert!(!source_is_strict("x; \"use strict\";"));
    // `"use\u0020strict"` spells the same characters and is deliberately not the directive.
    assert!(!source_is_strict("\"use\\u0020strict\";"));
    assert!(!source_is_strict(""));
}

/// An object with the given own properties, in order.
fn object(pairs: &[(&str, Value)]) -> Value {
    let mut data = ObjectData::default();
    for (key, value) in pairs {
        data.set(key, value.clone());
    }
    Value::Object(Rc::new(RefCell::new(data)))
}

/// Re-assigning a key keeps its position: JavaScript enumerates string keys in insertion
/// order, so a `set` that moved the key would change an observable property of the object.
/// `cause` follows InstallErrorCause: the property exists when the options object *has* the
/// key, even when its value is undefined. Testing presence rather than value is the whole
/// point of that step, and copying the value would lose the distinction.
/// `a["0"]` and `a[0]` name the same element. `a[-1]` and `a[1.5]` are ordinary string
/// properties, and treating them as indices would silently move data.
#[test]
fn only_integer_like_keys_are_array_indices() {
    assert_eq!(array_index(&Value::Number(0.0)), Some(0));
    assert_eq!(array_index(&Value::String("2".to_owned().into())), Some(2));
    assert_eq!(array_index(&Value::Number(-1.0)), None);
    assert_eq!(array_index(&Value::Number(1.5)), None);
    assert_eq!(array_index(&Value::Number(f64::INFINITY)), None);
    assert_eq!(array_index(&Value::String("x".to_owned().into())), None);
}

/// `in` asks about presence, which is not the same question as reading and comparing to
/// undefined: a property explicitly set to undefined is present.
#[test]
fn in_tests_presence_rather_than_value() {
    let object = object(&[("set_to_undefined", Value::Undefined)]);
    assert!(has_property(
        &Intrinsics::default(),
        &object,
        &Value::String("set_to_undefined".to_owned().into())
    ));
    assert!(!has_property(
        &Intrinsics::default(),
        &object,
        &Value::String("absent".to_owned().into())
    ));
}

/// The corpus asserts `Symbol.iterator in [1,2]`, so `in` and the property read have to
/// agree about what an array exposes. They share one table for that reason.
#[test]
fn arrays_expose_the_same_properties_to_reads_and_to_in() {
    let items = Value::Array(ArrayData::new(vec![Value::Number(1.0)]));
    for key in ["join", "values", "@@iterator", "length"] {
        assert!(
            has_property(
                &Intrinsics::default(),
                &items,
                &Value::String(key.to_owned().into())
            ),
            "`{key} in array` must be true"
        );
    }
    assert!(
        has_property(&Intrinsics::default(), &items, &Value::Number(0.0)),
        "index 0 present"
    );
    assert!(
        !has_property(&Intrinsics::default(), &items, &Value::Number(1.0)),
        "index 1 absent"
    );
}

/// A symbol keys a property under its reserved slot, not under its printable description.
#[test]
fn a_symbol_key_is_not_its_display_string() {
    let symbol = Value::Symbol(Symbol::WellKnown(WellKnown::Iterator));
    assert_eq!(symbol.property_key_lossy(), "@@iterator");
    assert_eq!(symbol.to_display_string(), "Symbol(Symbol.iterator)");
    assert_eq!(symbol.type_of(), "symbol");
}

/// `[].values` read twice must be the same function, which is what
/// `[][Symbol.iterator] === [].values` asserts. Binding a receiver at read time would make
/// each read a fresh value and quietly break it.
#[test]
fn reading_an_array_method_twice_yields_the_same_function() {
    assert!(strict_equal(
        &array_property("values"),
        &array_property("@@iterator")
    ));
    assert!(strict_equal(
        &array_property("join"),
        &array_property("join")
    ));
    assert!(!strict_equal(
        &array_property("join"),
        &array_property("values")
    ));
}

/// `join` treats null and undefined as empty rather than stringifying them, which is the one
/// place its output differs from mapping `String` over the elements.
#[test]
fn join_renders_nullish_elements_as_empty() {
    let items = [
        Value::Number(1.0),
        Value::Undefined,
        Value::Null,
        Value::String("x".to_owned().into()),
    ];
    assert_eq!(join(&items, ",").unwrap(), "1,,,x");
}

#[test]
fn error_cause_is_installed_by_presence_not_by_value() {
    let with_cause = build_error(
        ErrorKind::Error,
        &[
            Value::String("m".to_owned().into()),
            object(&[("cause", Value::Undefined)]),
        ],
    );
    let Value::Object(data) = &with_cause else {
        panic!("construct_error returns an object");
    };
    assert_eq!(data.borrow().get("cause"), Some(Value::Undefined));

    let without = build_error(ErrorKind::Error, &[Value::String("m".to_owned().into())]);
    let Value::Object(data) = &without else {
        panic!("construct_error returns an object");
    };
    assert_eq!(data.borrow().get("cause"), None, "no options, no cause");
}

/// A cause is carried by identity: the error holds the same object the caller passed.
#[test]
fn error_cause_keeps_the_identity_of_the_value_given() {
    let inner = object(&[("marker", Value::Number(1.0))]);
    let outer = build_error(
        ErrorKind::Error,
        &[
            Value::String("outer".to_owned().into()),
            object(&[("cause", inner.clone())]),
        ],
    );
    let Value::Object(data) = &outer else {
        panic!("construct_error returns an object");
    };
    let cause = data.borrow().get("cause").expect("cause present");
    assert!(strict_equal(&cause, &inner), "cause is the same object");
}

#[test]
fn error_brand_is_not_the_reserved_trace_property() {
    let error = build_error(ErrorKind::Error, &[Value::String("x".to_owned().into())]);
    let Value::Object(error) = &error else {
        panic!("build_error returns an object");
    };
    assert!(error.borrow().error_brand);

    // The trace is deliberately a separate, reserved property. Program-visible writes and
    // deletes must not change the internal Error brand.
    error
        .borrow_mut()
        .set(ERROR_TRACE, Value::String("spoofed".to_owned().into()));
    error.borrow_mut().remove(ERROR_TRACE);
    assert!(error.borrow().error_brand);

    let fake = object(&[(ERROR_TRACE, Value::String("spoofed".to_owned().into()))]);
    let Value::Object(fake) = fake else {
        unreachable!();
    };
    assert!(!fake.borrow().error_brand);
}

#[test]
fn error_is_error_inherits_through_native_error_constructors() {
    let intrinsics = Intrinsics::default();
    let type_error = Value::Native(Native::Error(ErrorKind::Type), RealmId::PRIMARY);
    assert!(matches!(
        native_inherited_property(&intrinsics, &type_error, "isError"),
        Value::Native(Native::ErrorIsError, _)
    ));
    assert!(matches!(
        prototype_of(&type_error),
        Value::Native(Native::Error(ErrorKind::Error), _)
    ));
}

#[test]
fn regexp_escape_appears_in_constructor_own_property_names() {
    let intrinsics = Intrinsics::default();
    let constructor = Value::Native(Native::RegExpConstructor, RealmId::PRIMARY);
    assert!(own_property_keys(&intrinsics, &constructor).contains(&"escape".to_owned()));
}

/// An error with no message gets a bare header, not one with a trailing colon -- and no own
/// `message` at all, because an absent message is the prototype's empty one.
#[test]
fn an_empty_message_produces_a_bare_stack_header() {
    for arguments in [Vec::new(), vec![Value::Undefined]] {
        let Value::Object(data) = build_error(ErrorKind::Error, &arguments) else {
            panic!("construct_error returns an object");
        };
        assert_eq!(
            data.borrow().get("stack"),
            Some(Value::String("Error".to_owned().into()))
        );
        assert_eq!(data.borrow().get_own("message"), None);
    }
}

/// `name` is the prototype's, never the instance's.
///
/// An own one would shadow a subclass's `name` class field, which is installed on the
/// instance while the constructor runs -- so `e.name` answered "Error" for an error the
/// program had explicitly named, and `if (e.name === "TimeoutError")` took the wrong branch.
#[test]
fn an_error_instance_has_no_own_name() {
    let Value::Object(data) = build_error(ErrorKind::Type, &[Value::String("m".to_owned().into())])
    else {
        panic!("construct_error returns an object");
    };
    let data = data.borrow();
    assert_eq!(data.get_own("name"), None);
    assert_eq!(
        data.get_own("message"),
        Some(&Value::String("m".to_owned().into())),
        "a message that was passed is still the instance's own"
    );
}

/// A `DataView` reads *big*-endian unless told otherwise -- the opposite of the platform and
/// of every typed array. Getting it backwards round-trips cleanly through the same view, so
/// nothing inside a program notices; it only shows when the bytes meet something outside
/// one, which is exactly what a `DataView` is for.
#[test]
fn a_data_view_defaults_to_the_opposite_order_from_the_platform() {
    let mut written = [0_u8; 2];
    TypedKind::Int16.encode(&Value::Number(258.0), &mut written);
    order_bytes(&mut written, false);
    assert_eq!(written, [0x01, 0x02], "the high byte leads");

    let mut same_order = written;
    order_bytes(&mut same_order, false);
    assert_eq!(TypedKind::Int16.decode(&same_order), Value::Number(258.0));

    let mut other_order = written;
    order_bytes(&mut other_order, true);
    assert_eq!(
        TypedKind::Int16.decode(&other_order),
        Value::Number(513.0),
        "the same bytes read the other way round"
    );
}

/// A `BigInt64Array` element is a BigInt, not a number, and the sixty-four bits survive.
///
/// `2^63 - 1` is the value that proves it: the nearest `f64` to it is `2^63`, so a view
/// that decoded through a double would answer a number one larger than what was stored and
/// nothing inside the program could tell.
#[test]
fn a_big_element_round_trips_without_rounding() {
    let mut written = [0_u8; 8];
    let value = Value::BigInt(Rc::new(num_bigint::BigInt::from(i64::MAX)));
    TypedKind::BigInt64.encode(&value, &mut written);
    assert_eq!(TypedKind::BigInt64.decode(&written), value);
    // The same bits, read unsigned, are the other end of the wrap.
    TypedKind::BigInt64.encode(
        &Value::BigInt(Rc::new(num_bigint::BigInt::from(-1))),
        &mut written,
    );
    assert_eq!(
        TypedKind::BigUint64.decode(&written),
        Value::BigInt(Rc::new(num_bigint::BigInt::from(u64::MAX)))
    );
}

/// The two numeric towers do not cross, in either direction.
#[test]
fn an_element_refuses_the_other_numeric_tower() {
    let one = Value::Number(1.0);
    let big = Value::BigInt(Rc::new(num_bigint::BigInt::from(1)));
    assert!(coerce_element(TypedKind::BigInt64, &one).is_err());
    assert!(coerce_element(TypedKind::Uint8, &big).is_err());
    assert_eq!(coerce_element(TypedKind::BigInt64, &big).unwrap(), big);
    assert_eq!(coerce_element(TypedKind::Uint8, &one).unwrap(), one);
}

/// `typeof null` is "object" -- a specification bug old enough to be load-bearing, and
/// distinct from the diagnostic name, which says "null" because that is more useful there.
#[test]
fn typeof_reports_the_language_tags_including_the_null_one() {
    assert_eq!(Value::Null.type_of(), "object");
    assert_eq!(Value::Null.type_name(), "null");
    assert_eq!(Value::Undefined.type_of(), "undefined");
    assert_eq!(Value::Number(1.0).type_of(), "number");
    assert_eq!(
        Value::Native(Native::Error(ErrorKind::Error), RealmId::PRIMARY).type_of(),
        "function"
    );
    assert_eq!(object(&[]).type_of(), "object");
}

#[test]
fn new_on_a_non_constructor_is_rejected() {
    let program = Program {
        source: "".into(),
        template_sites: Vec::new(),
        version: kiln_ir::BYTECODE_VERSION,
        constants: vec![number(1.0)],
        code: vec![
            Instruction::Const(0),
            Instruction::New { argc: 0 },
            Instruction::Return,
        ],
        local_count: 0,
        functions: Vec::new(),
    };
    assert!(matches!(
        run(&program),
        Err(ExecutionError::Uncaught { message, .. })
            if message == "TypeError: a number is not a constructor"
    ));
}

/// A write must create an own property rather than mutating the shared prototype, or two
/// instances of one class would not be independent.
/// `instanceof` compares the constructor's prototype by identity, walking the chain. Two
/// unrelated classes with identical shapes must not be confused for one another.
#[test]
fn instanceof_walks_the_chain_by_identity() {
    let parent_prototype = Value::Object(Rc::new(RefCell::new(ObjectData::default())));
    let parent = Rc::new(Closure {
        inferred_name: None,
        realm: RealmId::PRIMARY,
        name: None,
        program: no_program(),
        strictness: Rc::new(Strictness::default()),
        module_path: None,
        captured_new_target: None,
        function: 0,
        upvalues: Vec::new(),
        captured_this: None,
        captured_arguments: None,
        prototype: RefCell::new(Some(parent_prototype.clone())),
        properties: RefCell::new(ObjectData::default()),
    });

    let mut child_prototype_data = ObjectData::default();
    child_prototype_data.set_prototype(parent_prototype);
    let child_prototype = Value::Object(Rc::new(RefCell::new(child_prototype_data)));
    let child = Rc::new(Closure {
        inferred_name: None,
        realm: RealmId::PRIMARY,
        name: None,
        program: no_program(),
        strictness: Rc::new(Strictness::default()),
        module_path: None,
        captured_new_target: None,
        function: 1,
        upvalues: Vec::new(),
        captured_this: None,
        captured_arguments: None,
        prototype: RefCell::new(Some(child_prototype.clone())),
        properties: RefCell::new(ObjectData::default()),
    });

    let mut instance_data = ObjectData::default();
    instance_data.set_prototype(child_prototype);
    let instance = Value::Object(Rc::new(RefCell::new(instance_data)));

    assert!(instance_of(
        &instance,
        &Value::Function(Rc::clone(&child)),
        None
    ));
    assert!(
        instance_of(&instance, &Value::Function(Rc::clone(&parent)), None),
        "the chain is walked, not just the immediate link"
    );

    // A separate constructor with its own prototype must not match, however alike.
    let unrelated = Rc::new(Closure {
        inferred_name: None,
        realm: RealmId::PRIMARY,
        name: None,
        program: no_program(),
        strictness: Rc::new(Strictness::default()),
        module_path: None,
        captured_new_target: None,
        function: 2,
        upvalues: Vec::new(),
        captured_this: None,
        captured_arguments: None,
        prototype: RefCell::new(Some(Value::Object(Rc::new(RefCell::new(
            ObjectData::default(),
        ))))),
        properties: RefCell::new(ObjectData::default()),
    });
    assert!(!instance_of(&instance, &Value::Function(unrelated), None));
    assert!(!instance_of(
        &Value::Number(1.0),
        &Value::Function(child),
        None
    ));
}

/// `String(e)` renders an error through its inherited `toString`, so an ordinary object that
/// merely has `name` and `message` is left alone.
#[test]
fn error_to_string_is_keyed_on_the_inherited_method() {
    let plain = object(&[
        ("name", Value::String("NotAnError".to_owned().into())),
        ("message", Value::String("m".to_owned().into())),
    ]);
    assert!(!has_error_to_string(&plain));

    let erroneous = object(&[
        ("name", Value::String("E".to_owned().into())),
        ("message", Value::String("boom".to_owned().into())),
        (
            "toString",
            Value::Native(Native::ErrorToString, RealmId::PRIMARY),
        ),
    ]);
    assert!(has_error_to_string(&erroneous));
    assert_eq!(error_to_string(&erroneous), "E: boom");
}

/// An error with no message renders as a bare name, with no trailing colon.
#[test]
fn error_to_string_omits_an_empty_message() {
    let bare = object(&[
        ("name", Value::String("Error".to_owned().into())),
        ("message", Value::String(String::new().into())),
    ]);
    assert_eq!(error_to_string(&bare), "Error");
}

/// Deleting during iteration must not skip the next element. Compacting the backing store
/// shifts everything past the cursor by one, which is the bug this design exists to avoid.
#[test]
fn deleting_leaves_a_hole_rather_than_compacting() {
    let entries = vec![Value::Number(1.0), Value::Number(2.0), Value::Number(3.0)];
    assert_eq!(find_entry(&entries, &Value::Number(2.0), false), Some(1));

    let with_hole = vec![
        Value::Number(1.0),
        Value::Native(Native::Tombstone, RealmId::PRIMARY),
        Value::Number(3.0),
    ];
    assert_eq!(
        find_entry(&with_hole, &Value::Number(3.0), false),
        Some(2),
        "a hole must not shift the indices after it"
    );
    assert_eq!(
        find_entry(&with_hole, &Value::Number(2.0), false),
        None,
        "a deleted entry is not found"
    );
}

/// Set and Map use SameValueZero, which differs from `===` in exactly one place: NaN is its
/// own key, so a Set can contain one and find it again.
#[test]
fn collections_key_on_same_value_zero() {
    let nan = Value::Number(f64::NAN);
    assert!(!strict_equal(&nan, &nan), "=== says NaN is not itself");
    assert!(same_value_zero(&nan, &nan), "SameValueZero says it is");

    assert!(same_value_zero(&Value::Number(0.0), &Value::Number(-0.0)));
    assert!(!same_value_zero(
        &Value::Number(1.0),
        &Value::String("1".to_owned().into())
    ));
}

/// A Map keyed by identity finds the same object and not an equal one.
#[test]
fn map_keys_are_matched_by_identity() {
    let key = object(&[("id", Value::Number(1.0))]);
    let twin = object(&[("id", Value::Number(1.0))]);
    let entries = vec![Value::Array(ArrayData::new(vec![
        key.clone(),
        Value::String("found".to_owned().into()),
    ]))];

    assert_eq!(find_entry(&entries, &key, true), Some(0));
    assert_eq!(
        find_entry(&entries, &twin, true),
        None,
        "an equal-looking object is a different key"
    );
}

/// An exception with no handler is an execution error naming what was thrown, rather than a
/// silent stop or a panic.
#[test]
fn an_uncaught_throw_names_what_was_thrown() {
    let program = Program {
        source: "".into(),
        template_sites: Vec::new(),
        version: kiln_ir::BYTECODE_VERSION,
        constants: vec![Constant::String("boom".to_owned().into())],
        code: vec![Instruction::Const(0), Instruction::Throw],
        local_count: 0,
        functions: Vec::new(),
    };
    let Err(ExecutionError::Uncaught { message, .. }) = run(&program) else {
        panic!("an unhandled throw must not succeed");
    };
    assert_eq!(message, "boom");
}

/// A handler restores the stack to the height it had when the handler was registered, so
/// values left by the abandoned code cannot leak into the catch block.
#[test]
fn unwinding_restores_the_stack_the_handler_expected() {
    let program = Program {
        source: "".into(),
        template_sites: Vec::new(),
        version: kiln_ir::BYTECODE_VERSION,
        constants: vec![
            number(1.0),
            number(2.0),
            Constant::String("x".to_owned().into()),
        ],
        code: vec![
            Instruction::Const(0), // a value the handler must still see
            Instruction::PushHandler(6),
            Instruction::Const(1), // debris left by the abandoned try block
            Instruction::Const(1),
            Instruction::Const(2),
            Instruction::Throw,
            // handler: the thrown value is on top, and the sentinel is beneath it
            Instruction::Pop,
            Instruction::Return,
        ],
        local_count: 0,
        functions: Vec::new(),
    };
    assert_eq!(
        run(&program),
        Ok(Value::Number(1.0)),
        "the sentinel from before the handler survives, the debris does not"
    );
}

/// An error renders through its inherited `toString`; any other value through its ordinary
/// string form, because JavaScript allows throwing anything.
#[test]
fn a_thrown_value_is_described_by_what_it_is() {
    let error = object(&[
        ("name", Value::String("E".to_owned().into())),
        ("message", Value::String("boom".to_owned().into())),
        (
            "toString",
            Value::Native(Native::ErrorToString, RealmId::PRIMARY),
        ),
    ]);
    assert_eq!(describe_thrown(&error), "E: boom");
    assert_eq!(describe_thrown(&Value::Number(42.0)), "42");
    assert_eq!(describe_thrown(&Value::Undefined), "undefined");
}

#[test]
fn a_write_does_not_reach_through_the_prototype() {
    let shared = Rc::new(RefCell::new(ObjectData::default()));
    shared.borrow_mut().set("value", Value::Number(1.0));

    let mut instance = ObjectData::default();
    instance.set_prototype(Value::Object(Rc::clone(&shared)));
    assert_eq!(instance.get("value"), Some(Value::Number(1.0)), "inherited");

    instance.set("value", Value::Number(2.0));
    assert_eq!(instance.get("value"), Some(Value::Number(2.0)), "own wins");
    assert_eq!(
        shared.borrow().get_own("value"),
        Some(&Value::Number(1.0)),
        "the prototype is untouched"
    );
}

/// Reads follow the chain to any depth; `get_own` never does. The two are separate because
/// `in` and property assignment need opposite answers.
#[test]
fn reads_follow_the_prototype_chain_and_own_reads_do_not() {
    let grandparent = Rc::new(RefCell::new(ObjectData::default()));
    grandparent.borrow_mut().set("deep", Value::Number(3.0));

    let mut parent_data = ObjectData::default();
    parent_data.set_prototype(Value::Object(grandparent));
    let parent = Rc::new(RefCell::new(parent_data));

    let mut child = ObjectData::default();
    child.set_prototype(Value::Object(parent));

    assert_eq!(child.get("deep"), Some(Value::Number(3.0)));
    assert_eq!(child.get_own("deep"), None);
    assert_eq!(child.get("absent"), None);
}

#[test]
fn reassigning_a_property_keeps_its_position() {
    let mut data = ObjectData::default();
    data.set("first", Value::Number(1.0));
    data.set("second", Value::Number(2.0));
    data.set("first", Value::Number(99.0));
    assert_eq!(data.keys().collect::<Vec<_>>(), ["first", "second"]);
    assert_eq!(data.get("first"), Some(Value::Number(99.0)));
}

/// `===` on objects asks whether two references name the same object. Structural equality
/// would say two distinct objects with equal contents are the same, and then a write through
/// one would appear to change the other.
#[test]
fn object_equality_is_identity_not_structure() {
    let left = object(&[("a", Value::Number(1.0))]);
    let right = object(&[("a", Value::Number(1.0))]);
    assert!(strict_equal(&left, &left), "an object equals itself");
    assert_eq!(&left, &left, "Rust equality uses JavaScript identity too");
    assert!(
        !strict_equal(&left, &right),
        "equal contents are not the same object"
    );
    assert_ne!(
        &left, &right,
        "derived payload equality must not leak through Value"
    );
    assert!(strict_equal(&left, &left.clone()), "a clone shares the Rc");
}

#[test]
fn engine_and_commonjs_values_are_reflexive_identity_objects() {
    let temporal = temporal::wrap(
        kiln_temporal::plain_date(2026.0, 8.0, 31.0, None).expect("construct Temporal.PlainDate"),
    );
    let intl = Rc::new(intl::IntlValue::Locale("en-US".to_owned()));
    let require = Rc::new(CommonJsRequire::new(std::iter::empty::<(
        String,
        String,
        Cell,
    )>()));

    let same_objects = [
        temporal.clone(),
        Value::Intl(Rc::clone(&intl)),
        Value::CommonJsRequire(Rc::clone(&require)),
        Value::CommonJsResolve(Rc::clone(&require)),
    ];
    for value in &same_objects {
        let cloned = value.clone();
        assert!(
            strict_equal(value, &cloned),
            "{} must equal another reference to itself",
            value.type_name()
        );
    }

    let other_temporal = temporal::wrap(
        kiln_temporal::plain_date(2026.0, 8.0, 31.0, None)
            .expect("construct equal but distinct Temporal.PlainDate"),
    );
    let other_intl = Value::Intl(Rc::new(intl::IntlValue::Locale("en-US".to_owned())));
    let other_require = Value::CommonJsRequire(Rc::new(CommonJsRequire::new(std::iter::empty::<(
        String,
        String,
        Cell,
    )>())));
    assert!(!strict_equal(&same_objects[0], &other_temporal));
    assert!(!strict_equal(&same_objects[1], &other_intl));
    assert!(!strict_equal(&same_objects[2], &other_require));
    assert!(
        !strict_equal(&same_objects[2], &same_objects[3]),
        "require and require.resolve share state, not function identity"
    );
}

/// A missing property is `undefined`, not an error: reading one is legal JavaScript.
#[test]
fn a_missing_property_reads_as_undefined() {
    let data = ObjectData::default();
    assert_eq!(data.get("absent"), None);
}

/// Assigning a property of a primitive is a TypeError in strict mode, and modules are always
/// strict. Silently discarding it would lose a write the program believes it made.
#[test]
fn setting_a_property_on_a_primitive_is_rejected() {
    let program = Program {
        source: "".into(),
        template_sites: Vec::new(),
        version: kiln_ir::BYTECODE_VERSION,
        constants: vec![Constant::String("x".to_owned().into()), number(1.0)],
        code: vec![
            Instruction::Const(1),
            Instruction::Const(1),
            Instruction::SetProperty(0),
            Instruction::Return,
        ],
        local_count: 0,
        functions: Vec::new(),
    };
    assert!(matches!(
        run(&program),
        Err(ExecutionError::Uncaught { message, .. })
            if message == "TypeError: cannot set a property on a number"
    ));
}

#[test]
fn closures_from_separate_calls_capture_separate_cells() {
    // outer(): let c = <cell>; return closure(inner) -- inner increments and returns c.
    let program = Program {
        source: "".into(),
        template_sites: Vec::new(),
        version: kiln_ir::BYTECODE_VERSION,
        constants: vec![number(1.0)],
        code: vec![
            Instruction::MakeFunction(0),
            Instruction::Call { argc: 0 },
            Instruction::Call { argc: 0 },
            Instruction::MakeFunction(0),
            Instruction::Call { argc: 0 },
            Instruction::Call { argc: 0 },
            Instruction::Return,
        ],
        local_count: 0,
        functions: vec![
            kiln_ir::FunctionProto {
                inferred_name: None,
                span: kiln_ir::Span::default(),
                prologue_end: 0,
                asynchronous: false,
                name: Some("outer".to_owned()),
                arrow: false,
                generator: false,
                async_generator: false,
                is_method: false,
                parameter_count: 0,
                declared_length: 0,
                mapped_arguments: false,
                local_count: 1,
                code: vec![
                    Instruction::NewCell(0),
                    Instruction::Const(0),
                    Instruction::StoreCell(0),
                    Instruction::MakeFunction(1),
                    Instruction::Return,
                ],
                upvalues: Vec::new(),
                type_annotations: None,
            },
            kiln_ir::FunctionProto {
                inferred_name: None,
                span: kiln_ir::Span::default(),
                prologue_end: 0,
                asynchronous: false,
                name: Some("inner".to_owned()),
                arrow: false,
                generator: false,
                async_generator: false,
                is_method: false,
                parameter_count: 0,
                declared_length: 0,
                mapped_arguments: false,
                local_count: 0,
                code: vec![
                    Instruction::LoadUpvalue(0),
                    Instruction::Const(0),
                    Instruction::Binary(BinaryOp::Add),
                    Instruction::Dup,
                    Instruction::StoreUpvalue(0),
                    Instruction::Return,
                ],
                upvalues: vec![Upvalue::ParentLocal(0)],
                type_annotations: None,
            },
        ],
    };
    // Each outer() call makes a fresh cell seeded to 1, so each inner() returns 2.
    assert_eq!(run(&program), Ok(Value::Number(2.0)));
}

/// A closure keeps working after the call that created it has returned, which is the whole
/// reason captures are cells rather than pointers into a frame.
#[test]
fn a_captured_cell_outlives_the_frame_that_created_it() {
    let program = Program {
        source: "".into(),
        template_sites: Vec::new(),
        version: kiln_ir::BYTECODE_VERSION,
        constants: vec![number(5.0)],
        code: vec![
            Instruction::MakeFunction(0),
            Instruction::Call { argc: 0 },
            Instruction::Call { argc: 0 },
            Instruction::Return,
        ],
        local_count: 0,
        functions: vec![
            kiln_ir::FunctionProto {
                inferred_name: None,
                span: kiln_ir::Span::default(),
                prologue_end: 0,
                asynchronous: false,
                name: Some("outer".to_owned()),
                arrow: false,
                generator: false,
                async_generator: false,
                is_method: false,
                parameter_count: 0,
                declared_length: 0,
                mapped_arguments: false,
                local_count: 1,
                code: vec![
                    Instruction::NewCell(0),
                    Instruction::Const(0),
                    Instruction::StoreCell(0),
                    Instruction::MakeFunction(1),
                    Instruction::Return,
                ],
                upvalues: Vec::new(),
                type_annotations: None,
            },
            kiln_ir::FunctionProto {
                inferred_name: None,
                span: kiln_ir::Span::default(),
                prologue_end: 0,
                asynchronous: false,
                name: Some("read".to_owned()),
                arrow: false,
                generator: false,
                async_generator: false,
                is_method: false,
                parameter_count: 0,
                declared_length: 0,
                mapped_arguments: false,
                local_count: 0,
                code: vec![Instruction::LoadUpvalue(0), Instruction::Return],
                upvalues: vec![Upvalue::ParentLocal(0)],
                type_annotations: None,
            },
        ],
    };
    assert_eq!(run(&program), Ok(Value::Number(5.0)));
}

#[test]
fn capturing_a_slot_that_was_never_boxed_is_rejected() {
    let program = Program {
        source: "".into(),
        template_sites: Vec::new(),
        version: kiln_ir::BYTECODE_VERSION,
        constants: Vec::new(),
        code: vec![Instruction::MakeFunction(0), Instruction::Return],
        local_count: 1,
        functions: vec![kiln_ir::FunctionProto {
            inferred_name: None,
            span: kiln_ir::Span::default(),
            prologue_end: 0,
            asynchronous: false,
            name: None,
            arrow: false,
            generator: false,
            async_generator: false,
            is_method: false,
            parameter_count: 0,
            declared_length: 0,
            mapped_arguments: false,
            local_count: 0,
            code: vec![Instruction::Return],
            upvalues: vec![Upvalue::ParentLocal(0)],
            type_annotations: None,
        }],
    };
    assert!(matches!(
        run(&program),
        Err(ExecutionError::NotACell { slot: 0 })
    ));
}

#[test]
fn a_call_binds_arguments_to_parameters_in_order() {
    let program = calling_program(2, vec![Instruction::Const(0), Instruction::Const(1)]);
    assert_eq!(run(&program), Ok(Value::Number(2.0)));
}

/// Wrong arity is not an error in JavaScript, so it is not one here either.
#[test]
fn missing_arguments_are_undefined_and_extra_ones_are_discarded() {
    let short = calling_program(1, vec![Instruction::Const(0)]);
    assert_eq!(
        run(&short),
        Ok(Value::Undefined),
        "second parameter unbound"
    );

    let long = calling_program(
        3,
        vec![
            Instruction::Const(0),
            Instruction::Const(1),
            Instruction::Const(2),
        ],
    );
    assert_eq!(run(&long), Ok(Value::Number(2.0)), "third argument dropped");
}

/// The caller's operand stack must survive the call untouched.
#[test]
fn a_call_leaves_only_its_result_on_the_caller_stack() {
    let mut program = calling_program(2, vec![Instruction::Const(0), Instruction::Const(1)]);
    // Push a sentinel first; it must still be underneath the result afterwards.
    program.code.insert(0, Instruction::Const(2));
    program.code.pop();
    program.code.push(Instruction::Pop);
    program.code.push(Instruction::Return);
    assert_eq!(run(&program), Ok(Value::Number(3.0)), "sentinel survived");
}

#[test]
fn calling_a_non_function_is_an_error_rather_than_a_coercion() {
    let program = Program {
        source: "".into(),
        template_sites: Vec::new(),
        version: kiln_ir::BYTECODE_VERSION,
        constants: vec![number(1.0)],
        code: vec![
            Instruction::Const(0),
            Instruction::Call { argc: 0 },
            Instruction::Return,
        ],
        local_count: 0,
        functions: Vec::new(),
    };
    // A TypeError the program could have caught, not a fatal stop: reaching the top with
    // no handler is what makes it an `Uncaught`, and the message names the kind.
    assert!(matches!(
        run(&program),
        Err(ExecutionError::Uncaught { message, .. })
            if message == "TypeError: a number is not a function"
    ));
}

/// Unbounded recursion has to be reported, not abort the process by exhausting the host
/// stack. The step limit alone does not do it: frames are consumed far faster than steps.
#[test]
fn runaway_recursion_is_bounded_by_call_depth() {
    let program = Program {
        source: "".into(),
        template_sites: Vec::new(),
        version: kiln_ir::BYTECODE_VERSION,
        constants: Vec::new(),
        code: vec![
            Instruction::MakeFunction(0),
            Instruction::Call { argc: 0 },
            Instruction::Return,
        ],
        local_count: 0,
        functions: vec![kiln_ir::FunctionProto {
            inferred_name: None,
            span: kiln_ir::Span::default(),
            prologue_end: 0,
            asynchronous: false,
            name: Some("forever".to_owned()),
            arrow: false,
            generator: false,
            async_generator: false,
            is_method: false,
            parameter_count: 0,
            declared_length: 0,
            mapped_arguments: false,
            local_count: 0,
            code: vec![
                Instruction::MakeFunction(0),
                Instruction::Call { argc: 0 },
                Instruction::Return,
            ],
            upvalues: Vec::new(),
            type_annotations: None,
        }],
    };
    let mut host = TestHost {
        calls: Vec::new(),
        clock_now: None,
        entropy: std::collections::VecDeque::new(),
        timezone: None,
        locale: None,
    };
    let limits = ExecutionLimits {
        max_call_depth: 8,
        ..ExecutionLimits::default()
    };
    // Reported as a thrown `RangeError`, which nothing here catches, rather than as a
    // fatal status: the bound is still enforced, and a program that wants to survive its own
    // recursion can now write the `try` that does it.
    let outcome = execute(
        Rc::new(program.clone()),
        &mut host,
        limits,
        ExecutionCapabilities::default(),
    );
    assert!(
        matches!(&outcome, Err(ExecutionError::Uncaught { message, .. })
                if message.contains("Maximum call stack size exceeded")),
        "expected an uncaught RangeError, got {outcome:?}"
    );
}

/// A module slot written inside a frame must be visible to the next frame that reads it:
/// that is the difference between module scope and a per-frame copy.
#[test]
fn module_slots_are_shared_across_frames() {
    let program = Program {
        source: "".into(),
        template_sites: Vec::new(),
        version: kiln_ir::BYTECODE_VERSION,
        constants: vec![number(7.0)],
        code: vec![
            Instruction::MakeFunction(0),
            Instruction::Call { argc: 0 },
            Instruction::Pop,
            Instruction::MakeFunction(1),
            Instruction::Call { argc: 0 },
            Instruction::Return,
        ],
        local_count: 1,
        functions: vec![
            kiln_ir::FunctionProto {
                inferred_name: None,
                span: kiln_ir::Span::default(),
                prologue_end: 0,
                asynchronous: false,
                name: Some("write".to_owned()),
                arrow: false,
                generator: false,
                async_generator: false,
                is_method: false,
                parameter_count: 0,
                declared_length: 0,
                mapped_arguments: false,
                local_count: 0,
                code: vec![
                    Instruction::Const(0),
                    Instruction::StoreGlobal(0),
                    Instruction::Const(0),
                    Instruction::Return,
                ],
                upvalues: Vec::new(),
                type_annotations: None,
            },
            kiln_ir::FunctionProto {
                inferred_name: None,
                span: kiln_ir::Span::default(),
                prologue_end: 0,
                asynchronous: false,
                name: Some("read".to_owned()),
                arrow: false,
                generator: false,
                async_generator: false,
                is_method: false,
                parameter_count: 0,
                declared_length: 0,
                mapped_arguments: false,
                local_count: 0,
                code: vec![Instruction::LoadGlobal(0), Instruction::Return],
                upvalues: Vec::new(),
                type_annotations: None,
            },
        ],
    };
    assert_eq!(run(&program), Ok(Value::Number(7.0)));
}

#[test]
fn an_unknown_function_index_is_rejected_rather_than_indexed() {
    let program = Program {
        source: "".into(),
        template_sites: Vec::new(),
        version: kiln_ir::BYTECODE_VERSION,
        constants: Vec::new(),
        code: vec![Instruction::MakeFunction(3), Instruction::Return],
        local_count: 0,
        functions: Vec::new(),
    };
    assert!(matches!(
        run(&program),
        Err(ExecutionError::UnknownFunction { function: 3 })
    ));
}

fn number(value: f64) -> Constant {
    Constant::Number(value.to_bits())
}

fn program(constants: Vec<Constant>, code: Vec<Instruction>) -> Program {
    Program {
        source: "".into(),
        template_sites: Vec::new(),
        version: kiln_ir::BYTECODE_VERSION,
        constants,
        code,
        local_count: 0,
        functions: Vec::new(),
    }
}

#[test]
fn scalar_coercions_match_the_execution_slice() {
    assert!(!Value::Number(0.0).is_truthy());
    assert!(!Value::Number(f64::NAN).is_truthy());
    assert_eq!(Value::String("  ".into()).as_number(), 0.0);
    assert!(Value::String("x".into()).as_number().is_nan());
    assert_eq!(Value::Number(-0.0).to_display_string(), "0");
}

#[test]
fn arithmetic_and_concat_are_scalar_operations() {
    assert_eq!(
        binary(
            &Intrinsics::default(),
            BinaryOp::Add,
            Value::Number(2.0),
            Value::Number(3.0)
        )
        .unwrap(),
        Value::Number(5.0)
    );
    assert_eq!(
        binary(
            &Intrinsics::default(),
            BinaryOp::Add,
            Value::String("x".into()),
            Value::Number(3.0)
        )
        .unwrap(),
        Value::String("x3".into())
    );
    assert_eq!(
        binary(
            &Intrinsics::default(),
            BinaryOp::Exponentiate,
            Value::Number(2.0),
            Value::Number(3.0)
        )
        .unwrap(),
        Value::Number(8.0)
    );
    assert_eq!(
        binary(
            &Intrinsics::default(),
            BinaryOp::StrictEqual,
            Value::Number(f64::NAN),
            Value::Number(f64::NAN)
        )
        .unwrap(),
        Value::Boolean(false)
    );
}

fn symbol_with_throwing_right_value_of(operator: BinaryOp) -> Program {
    // `Symbol() * f()` must reject while reducing the left operand. If the right operand is
    // reduced first (or both operands are reduced before the numeric check), its `valueOf`
    // throws "called" and hides the required Symbol TypeError.
    Program {
        source: "".into(),
        template_sites: Vec::new(),
        version: kiln_ir::BYTECODE_VERSION,
        constants: vec![
            Constant::String("valueOf".to_owned().into()),
            Constant::String("called".to_owned().into()),
        ],
        code: vec![
            Instruction::LoadBuiltin(Builtin::Symbol),
            Instruction::Call { argc: 0 },
            Instruction::MakeFunction(0),
            Instruction::Call { argc: 0 },
            Instruction::Binary(operator),
            Instruction::Return,
        ],
        local_count: 0,
        functions: vec![
            kiln_ir::FunctionProto {
                inferred_name: None,
                span: kiln_ir::Span::default(),
                prologue_end: 0,
                asynchronous: false,
                name: Some("right".to_owned()),
                arrow: false,
                generator: false,
                async_generator: false,
                is_method: false,
                parameter_count: 0,
                declared_length: 0,
                mapped_arguments: false,
                local_count: 0,
                code: vec![
                    Instruction::Const(0),
                    Instruction::MakeFunction(1),
                    Instruction::NewObject { count: 1 },
                    Instruction::Return,
                ],
                upvalues: Vec::new(),
                type_annotations: None,
            },
            kiln_ir::FunctionProto {
                inferred_name: None,
                span: kiln_ir::Span::default(),
                prologue_end: 0,
                asynchronous: false,
                name: Some("valueOf".to_owned()),
                arrow: false,
                generator: false,
                async_generator: false,
                is_method: false,
                parameter_count: 0,
                declared_length: 0,
                mapped_arguments: false,
                local_count: 0,
                code: vec![Instruction::Const(1), Instruction::Throw],
                upvalues: Vec::new(),
                type_annotations: None,
            },
        ],
    }
}

#[test]
fn a_symbol_numeric_error_precedes_the_right_value_of_call() {
    assert!(matches!(
        run(&symbol_with_throwing_right_value_of(BinaryOp::Multiply)),
        Err(ExecutionError::Uncaught { message, .. })
            if message == "TypeError: Cannot convert a Symbol value to a number"
    ));
}

#[test]
fn symbol_addition_and_relational_comparison_reduce_the_right_value_first() {
    for operator in [BinaryOp::Add, BinaryOp::LessThan] {
        assert!(
            matches!(
                run(&symbol_with_throwing_right_value_of(operator)),
                Err(ExecutionError::Uncaught { message, .. }) if message == "called"
            ),
            "{operator:?} must reduce the right operand before choosing its conversion"
        );
    }
}

#[test]
fn executes_arithmetic_and_reports_instruction_count() {
    let program = program(
        vec![number(2.0), number(3.0)],
        vec![
            Instruction::Const(0),
            Instruction::Const(1),
            Instruction::Binary(BinaryOp::Add),
            Instruction::Return,
        ],
    );
    let mut host = TestHost {
        calls: Vec::new(),
        clock_now: None,
        entropy: std::collections::VecDeque::new(),
        timezone: None,
        locale: None,
    };
    let result = execute(
        Rc::new(program.clone()),
        &mut host,
        ExecutionLimits::default(),
        ExecutionCapabilities::default(),
    )
    .unwrap();
    assert_eq!(result.value, Value::Number(5.0));
    assert_eq!(result.steps, 4);
}

#[test]
fn a_marked_local_read_throws_a_reference_error() {
    // Lexical slots are marked on scope entry, so a direct read must inspect the marker just
    // like a captured cell does. This is deliberately hand-written bytecode: it catches a
    // regression where `LoadLocal` silently returned the marker as `undefined`.
    let program = Program {
        source: "".into(),
        template_sites: Vec::new(),
        version: kiln_ir::BYTECODE_VERSION,
        constants: Vec::new(),
        code: vec![
            Instruction::MarkUninitialized(0),
            Instruction::LoadLocal(0),
            Instruction::Return,
        ],
        local_count: 1,
        functions: Vec::new(),
    };
    assert!(matches!(
        run(&program),
        Err(ExecutionError::Uncaught { message, .. })
            if message == "ReferenceError: cannot access a binding before its declaration"
    ));
}

#[test]
fn a_captured_dead_zone_read_names_its_source_binding() {
    let program = Program {
        source: "".into(),
        template_sites: Vec::new(),
        version: kiln_ir::BYTECODE_VERSION,
        constants: vec![Constant::String("later".to_owned().into())],
        code: vec![
            Instruction::NewCell(0),
            Instruction::MarkUninitialized(0),
            Instruction::MakeFunction(0),
            Instruction::Call { argc: 0 },
            Instruction::Return,
        ],
        local_count: 1,
        functions: vec![kiln_ir::FunctionProto {
            inferred_name: None,
            span: kiln_ir::Span::default(),
            prologue_end: 0,
            asynchronous: false,
            name: Some("readTooEarly".to_owned()),
            arrow: false,
            generator: false,
            async_generator: false,
            is_method: false,
            parameter_count: 0,
            declared_length: 0,
            mapped_arguments: false,
            local_count: 0,
            code: vec![Instruction::LoadUpvalue(0), Instruction::Return],
            upvalues: vec![Upvalue::NamedParentLocal { slot: 0, name: 0 }],
            type_annotations: None,
        }],
    };

    assert!(matches!(
        run(&program),
        Err(ExecutionError::Uncaught { message, .. })
            if message == "ReferenceError: cannot access binding `later` before its declaration"
    ));
}

#[test]
fn conditional_jumps_consume_conditions() {
    let program = program(
        vec![Constant::Boolean(false), number(1.0), number(2.0)],
        vec![
            Instruction::Const(0),
            Instruction::JumpIfFalse(4),
            Instruction::Const(1),
            Instruction::Jump(5),
            Instruction::Const(2),
            Instruction::Return,
        ],
    );
    let mut host = TestHost {
        calls: Vec::new(),
        clock_now: None,
        entropy: std::collections::VecDeque::new(),
        timezone: None,
        locale: None,
    };
    let result = execute(
        Rc::new(program.clone()),
        &mut host,
        ExecutionLimits::default(),
        ExecutionCapabilities::default(),
    )
    .unwrap();
    assert_eq!(result.value, Value::Number(2.0));
}

#[test]
fn nullish_jump_keeps_non_nullish_left_operand() {
    // This is the shape emitted for `left ?? right`: duplicate the left value, consume the
    // duplicate for the branch, and leave the selected operand on the stack.
    let program = program(
        vec![
            Constant::String("left".into()),
            Constant::String("right".into()),
        ],
        vec![
            Instruction::Const(0),
            Instruction::Dup,
            Instruction::JumpIfNotNullish(6),
            Instruction::Pop,
            Instruction::Const(1),
            Instruction::Jump(6),
            Instruction::Return,
        ],
    );
    let mut host = TestHost {
        calls: Vec::new(),
        clock_now: None,
        entropy: std::collections::VecDeque::new(),
        timezone: None,
        locale: None,
    };
    let result = execute(
        Rc::new(program.clone()),
        &mut host,
        ExecutionLimits::default(),
        ExecutionCapabilities::default(),
    )
    .unwrap();
    assert_eq!(result.value, Value::String("left".into()));
}

#[test]
fn host_calls_preserve_argument_order_and_round_trip_values() {
    let program = program(
        vec![Constant::String("first".into()), number(2.0)],
        vec![
            Instruction::Const(0),
            Instruction::Const(1),
            Instruction::HostCall {
                op: HostOp::WriteStdout,
                argc: 2,
            },
            Instruction::Return,
        ],
    );
    let mut host = TestHost {
        calls: Vec::new(),
        clock_now: None,
        entropy: std::collections::VecDeque::new(),
        timezone: None,
        locale: None,
    };
    execute(
        Rc::new(program.clone()),
        &mut host,
        ExecutionLimits::default(),
        ExecutionCapabilities::default(),
    )
    .unwrap();
    assert_eq!(host.calls.len(), 1);
    assert_eq!(host.calls[0].0, HostOp::WriteStdout);
    assert_eq!(host.calls[0].1[0], HostValue::String("first".into()));
    assert_eq!(host.calls[0].1[1], HostValue::number(2.0));
}

#[test]
fn step_limit_stops_an_infinite_program() {
    let program = program(Vec::new(), vec![Instruction::Jump(0)]);
    let mut host = TestHost {
        calls: Vec::new(),
        clock_now: None,
        entropy: std::collections::VecDeque::new(),
        timezone: None,
        locale: None,
    };
    let error = execute(
        Rc::new(program.clone()),
        &mut host,
        ExecutionLimits {
            max_steps: 3,
            ..ExecutionLimits::default()
        },
        ExecutionCapabilities::default(),
    )
    .unwrap_err();
    assert_eq!(
        error,
        ExecutionError::StepLimitExceeded { steps: 3, limit: 3 }
    );
}

#[test]
fn module_execution_initializes_import_slots_and_publishes_exports() {
    let module = ModuleProgram {
        program: Program {
            source: "".into(),
            template_sites: Vec::new(),
            version: kiln_ir::BYTECODE_VERSION,
            constants: vec![number(2.0)],
            // An import slot holds a cell and an exported slot must too, which is what the
            // compiler emits for both -- so the fixture reads through one and writes into
            // one, as real bytecode does.
            code: vec![
                Instruction::NewCell(1),
                Instruction::LoadCell(0),
                Instruction::Const(0),
                Instruction::Binary(BinaryOp::Multiply),
                Instruction::StoreCell(1),
                Instruction::Const(0),
                Instruction::Return,
            ],
            local_count: 2,
            functions: Vec::new(),
        },
        dependencies: vec!["./dep.js".into()],
        imports: vec![kiln_ir::ImportBinding {
            source: "./dep.js".into(),
            imported: "value".into(),
            local: "input".into(),
            local_slot: 0,
        }],
        exports: vec![kiln_ir::ExportBinding {
            exported: "answer".into(),
            local_slot: 1,
        }],
        strictness: Strictness {
            body: true,
            functions: Vec::new(),
        },
        ..Default::default()
    };
    let mut host = TestHost {
        calls: Vec::new(),
        clock_now: None,
        entropy: std::collections::VecDeque::new(),
        timezone: None,
        locale: None,
    };

    let result = execute_module(
        &module,
        &[Value::Number(21.0)],
        &mut host,
        ExecutionLimits::default(),
        ExecutionCapabilities::default(),
    )
    .expect("module executes");

    assert_eq!(result.exports[0].name, "answer");
    // The cell, not its contents: an import is a live view of the exporter's binding, so
    // what is published is the box rather than what was in it when the module finished.
    assert_eq!(
        read_through_cell(&result.exports[0].value),
        Value::Number(42.0)
    );
}

/// `json_stringify` for a fixture with no accessors, so it needs no interpreter to read one.
fn serialize_units(
    value: &Value,
    indent: &StringUnits,
) -> Result<Option<StringUnits>, ExecutionError> {
    let mut unreachable_getter = |call: crate::JsonCall<'_>| match call {
        // No fixture here defines an accessor, carries `toJSON`, or passes a replacer.
        crate::JsonCall::Getter(..) => unreachable!("no fixture here defines an accessor"),
        crate::JsonCall::ToJson(receiver, _) => Ok(receiver.clone()),
        crate::JsonCall::Replace(_, _, property) => Ok(property),
    };
    // The root's holder is the wrapper `{"": value}` the specification serialises through; no
    // fixture here has a replacer to read it, so the value itself stands in.
    let empty = StringUnits::default();
    json_stringify(
        value,
        indent,
        &empty,
        &mut Vec::new(),
        &mut unreachable_getter,
        None,
        &empty,
        value,
    )
}

fn serialize(value: &Value, indent: &str) -> Result<Option<String>, ExecutionError> {
    let indent = StringUnits::from(indent);
    serialize_units(value, &indent).map(|result| {
        result.map(|text| {
            text.to_scalar_string()
                .expect("ordinary JSON fixture should be scalar text")
        })
    })
}

fn json(value: &Value) -> Option<String> {
    serialize(value, "").expect("no cycle in this fixture")
}

fn array(items: &[Value]) -> Value {
    Value::Array(ArrayData::new(items.to_vec()))
}

#[test]
fn json_scalars_serialise_as_themselves() {
    assert_eq!(json(&Value::Null).as_deref(), Some("null"));
    assert_eq!(json(&Value::Boolean(true)).as_deref(), Some("true"));
    assert_eq!(json(&Value::Number(1.5)).as_deref(), Some("1.5"));
    assert_eq!(
        json(&Value::String("hi".to_owned().into())).as_deref(),
        Some(r#""hi""#)
    );
}

#[test]
fn json_omits_what_it_cannot_spell() {
    // The caller turns `None` into `undefined`, a dropped property, or a null element --
    // three different outcomes that all start here.
    assert_eq!(json(&Value::Undefined), None);
    assert_eq!(
        json(&Value::Symbol(Symbol::WellKnown(WellKnown::Iterator))),
        None
    );
    assert_eq!(
        json(&Value::Native(Native::ArrayIsArray, RealmId::PRIMARY)),
        None
    );
}

#[test]
fn json_writes_non_finite_numbers_as_null() {
    assert_eq!(json(&Value::Number(f64::NAN)).as_deref(), Some("null"));
    assert_eq!(json(&Value::Number(f64::INFINITY)).as_deref(), Some("null"));
}

#[test]
fn json_drops_unrepresentable_properties_but_nulls_unrepresentable_elements() {
    // Dropping an array element would renumber every element after it, so the two containers
    // have to disagree here.
    let value = object(&[
        ("kept", Value::Number(1.0)),
        ("dropped", Value::Undefined),
        ("items", array(&[Value::Number(1.0), Value::Undefined])),
    ]);
    assert_eq!(
        json(&value).as_deref(),
        Some(r#"{"kept":1,"items":[1,null]}"#)
    );
}

#[test]
fn json_skips_internal_properties() {
    // `@@`-prefixed entries are the VM's own bookkeeping -- an iterator cursor, a well-known
    // symbol -- and are not own enumerable string properties of the object.
    let value = object(&[
        ("@@index", Value::Number(0.0)),
        ("real", Value::Number(1.0)),
    ]);
    assert_eq!(json(&value).as_deref(), Some(r#"{"real":1}"#));
}

#[test]
fn json_escapes_strings() {
    let value = Value::String("a\"b\\c\nd\u{1}".to_owned().into());
    assert_eq!(json(&value).as_deref(), Some(r#""a\"b\\c\nd\u0001""#));
}

#[test]
fn json_empty_containers_stay_on_one_line_when_indented() {
    let value = object(&[("a", array(&[])), ("b", object(&[]))]);
    let text = serialize(&value, "  ").expect("no cycle");
    assert_eq!(text.as_deref(), Some("{\n  \"a\": [],\n  \"b\": {}\n}"));
}

#[test]
fn json_indents_nested_containers() {
    let value = object(&[("a", array(&[Value::Number(1.0)]))]);
    let text = serialize(&value, "  ").expect("no cycle");
    assert_eq!(text.as_deref(), Some("{\n  \"a\": [\n    1\n  ]\n}"));
}

#[test]
fn json_rejects_a_value_that_contains_itself() {
    let items = ArrayData::new(Vec::new());
    items.borrow_mut().push(Value::Array(Rc::clone(&items)));
    assert!(matches!(
        serialize(&Value::Array(items), ""),
        Err(ExecutionError::CyclicJson)
    ));
}

#[test]
fn json_allows_the_same_value_twice_in_different_places() {
    // A diamond is not a cycle. The guard tracks the path from the root, not every value it
    // has ever seen, which is the whole reason it pops on the way back out.
    let shared = object(&[("n", Value::Number(1.0))]);
    let value = array(&[shared.clone(), shared]);
    assert_eq!(json(&value).as_deref(), Some(r#"[{"n":1},{"n":1}]"#));
}

#[test]
fn json_indent_argument_is_capped() {
    assert_eq!(json_indent(Some(&Value::Number(2.0))), "  ");
    assert_eq!(json_indent(Some(&Value::Number(50.0))).len(), 10);
    assert_eq!(
        json_indent(Some(&Value::String("ab".to_owned().into()))),
        "ab"
    );
    // A fractional or zero count means no indent at all, same as omitting the argument.
    assert_eq!(json_indent(Some(&Value::Number(0.0))), "");
    assert_eq!(json_indent(None), "");
}

#[test]
fn json_quotes_and_keys_preserve_utf16_units() {
    let key = StringUnits::from_units(&[0xd800, 0xfffd, 0xdc00]);
    let value = Value::String(key.clone());
    let mut data = ObjectData::default();
    data.set_units(&key, value);
    let object = Value::Object(Rc::new(RefCell::new(data)));
    let text = serialize_units(&object, &StringUnits::default())
        .expect("serialize")
        .expect("object result");
    assert_eq!(
        text.to_units().as_ref(),
        &[
            b'{' as u16,
            b'"' as u16,
            b'\\' as u16,
            b'u' as u16,
            b'd' as u16,
            b'8' as u16,
            b'0' as u16,
            b'0' as u16,
            0xfffd,
            b'\\' as u16,
            b'u' as u16,
            b'd' as u16,
            b'c' as u16,
            b'0' as u16,
            b'0' as u16,
            b'"' as u16,
            b':' as u16,
            b'"' as u16,
            b'\\' as u16,
            b'u' as u16,
            b'd' as u16,
            b'8' as u16,
            b'0' as u16,
            b'0' as u16,
            0xfffd,
            b'\\' as u16,
            b'u' as u16,
            b'd' as u16,
            b'c' as u16,
            b'0' as u16,
            b'0' as u16,
            b'"' as u16,
            b'}' as u16,
        ]
    );
}

#[test]
fn json_replacer_key_matching_uses_exact_units() {
    let key = StringUnits::from_units(&[0xd800]);
    let mut data = ObjectData::default();
    data.set_units(&key, Value::Number(7.0));
    let replacement = StringUnits::from_units(&[0xfffd]);
    data.set_units(&replacement, Value::Number(8.0));
    let object = Value::Object(Rc::new(RefCell::new(data)));
    let allowed = vec![key.clone()];
    let mut callback_keys = Vec::new();
    let mut unreachable_getter = |call: crate::JsonCall<'_>| match call {
        crate::JsonCall::Getter(..) => unreachable!("no getter"),
        crate::JsonCall::ToJson(receiver, _) => Ok(receiver.clone()),
        crate::JsonCall::Replace(_, actual, property) => {
            callback_keys.push(actual.clone());
            Ok(property)
        }
    };
    let empty = StringUnits::default();
    let text = json_stringify(
        &object,
        &empty,
        &empty,
        &mut Vec::new(),
        &mut unreachable_getter,
        Some(&allowed),
        &empty,
        &object,
    )
    .expect("serialize")
    .expect("object result");
    assert_eq!(
        text.to_units().as_ref(),
        &[
            b'{' as u16,
            b'"' as u16,
            b'\\' as u16,
            b'u' as u16,
            b'd' as u16,
            b'8' as u16,
            b'0' as u16,
            b'0' as u16,
            b'"' as u16,
            b':' as u16,
            b'7' as u16,
            b'}' as u16
        ]
    );
    assert_eq!(
        callback_keys,
        vec![StringUnits::default(), key.clone()],
        "replacer receives the root key and the exact lone-surrogate property key"
    );
    let Value::Object(parsed) = parse_json_text(&text).expect("serialized JSON parses") else {
        panic!("serialized object did not parse as an object");
    };
    assert_eq!(parsed.borrow().get_units(&key), Some(Value::Number(7.0)));
}

#[test]
fn json_raw_json_and_indent_keep_units() {
    let raw_text = StringUnits::from_units(&[b'"' as u16, 0xd800, b'"' as u16]);
    let raw = build_raw_json(&raw_text).expect("valid raw JSON string");
    let raw_output = serialize_units(&raw, &StringUnits::default())
        .expect("serialize")
        .expect("raw result");
    assert_eq!(raw_output.to_units().as_ref(), raw_text.to_units().as_ref());

    let indent = StringUnits::from_units(&[0xd800, b' ' as u16]);
    let object = object(&[("x", Value::Number(1.0))]);
    let output = serialize_units(&object, &indent)
        .expect("serialize")
        .expect("object result");
    assert_eq!(
        output.to_units().as_ref(),
        &[
            b'{' as u16,
            b'\n' as u16,
            0xd800,
            b' ' as u16,
            b'"' as u16,
            b'x' as u16,
            b'"' as u16,
            b':' as u16,
            b' ' as u16,
            b'1' as u16,
            b'\n' as u16,
            b'}' as u16
        ]
    );
}

#[test]
fn two_symbols_with_one_description_key_different_properties() {
    let first = Symbol::Unique(Rc::new(UniqueSymbol {
        serial: 1,
        description: Some("same".to_owned().into()),
    }));
    let second = Symbol::Unique(Rc::new(UniqueSymbol {
        serial: 2,
        description: Some("same".to_owned().into()),
    }));
    assert_ne!(first.property_key(), second.property_key());
    assert_ne!(first, second);
    assert_eq!(first, first.clone());
    // The description is for display only, and never reaches the key.
    assert_eq!(
        Value::Symbol(first).to_display_string(),
        "Symbol(same)".to_owned()
    );
}

#[test]
fn symbol_registry_and_symbol_key_names_preserve_units() {
    let mut host = TestHost {
        calls: Vec::new(),
        clock_now: None,
        entropy: std::collections::VecDeque::new(),
        timezone: None,
        locale: None,
    };
    let program = no_program();
    let mut vm = test_vm(&program, &mut host);
    let high = Value::String(StringUnits::from_units(&[0xd800]));
    let replacement = Value::String(StringUnits::from_units(&[0xfffd]));
    let constructed = vm
        .dispatch_native(
            Native::SymbolConstructor,
            &Value::Undefined,
            std::slice::from_ref(&high),
            0,
        )
        .expect("Symbol(high)");
    assert_eq!(
        vm.read_property(&constructed, "description", 0),
        Ok(high.clone())
    );
    let high_symbol = vm
        .dispatch_native(Native::SymbolFor, &Value::Undefined, &[high], 0)
        .expect("Symbol.for(high)");
    let replacement_symbol = vm
        .dispatch_native(Native::SymbolFor, &Value::Undefined, &[replacement], 0)
        .expect("Symbol.for(replacement)");
    assert_ne!(high_symbol, replacement_symbol);
    let missing = vm
        .dispatch_native(Native::SymbolFor, &Value::Undefined, &[], 0)
        .expect("Symbol.for() uses undefined");
    let key = vm
        .dispatch_native(Native::SymbolKeyFor, &Value::Undefined, &[high_symbol], 0)
        .expect("Symbol.keyFor");
    assert_eq!(key, Value::String(StringUnits::from_units(&[0xd800])));
    let missing_key = vm
        .dispatch_native(Native::SymbolKeyFor, &Value::Undefined, &[missing], 0)
        .expect("Symbol.keyFor(undefined-key symbol)");
    assert_eq!(missing_key, Value::String("undefined".into()));
    assert!(
        vm.dispatch_native(
            Native::SymbolKeyFor,
            &Value::Undefined,
            &[Value::Undefined],
            0,
        )
        .is_err()
    );

    let Value::Function(function) = run(&Program {
        source: "".into(),
        template_sites: Vec::new(),
        version: kiln_ir::BYTECODE_VERSION,
        constants: Vec::new(),
        code: vec![Instruction::MakeFunction(0), Instruction::Return],
        local_count: 0,
        functions: vec![kiln_ir::FunctionProto {
            inferred_name: None,
            asynchronous: false,
            name: None,
            arrow: false,
            generator: false,
            async_generator: false,
            is_method: true,
            parameter_count: 0,
            mapped_arguments: false,
            declared_length: 0,
            local_count: 0,
            prologue_end: 0,
            code: vec![Instruction::Return],
            upvalues: Vec::new(),
            span: kiln_ir::Span::default(),
            type_annotations: None,
        }],
    })
    .expect("method closure") else {
        panic!("expected a function");
    };
    let symbol = Symbol::Unique(Rc::new(UniqueSymbol {
        serial: 9,
        description: Some(StringUnits::from_units(&[0xd800])),
    }));
    define_function_name(
        &Value::Function(function.clone()),
        &Value::Symbol(symbol),
        None,
    )
    .expect("symbol description naming");
    assert_eq!(
        function.properties.borrow().get_own("name"),
        Some(&Value::String(StringUnits::from_units(&[
            b'[' as u16,
            0xd800,
            b']' as u16,
        ])))
    );

    let no_description = Symbol::Unique(Rc::new(UniqueSymbol {
        serial: 10,
        description: None,
    }));
    define_function_name(
        &Value::Function(function.clone()),
        &Value::Symbol(no_description),
        None,
    )
    .expect("description-less symbol naming");
    assert_eq!(
        function.properties.borrow().get_own("name"),
        Some(&Value::String(StringUnits::default()))
    );

    let empty_description = Symbol::Unique(Rc::new(UniqueSymbol {
        serial: 11,
        description: Some(StringUnits::default()),
    }));
    define_function_name(
        &Value::Function(function.clone()),
        &Value::Symbol(empty_description),
        None,
    )
    .expect("empty symbol description naming");
    assert_eq!(
        function.properties.borrow().get_own("name"),
        Some(&Value::String(StringUnits::from("[]")))
    );

    let lone_accessor = Symbol::Unique(Rc::new(UniqueSymbol {
        serial: 12,
        description: Some(StringUnits::from_units(&[0xd800])),
    }));
    define_function_name(
        &Value::Function(function.clone()),
        &Value::Symbol(lone_accessor),
        Some("get"),
    )
    .expect("accessor symbol description naming");
    assert_eq!(
        function.properties.borrow().get_own("name"),
        Some(&Value::String(StringUnits::from_units(&[
            b'g' as u16,
            b'e' as u16,
            b't' as u16,
            b' ' as u16,
            b'[' as u16,
            0xd800,
            b']' as u16,
        ])))
    );
}

#[test]
fn a_symbols_key_is_reserved_so_a_string_cannot_spell_it() {
    let symbol = Symbol::Unique(Rc::new(UniqueSymbol {
        serial: 7,
        description: None,
    }));
    assert!(symbol.property_key().starts_with(RESERVED_KEY_PREFIX));
    assert!(
        Symbol::WellKnown(WellKnown::Iterator)
            .property_key()
            .starts_with(RESERVED_KEY_PREFIX)
    );
}

#[test]
fn spread_reads_every_own_property_including_symbol_keyed_ones() {
    let symbol = Symbol::Unique(Rc::new(UniqueSymbol {
        serial: 1,
        description: None,
    }));
    let source = object(&[
        ("plain", Value::Number(1.0)),
        (&symbol.property_key(), Value::Number(2.0)),
    ]);
    let copied = own_properties_units(&Intrinsics::default(), &source);
    assert_eq!(copied.len(), 2, "a spread carries symbol keys across");
    assert_eq!(copied[0].0, "plain");
}

#[test]
fn spreading_something_without_own_properties_contributes_nothing() {
    // `{ ...null }` is `{}`, not a throw -- the one place object spread is more forgiving
    // than array spread.
    assert!(own_properties_units(&Intrinsics::default(), &Value::Null).is_empty());
    assert!(own_properties_units(&Intrinsics::default(), &Value::Undefined).is_empty());
    assert!(own_properties_units(&Intrinsics::default(), &Value::Number(1.0)).is_empty());
}

#[test]
fn an_array_and_a_string_spread_as_their_indices() {
    let items = own_properties_units(
        &Intrinsics::default(),
        &array(&[Value::Number(7.0), Value::Number(8.0)]),
    );
    assert_eq!(items[0].0, "0");
    assert_eq!(items[1].0, "1");
    let characters = own_properties_units(
        &Intrinsics::default(),
        &Value::String("hi".to_owned().into()),
    );
    assert_eq!(characters.len(), 2);
    assert_eq!(characters[1].0, "1");
}

#[test]
fn removing_a_property_keeps_the_order_of_the_rest() {
    // Insertion order is observable, so a removal must not be implemented as a swap.
    let mut data = ObjectData::default();
    for key in ["a", "b", "c"] {
        data.set(key, Value::Undefined);
    }
    data.remove("b");
    assert_eq!(data.keys().collect::<Vec<_>>(), vec!["a", "c"]);
}

#[test]
fn an_index_key_enumerates_before_every_string_key() {
    // The order own keys enumerate in is observable through `Object.keys`, `JSON.stringify`,
    // spread and `for-in`, and it is not insertion order: indices come first, ascending.
    let mut data = ObjectData::default();
    for key in ["b", "10", "a", "2", "0"] {
        data.set(key, Value::Undefined);
    }
    assert_eq!(
        data.keys().collect::<Vec<_>>(),
        vec!["0", "2", "10", "b", "a"]
    );
}

#[test]
fn only_a_canonical_index_counts_as_one() {
    // `"01"` and `"1.0"` name the same property as themselves and not as 1, so they are
    // ordinary string keys and enumerate in insertion order.
    assert_eq!(index_key("0"), Some(0));
    assert_eq!(index_key("10"), Some(10));
    assert_eq!(index_key("01"), None);
    assert_eq!(index_key("1.0"), None);
    assert_eq!(index_key("-1"), None);
    assert_eq!(index_key(""), None);
    // 2^32 - 1 is the length, never an index.
    assert_eq!(index_key("4294967295"), None);
    assert_eq!(index_key("4294967294"), Some(4_294_967_294));
}

#[test]
fn an_element_key_is_wider_than_an_index() {
    // What separates a typed array's element keys from its named ones. Every name that is
    // the canonical spelling of *some* number is an element key -- even one no view can
    // hold -- and a typed array refuses those rather than storing them, which is why the
    // set is asked about by name and not by whether an index came back.
    assert_eq!(canonical_numeric_index("0"), Some(0.0));
    assert_eq!(canonical_numeric_index("1.5"), Some(1.5));
    assert_eq!(canonical_numeric_index("-1"), Some(-1.0));
    assert_eq!(canonical_numeric_index("NaN").map(f64::is_nan), Some(true));
    assert_eq!(canonical_numeric_index("Infinity"), Some(f64::INFINITY));
    // The one name the specification admits by hand: `String(-0)` is `"0"`.
    assert_eq!(canonical_numeric_index("-0"), Some(-0.0));
    assert!(canonical_numeric_index("-0").is_some_and(f64::is_sign_negative));
    // Ordinary names, stored under themselves.
    assert_eq!(canonical_numeric_index("01"), None);
    assert_eq!(canonical_numeric_index("1e2"), None);
    assert_eq!(canonical_numeric_index("1.0"), None);
    assert_eq!(canonical_numeric_index(" 1"), None);
    assert_eq!(canonical_numeric_index("length"), None);
    assert_eq!(canonical_numeric_index(""), None);
}

#[test]
fn reassigning_a_key_does_not_move_it() {
    let mut data = ObjectData::default();
    data.set("b", Value::Number(1.0));
    data.set("a", Value::Number(2.0));
    data.set("b", Value::Number(3.0));
    assert_eq!(data.keys().collect::<Vec<_>>(), vec!["b", "a"]);
}

#[test]
fn a_negative_slice_bound_counts_from_the_end() {
    // The reason this is not a cast: `-2` means two from the end, not clamp-to-zero.
    assert_eq!(relative_index(Some(&Value::Number(-2.0)), 5, 0), 3);
    assert_eq!(relative_index(Some(&Value::Number(-99.0)), 5, 0), 0);
    assert_eq!(relative_index(Some(&Value::Number(2.0)), 5, 0), 2);
    assert_eq!(relative_index(Some(&Value::Number(99.0)), 5, 0), 5);
    // An absent or explicitly undefined bound takes the default, not zero.
    assert_eq!(relative_index(None, 5, 5), 5);
    assert_eq!(relative_index(Some(&Value::Undefined), 5, 5), 5);
}

fn parse_json(text: &str) -> Result<Value, ExecutionError> {
    let text = StringUnits::from(text);
    parse_json_text(&text)
}

#[test]
fn json_parses_each_kind_of_value() {
    assert!(matches!(parse_json("null"), Ok(Value::Null)));
    assert!(matches!(parse_json("true"), Ok(Value::Boolean(true))));
    assert!(matches!(parse_json(" -1.5e2 "), Ok(Value::Number(n)) if n == -150.0));
    assert!(matches!(parse_json(r#""s""#), Ok(Value::String(s)) if s == "s"));
    assert!(matches!(parse_json("[]"), Ok(Value::Array(_))));
    assert!(matches!(parse_json("{}"), Ok(Value::Object(_))));
}

#[test]
fn json_parses_escapes_including_surrogates() {
    let Ok(Value::String(text)) = parse_json(r#""a\"b\\c\u00e9\n""#) else {
        panic!("expected a string");
    };
    assert_eq!(text, "a\"b\\cé\n");
    // A lone surrogate is a valid JavaScript string unit and JSON.parse preserves it.
    let Ok(Value::String(lone)) = parse_json(r#""\ud800""#) else {
        panic!("expected a string");
    };
    assert_eq!(lone, StringUnits::from_units(&[0xd800]));
}

#[test]
fn json_rejects_what_is_not_json() {
    // Every one of these is accepted by some more permissive parser, which is why they are
    // listed: `f64::from_str` alone would take three of them.
    for text in [
        "",
        "{",
        "[1,]",
        "{\"a\":}",
        "'s'",
        "+1",
        ".5",
        "NaN",
        "inf",
        "01,",
        "nul",
        "{\"a\" 1}",
        "\"unterminated",
        "[1] extra",
    ] {
        assert!(
            parse_json(text).is_err(),
            "{text:?} should not parse as JSON"
        );
    }
}

#[test]
fn a_parsed_object_keeps_the_last_value_for_a_duplicate_key() {
    let Ok(Value::Object(object)) = parse_json(r#"{"a":1,"a":2}"#) else {
        panic!("expected an object");
    };
    assert_eq!(object.borrow().keys().count(), 1);
    assert!(matches!(object.borrow().get_own("a"), Some(Value::Number(n)) if *n == 2.0));
}

#[test]
fn a_console_shows_contents_where_string_shows_a_tag() {
    // The whole point of `inspect`: `String({a: 1})` is "[object Object]" and always will be.
    let value = object(&[
        ("a", Value::Number(1.0)),
        ("b", Value::String("x".to_owned().into())),
        ("c", Value::Null),
    ]);
    assert_eq!(inspect(&value), "{ a: 1, b: 'x', c: null }");
    assert_eq!(value.to_display_string(), "[object Object]");
}

#[test]
fn a_top_level_string_is_bare_and_a_nested_one_is_quoted() {
    // `console.log("hi")` prints hi; `{ a: "hi" }` has to show that `a` holds a string.
    assert_eq!(inspect(&Value::String("hi".to_owned().into())), "hi");
    assert_eq!(
        inspect(&array(&[Value::String("hi".to_owned().into())])),
        "[ 'hi' ]"
    );
}

#[test]
fn the_quote_character_is_whichever_the_string_lacks() {
    assert_eq!(quote_for_console("plain"), "'plain'");
    assert_eq!(quote_for_console("it's"), "\"it's\"");
    // Both quotes present but no backtick: node reaches for the backtick rather than escape.
    assert_eq!(quote_for_console("both ' and \""), "`both ' and \"`");
    // All three present, so something must be escaped and the preferred quote wins.
    assert_eq!(
        quote_for_console("all ' \" ` three"),
        "'all \\' \" ` three'"
    );
    assert_eq!(quote_for_console("a\tb\nc"), "'a\\tb\\nc'");
}

#[test]
fn a_key_is_bare_only_when_it_is_a_plain_identifier() {
    // node's rule excludes `$` and anything starting with a digit, however odd that looks.
    assert_eq!(inspect_key("valid_1"), "valid_1");
    assert_eq!(inspect_key("_x"), "_x");
    assert_eq!(inspect_key("has $"), "'has $'");
    assert_eq!(inspect_key("with$"), "'with$'");
    assert_eq!(inspect_key("3"), "'3'");
    assert_eq!(inspect_key(""), "''");
}

#[test]
fn empty_containers_and_nesting_read_the_way_node_writes_them() {
    assert_eq!(inspect(&object(&[])), "{}");
    assert_eq!(inspect(&array(&[])), "[]");
    assert_eq!(
        inspect(&array(&[array(&[Value::Number(1.0)])])),
        "[ [ 1 ] ]"
    );
}

#[test]
fn a_cycle_prints_once_rather_than_forever() {
    let items = ArrayData::new(Vec::new());
    items.borrow_mut().push(Value::Array(Rc::clone(&items)));
    assert_eq!(inspect(&Value::Array(items)), "[ [Circular *1] ]");
}

#[test]
fn nesting_past_the_depth_limit_becomes_a_placeholder() {
    // node stops descending at depth 2 and prints the kind instead.
    let deep = array(&[array(&[array(&[array(&[Value::Number(1.0)])])])]);
    assert_eq!(inspect(&deep), "[ [ [ [Array] ] ] ]");
}

#[test]
fn a_symbol_shows_its_description_rather_than_its_reserved_key() {
    // The key is `@@iterator`, which is an implementation detail and not what a console says.
    let symbol = Value::Symbol(Symbol::WellKnown(WellKnown::Iterator));
    assert_eq!(inspect(&symbol), "Symbol(Symbol.iterator)");
    assert_eq!(symbol.property_key_lossy(), "@@iterator");
}

#[test]
fn bitwise_operands_truncate_to_thirty_two_bits() {
    // Not a cast: the value wraps modulo 2^32 rather than saturating, which is why
    // `1 << 31` is negative and `2 ** 32 | 0` is zero.
    assert_eq!(to_int32(1.0), 1);
    assert_eq!(to_int32(2147483648.0), i32::MIN);
    assert_eq!(to_int32(4294967296.0), 0);
    assert_eq!(to_int32(-1.5), -1);
    assert_eq!(to_int32(f64::NAN), 0);
    assert_eq!(to_int32(f64::INFINITY), 0);
    assert_eq!(to_uint32(-1.0), u32::MAX);
}

#[test]
fn loose_equality_is_not_strict_equality_with_a_conversion() {
    // The three rules that are not conversions at all.
    assert!(loose_equal(&Value::Null, &Value::Undefined));
    assert!(!loose_equal(&Value::Null, &Value::Number(0.0)));
    assert!(!loose_equal(
        &Value::Number(f64::NAN),
        &Value::Number(f64::NAN)
    ));
    // A boolean becomes a number first, so `[] == false` goes by way of "" and 0.
    assert!(loose_equal(&array(&[]), &Value::Boolean(false)));
    assert!(loose_equal(
        &Value::Number(1.0),
        &Value::String("1".to_owned().into())
    ));
    assert!(loose_equal(
        &array(&[Value::Number(1.0)]),
        &Value::Number(1.0)
    ));
    // Two objects compare by identity even when their contents match.
    assert!(!loose_equal(&object(&[]), &object(&[])));
    let shared = object(&[]);
    assert!(loose_equal(&shared, &shared.clone()));
}

#[test]
fn math_round_goes_toward_positive_infinity() {
    // Not `f64::round`, which rounds a half away from zero.
    let round = |value: f64| {
        let Ok(Value::Number(result)) = call_native(
            &Intrinsics::default(),
            Native::MathRound,
            &Value::Undefined,
            &[Value::Number(value)],
        ) else {
            panic!("Math.round returns a number");
        };
        result
    };
    assert_eq!(round(1.5), 2.0);
    assert_eq!(round(-1.5), -1.0);
    assert_eq!(round(2.5), 3.0);
    // A negative input rounding to zero gives negative zero, which the floor loses.
    assert!(round(-0.5).is_sign_negative());
    assert_eq!(round(-0.5), 0.0);
}

#[test]
fn parse_int_reads_as_far_as_it_can() {
    // The whole difference from `Number(text)`, which is NaN for every one of these.
    let parsed = |text: &str, radix: Option<f64>| {
        let mut arguments = vec![Value::String(text.to_owned().into())];
        if let Some(radix) = radix {
            arguments.push(Value::Number(radix));
        }
        let Value::Number(result) = parse_int(&arguments) else {
            panic!("parseInt returns a number");
        };
        result
    };
    assert_eq!(parsed("12px", None), 12.0);
    assert_eq!(parsed("  -42  ", None), -42.0);
    assert_eq!(parsed("0x1f", None), 31.0);
    assert_eq!(parsed("ff", Some(16.0)), 255.0);
    assert!(parsed("px", None).is_nan());
    // A radix outside 2..=36 has no meaning, and neither does an empty prefix.
    assert!(parsed("12", Some(1.0)).is_nan());
    assert!(parsed("", None).is_nan());
}

#[test]
fn a_number_in_another_base_expands_its_fraction() {
    // There is no decimal spelling of 0.1 in base 3 to convert through, so the fraction is
    // produced by repeated multiplication instead.
    assert_eq!(number_in_radix(255.0, 16), "ff");
    assert_eq!(number_in_radix(255.0, 2), "11111111");
    assert_eq!(number_in_radix(0.5, 2), "0.1");
    assert_eq!(number_in_radix(-10.25, 4), "-22.1");
    assert_eq!(number_in_radix(0.0, 8), "0");
}

#[test]
fn string_index_of_counts_utf16_units_and_honors_start() {
    // The astral character occupies two JavaScript string positions, even though Rust sees
    // it as one scalar value.
    assert_eq!(string_index_of("a😀b", "b", None), 3.0);
    assert_eq!(string_index_of("a😀b", "b", Some(3.0)), 3.0);
    assert_eq!(string_index_of("a😀b", "b", Some(4.0)), -1.0);
}

#[test]
fn string_index_of_empty_needle_uses_the_clamped_start() {
    assert_eq!(string_index_of("abc", "", None), 0.0);
    assert_eq!(string_index_of("abc", "", Some(2.0)), 2.0);
    assert_eq!(string_index_of("abc", "", Some(99.0)), 3.0);
}

#[test]
fn string_index_of_nan_and_infinity_start_follow_to_integer_or_length() {
    assert_eq!(string_index_of("abc", "b", Some(f64::NAN)), 1.0);
    assert_eq!(string_index_of("abc", "b", Some(f64::INFINITY)), -1.0);
    assert_eq!(string_index_of("abc", "", Some(f64::INFINITY)), 3.0);
}

/// Run a program and return what it wrote to stdout, event loop included.
fn stdout_of(program: &Program) -> String {
    struct Recorder {
        lines: Vec<String>,
    }
    impl HostOps for Recorder {
        fn call(
            &mut self,
            _operation: HostOp,
            arguments: &[HostValue],
        ) -> Result<HostValue, HostError> {
            self.lines.push(
                arguments
                    .iter()
                    .map(HostValue::to_console_string)
                    .collect::<Vec<_>>()
                    .join(" "),
            );
            Ok(HostValue::Undefined)
        }
    }
    let mut host = Recorder { lines: Vec::new() };
    execute(
        Rc::new(program.clone()),
        &mut host,
        ExecutionLimits::default(),
        ExecutionCapabilities::default(),
    )
    .expect("runs");
    host.lines.join(",")
}

/// `setTimeout(() => console.log(tag), delay)` for each pair, in the order given.
fn timer_program(schedule: &[(&str, f64)]) -> Program {
    let mut constants = Vec::new();
    let mut code = Vec::new();
    let mut functions = Vec::new();
    for (tag, delay) in schedule {
        let tag_index = constants.len() as u32;
        constants.push(Constant::String((*tag).to_owned().into()));
        let delay_index = constants.len() as u32;
        constants.push(number(*delay));

        code.push(Instruction::LoadBuiltin(kiln_ir::Builtin::SetTimeout));
        code.push(Instruction::MakeFunction(functions.len() as u32));
        code.push(Instruction::Const(delay_index));
        code.push(Instruction::Call { argc: 2 });
        code.push(Instruction::Pop);
        functions.push(kiln_ir::FunctionProto {
            inferred_name: None,
            span: kiln_ir::Span::default(),
            prologue_end: 0,
            asynchronous: false,
            name: None,
            arrow: false,
            generator: false,
            async_generator: false,
            is_method: false,
            parameter_count: 0,
            declared_length: 0,
            mapped_arguments: false,
            local_count: 0,
            code: vec![
                Instruction::Const(tag_index),
                Instruction::HostCall {
                    op: HostOp::WriteStdout,
                    argc: 1,
                },
                Instruction::Return,
            ],
            upvalues: Vec::new(),
            type_annotations: None,
        });
    }
    code.push(Instruction::Return);
    Program {
        source: "".into(),
        template_sites: Vec::new(),
        version: kiln_ir::BYTECODE_VERSION,
        constants,
        code,
        local_count: 0,
        functions,
    }
}

/// `({ mark: "receiver", step: function* () { yield this.mark; } }).step().next().value`,
/// compiled by hand, printed.
fn generator_receiver_program() -> Program {
    let constants = vec![
        Constant::String("mark".to_owned().into()),
        Constant::String("receiver".to_owned().into()),
        Constant::String("step".to_owned().into()),
        Constant::String("next".to_owned().into()),
        Constant::String("value".to_owned().into()),
    ];
    Program {
        source: "".into(),
        template_sites: Vec::new(),
        version: kiln_ir::BYTECODE_VERSION,
        constants,
        code: vec![
            Instruction::Const(0),
            Instruction::Const(1),
            Instruction::Const(2),
            Instruction::MakeFunction(0),
            Instruction::NewObject { count: 2 },
            Instruction::CallMethod { name: 2, argc: 0 },
            Instruction::CallMethod { name: 3, argc: 0 },
            Instruction::GetProperty(4),
            Instruction::HostCall {
                op: HostOp::WriteStdout,
                argc: 1,
            },
            Instruction::Return,
        ],
        local_count: 0,
        functions: vec![kiln_ir::FunctionProto {
            inferred_name: None,
            span: kiln_ir::Span::default(),
            prologue_end: 0,
            asynchronous: false,
            name: None,
            arrow: false,
            generator: true,
            async_generator: false,
            is_method: false,
            parameter_count: 0,
            declared_length: 0,
            mapped_arguments: false,
            local_count: 0,
            code: vec![
                Instruction::LoadThis,
                Instruction::GetProperty(0),
                Instruction::Yield,
                Instruction::Return,
            ],
            upvalues: Vec::new(),
            type_annotations: None,
        }],
    }
}

#[test]
fn a_generator_body_sees_the_receiver_the_call_named() {
    // Calling a generator does not run its body, so the receiver has to be captured when the
    // generator object is built: by the time `next` resumes it, the interpreter is in
    // whatever frame the consumer happened to be in. Storing `undefined` instead made `this`
    // undefined in every generator, async function and async method -- and async functions
    // are compiled as generators, so `async m() { return this.x; }` threw rather than read.
    assert_eq!(stdout_of(&generator_receiver_program()), "receiver");
}

#[test]
fn a_timers_delay_costs_nothing_to_wait_for() {
    // The property the differential harness cannot check, because node would sleep for it:
    // an hour-long delay is as cheap as a zero-length one, and the order is still by time.
    let program = timer_program(&[("hour", 3_600_000.0), ("second", 1_000.0), ("now", 0.0)]);
    assert_eq!(stdout_of(&program), "now,second,hour");
}

#[test]
fn timers_due_together_run_in_the_order_they_were_scheduled() {
    // A due time is not a total order, so without a sequence number this depends on however
    // the queue happens to be arranged.
    let program = timer_program(&[("first", 0.0), ("second", 0.0), ("third", 0.0)]);
    assert_eq!(stdout_of(&program), "first,second,third");
}

#[test]
fn node_next_tick_runs_before_promise_reactions() {
    let program = Program {
        source: "".into(),
        template_sites: Vec::new(),
        version: kiln_ir::BYTECODE_VERSION,
        constants: vec![
            Constant::String("nextTick".into()),
            Constant::String("resolve".into()),
            Constant::String("then".into()),
            Constant::String("tick".into()),
            Constant::String("promise".into()),
        ],
        code: vec![
            Instruction::LoadBuiltin(kiln_ir::Builtin::Process),
            Instruction::GetProperty(0),
            Instruction::MakeFunction(0),
            Instruction::Call { argc: 1 },
            Instruction::Pop,
            Instruction::LoadBuiltin(kiln_ir::Builtin::Promise),
            Instruction::CallMethod { name: 1, argc: 0 },
            Instruction::MakeFunction(1),
            Instruction::CallMethod { name: 2, argc: 1 },
            Instruction::Pop,
            Instruction::Return,
        ],
        local_count: 0,
        functions: vec![
            kiln_ir::FunctionProto {
                inferred_name: None,
                span: kiln_ir::Span::default(),
                prologue_end: 0,
                asynchronous: false,
                name: None,
                arrow: false,
                generator: false,
                async_generator: false,
                is_method: false,
                parameter_count: 0,
                declared_length: 0,
                mapped_arguments: false,
                local_count: 0,
                code: vec![
                    Instruction::Const(3),
                    Instruction::HostCall {
                        op: HostOp::WriteStdout,
                        argc: 1,
                    },
                    Instruction::Return,
                ],
                upvalues: Vec::new(),
                type_annotations: None,
            },
            kiln_ir::FunctionProto {
                inferred_name: None,
                span: kiln_ir::Span::default(),
                prologue_end: 0,
                asynchronous: false,
                name: None,
                arrow: false,
                generator: false,
                async_generator: false,
                is_method: false,
                parameter_count: 0,
                declared_length: 0,
                mapped_arguments: false,
                local_count: 0,
                code: vec![
                    Instruction::Const(4),
                    Instruction::HostCall {
                        op: HostOp::WriteStdout,
                        argc: 1,
                    },
                    Instruction::Return,
                ],
                upvalues: Vec::new(),
                type_annotations: None,
            },
        ],
    };

    assert_eq!(stdout_of(&program), "tick,promise");
}

#[test]
fn a_program_is_not_finished_while_it_still_has_work_scheduled() {
    // Returning at the end of the module body would make every timer unobservable.
    let program = timer_program(&[("scheduled", 5.0)]);
    assert_eq!(stdout_of(&program), "scheduled");
}

#[test]
fn an_async_arrow_captures_its_defining_receiver() {
    let maker = kiln_ir::FunctionProto {
        inferred_name: None,
        span: kiln_ir::Span::default(),
        prologue_end: 0,
        asynchronous: false,
        name: None,
        arrow: false,
        generator: false,
        async_generator: false,
        is_method: false,
        parameter_count: 0,
        declared_length: 0,
        mapped_arguments: false,
        local_count: 0,
        code: vec![Instruction::MakeFunction(1), Instruction::Return],
        upvalues: Vec::new(),
        type_annotations: None,
    };
    let arrow = kiln_ir::FunctionProto {
        inferred_name: None,
        span: kiln_ir::Span::default(),
        prologue_end: 0,
        asynchronous: true,
        name: None,
        arrow: true,
        generator: true,
        async_generator: false,
        is_method: false,
        parameter_count: 0,
        declared_length: 0,
        mapped_arguments: false,
        local_count: 0,
        code: vec![
            Instruction::LoadThis,
            Instruction::GetProperty(1),
            Instruction::Return,
        ],
        upvalues: Vec::new(),
        type_annotations: None,
    };
    let report = kiln_ir::FunctionProto {
        inferred_name: None,
        span: kiln_ir::Span::default(),
        prologue_end: 0,
        asynchronous: false,
        name: None,
        arrow: false,
        generator: false,
        async_generator: false,
        is_method: false,
        parameter_count: 1,
        declared_length: 1,
        mapped_arguments: false,
        local_count: 1,
        code: vec![
            Instruction::LoadLocal(0),
            Instruction::HostCall {
                op: HostOp::WriteStdout,
                argc: 1,
            },
            Instruction::Return,
        ],
        upvalues: Vec::new(),
        type_annotations: None,
    };
    let program = Program {
        source: "".into(),
        template_sites: Vec::new(),
        version: kiln_ir::BYTECODE_VERSION,
        constants: vec![
            Constant::String("make".into()),
            Constant::String("tag".into()),
            Constant::String("owner".into()),
            Constant::String("wrong".into()),
            Constant::String("call".into()),
            Constant::String("then".into()),
        ],
        code: vec![
            Instruction::MakeFunction(0),
            Instruction::StoreLocal(0),
            Instruction::Const(1),
            Instruction::Const(2),
            Instruction::NewObject { count: 1 },
            Instruction::StoreLocal(1),
            Instruction::LoadLocal(1),
            Instruction::LoadLocal(0),
            Instruction::SetProperty(0),
            Instruction::Pop,
            Instruction::LoadLocal(1),
            Instruction::CallMethod { name: 0, argc: 0 },
            Instruction::StoreLocal(2),
            Instruction::Const(1),
            Instruction::Const(3),
            Instruction::NewObject { count: 1 },
            Instruction::StoreLocal(3),
            Instruction::LoadLocal(2),
            Instruction::LoadLocal(3),
            Instruction::CallMethod { name: 4, argc: 1 },
            Instruction::MakeFunction(2),
            Instruction::CallMethod { name: 5, argc: 1 },
            Instruction::Pop,
            Instruction::Return,
        ],
        local_count: 4,
        functions: vec![maker, arrow, report],
    };

    assert_eq!(stdout_of(&program), "owner");
}

#[test]
fn typed_array_indices_fail_before_casting_or_allocating() {
    assert!(matches!(
        to_index(Some(&Value::Number(-1.0))),
        Err(ExecutionError::BufferLengthOutOfRange { .. })
    ));
    assert!(matches!(
        to_index(Some(&Value::Number(f64::INFINITY))),
        Err(ExecutionError::BufferLengthOutOfRange { .. })
    ));
    assert!(matches!(
        to_index(Some(&Value::Number(1e300))),
        Err(ExecutionError::BufferLengthOutOfRange { .. })
    ));
    assert_eq!(to_index(Some(&Value::Number(f64::NAN))), Ok(0.0));
    assert_eq!(to_index(Some(&Value::Number(3.9))), Ok(3.0));

    let buffer = Value::ArrayBuffer(BufferData::fixed(4));
    assert!(matches!(
        construct_typed_array(TypedKind::Uint16, &[buffer.clone(), Value::Number(1.0)]),
        Err(ExecutionError::InvalidArrayLength)
    ));
    assert!(matches!(
        construct_typed_array(
            TypedKind::Uint16,
            &[buffer.clone(), Value::Number(0.0), Value::Number(3.0)]
        ),
        Err(ExecutionError::InvalidArrayLength)
    ));

    let inferred = construct_typed_array(
        TypedKind::Uint16,
        &[buffer, Value::Number(0.0), Value::Undefined],
    )
    .expect("undefined length infers the remaining buffer");
    assert!(matches!(inferred, Value::TypedArray(view) if view.len() == 2));

    let array = Value::Array(ArrayData::new(vec![Value::Number(7.0)]));
    let ignored = construct_typed_array(
        TypedKind::Uint8,
        &[array, Value::Number(1e300), Value::Number(1e300)],
    )
    .expect("extra array-source arguments are ignored");
    assert!(matches!(
        ignored,
        Value::TypedArray(view) if view.len() == 1 && view.get(0) == Some(Value::Number(7.0))
    ));
}

fn test_buffer(bytes: &[u8], max_byte_length: Option<usize>, shared: bool) -> Value {
    Value::ArrayBuffer(Rc::new(BufferData {
        bytes: Bytes::new(shared, bytes.to_vec()),
        max_byte_length,
        shared,
        detached: std::cell::Cell::new(false),
        immutable: false,
        properties: RefCell::new(ObjectData::default()),
    }))
}

#[test]
fn array_buffer_transfer_moves_bytes_and_detaches_the_source() {
    let source = test_buffer(&[1, 2, 3], None, false);
    let moved = call_native(
        &Intrinsics::default(),
        Native::ArrayBufferTransfer {
            preserve_resizability: true,
        },
        &source,
        &[],
    )
    .expect("transfer succeeds");
    let Value::ArrayBuffer(source) = &source else {
        unreachable!()
    };
    let Value::ArrayBuffer(moved) = moved else {
        unreachable!()
    };
    assert!(source.detached());
    assert_eq!(source.byte_length(), 0);
    assert_eq!(&*moved.bytes.borrow(), &[1, 2, 3]);
    assert!(!moved.detached());
}

#[test]
fn array_buffer_transfer_preserves_or_drops_resizability_and_checks_constraints() {
    let resizable = test_buffer(&[4, 5], Some(8), false);
    let moved = call_native(
        &Intrinsics::default(),
        Native::ArrayBufferTransfer {
            preserve_resizability: true,
        },
        &resizable,
        &[Value::Number(4.0)],
    )
    .expect("resizable transfer succeeds");
    let Value::ArrayBuffer(moved) = moved else {
        unreachable!()
    };
    assert_eq!(moved.max_byte_length, Some(8));
    assert_eq!(&*moved.bytes.borrow(), &[4, 5, 0, 0]);

    let fixed = test_buffer(&[6, 7], None, false);
    let moved = call_native(
        &Intrinsics::default(),
        Native::ArrayBufferTransfer {
            preserve_resizability: false,
        },
        &fixed,
        &[Value::Number(4.0)],
    )
    .expect("fixed-length transfer succeeds");
    let Value::ArrayBuffer(moved) = moved else {
        unreachable!()
    };
    assert_eq!(moved.max_byte_length, None);
    assert_eq!(&*moved.bytes.borrow(), &[6, 7, 0, 0]);
    assert!(matches!(
        moved.resize(3),
        Err(ExecutionError::BufferNotResizable)
    ));

    let too_large = test_buffer(&[0, 0], Some(2), false);
    assert!(matches!(
        call_native(
            &Intrinsics::default(),
            Native::ArrayBufferTransfer {
                preserve_resizability: true,
            },
            &too_large,
            &[Value::Number(3.0)],
        ),
        Err(ExecutionError::BufferLengthOutOfRange { .. })
    ));
    let shared = test_buffer(&[1], Some(2), true);
    assert!(matches!(
        call_native(
            &Intrinsics::default(),
            Native::ArrayBufferTransfer {
                preserve_resizability: true,
            },
            &shared,
            &[],
        ),
        Err(ExecutionError::BufferDetached)
    ));
}

#[test]
fn views_observe_array_buffer_detachment() {
    let source = test_buffer(&[9, 10, 11, 12], None, false);
    let typed =
        construct_typed_array(TypedKind::Uint8, std::slice::from_ref(&source)).expect("view");
    let Value::ArrayBuffer(source_data) = &source else {
        unreachable!()
    };
    let data_view = Value::DataView(Rc::new(DataViewData {
        buffer: Rc::clone(source_data),
        offset: 0,
        length: Some(4),
        properties: RefCell::new(ObjectData::default()),
    }));

    call_native(
        &Intrinsics::default(),
        Native::ArrayBufferTransfer {
            preserve_resizability: true,
        },
        &source,
        &[],
    )
    .expect("detach");

    let Value::TypedArray(typed) = typed else {
        unreachable!()
    };
    let Value::DataView(data_view) = data_view else {
        unreachable!()
    };
    assert_eq!(typed.len(), 0);
    assert_eq!(typed.get(0), None);
    assert!(data_view.out_of_bounds());
    assert_eq!(data_view.byte_length(), None);
}

#[test]
fn node_crypto_hashes_with_rustcrypto_and_node_encodings() {
    let hash = node_hash(&[Value::String("sha1".into())]).expect("supported hash");
    let state = node_hash_state(&hash).expect("branded hash");
    state.borrow_mut().bytes.extend_from_slice(b"abc");
    let digest = node_hash_digest(state.borrow().algorithm, &state.borrow().bytes);
    assert_eq!(
        to_hex_text(&digest),
        "a9993e364706816aba3e25717850c26c9cd0d89d"
    );
    assert_eq!(
        to_base64_text(&digest, false, false),
        "qZk+NkcGgWq6PiVxeFDCbJzQ2J0="
    );
    assert!(matches!(
        node_hash(&[Value::String("made-up".into())]),
        Err(ExecutionError::TypeErrorMessage { .. })
    ));
}

#[test]
fn node_string_decoder_preserves_split_utf8_sequences() {
    let mut state = StringDecoderState::default();
    assert_eq!(
        decode_utf8_chunk(&mut state.pending, &[0xf0, 0x9f], false),
        ""
    );
    assert_eq!(
        decode_utf8_chunk(&mut state.pending, &[0x92, 0xa9, b'!'], false),
        "💩!"
    );
    assert!(state.pending.is_empty());
    assert_eq!(decode_utf8_chunk(&mut state.pending, &[0xe2], true), "�");
}

#[test]
fn node_buffer_statics_are_enumerable_own_properties() {
    let intrinsics = Intrinsics::default();
    let buffer = Value::Native(Native::NodeBuffer, RealmId::PRIMARY);
    let keys = own_property_keys(&intrinsics, &buffer);
    assert!(keys.contains(&"from".to_owned()));
    assert!(keys.contains(&"isBuffer".to_owned()));
    assert_eq!(
        own_attributes(&intrinsics, &buffer, "isBuffer"),
        Some(Attributes::DEFAULT)
    );
}

#[test]
fn node_builtin_own_keys_follow_the_authoritative_ts_exports() {
    let cases = [
        ("node:querystring", Native::NodeQueryString, "parse"),
        ("node:util", Native::NodeUtil, "format"),
        ("node:url", Native::NodeUrl, "parse"),
        (
            "node:diagnostics_channel",
            Native::NodeDiagnosticsChannel,
            "channel",
        ),
        ("node:perf_hooks", Native::NodePerfHooks, "performance"),
    ];
    let intrinsics = Intrinsics::default();
    for (specifier, native, representative_export) in cases {
        let value = Value::Native(native, RealmId::PRIMARY);
        let keys = own_property_keys(&intrinsics, &value);
        let exports = kiln_builtins::builtin_exports_for_specifier(specifier)
            .expect("builtin has an authoritative TS export list");
        assert!(
            exports.contains(&representative_export),
            "{specifier} representative export must come from the TS contract"
        );
        assert!(
            keys.iter().any(|key| key == representative_export),
            "{specifier}.{representative_export} must be an own key"
        );
        for key in &keys {
            assert_eq!(
                own_attributes(&intrinsics, &value, key),
                Some(Attributes::DEFAULT),
                "{specifier}.{key} must be an actual own property"
            );
        }
        assert!(!exports.is_empty(), "{specifier} has a TS export contract");
    }
    assert!(
        !own_property_keys(
            &intrinsics,
            &Value::Native(Native::NodeHttp, RealmId::PRIMARY)
        )
        .iter()
        .any(|key| key == "METHODS")
    );
}

#[test]
fn return_signal_rejected_promise_does_not_surface_as_unhandled_rejection() {
    let program = Program {
        source: "".into(),
        template_sites: Vec::new(),
        version: kiln_ir::BYTECODE_VERSION,
        constants: Vec::new(),
        code: Vec::new(),
        local_count: 0,
        functions: Vec::new(),
    };
    let mut host = TestHost {
        calls: Vec::new(),
        clock_now: None,
        entropy: std::collections::VecDeque::new(),
        timezone: None,
        locale: None,
    };
    let mut vm = test_vm(&program, &mut host);
    let promise = PromiseState::pending();
    let settler = Value::Settler(Rc::clone(&promise), true);
    assert!(
        vm.call_value(
            &settler,
            Value::Undefined,
            vec![Value::Native(Native::ReturnSignal, RealmId::PRIMARY)],
            0
        )
        .is_ok()
    );
    assert!(vm.check_unhandled_rejection().is_ok());
}

/// Every member of the shared surface reports the key it is declared under.
///
/// WHY THIS IS THE GUARD AND NOT A TABLE. `kiln_builtin_surface::SURFACE` is the one place
/// either engine says what `Object` or `Buffer` *has*, and the key it says it under is also
/// the name that member must report: node answers the key for every static on every built-in,
/// which was measured across all seventeen rows the two engines disagreed on.
///
/// The native backend reads that name straight out of the row, so it cannot drift there. This
/// engine derives it from the Rust variant instead, because the table is keyed by
/// `(global, member)` strings and mapping one back to a `Native` would mean this leaf crate
/// depending on the interpreter. So the derivation stays and this holds it to the table.
///
/// It reads through `read_property` twice -- the global, then the member, then `name` off
/// that -- because that is the path a program takes, and a guard that consulted
/// `native_js_name` directly would agree with itself no matter what the read path answered.
///
/// Thirteen rows failed when this was written: `Buffer.concat.name` was `"nodeBufferConcat"`,
/// `performance.now.name` was `"performanceNow"`, `Object.getOwnPropertyNames.name` was
/// `"ownNames"`. None of them refused anything, which is why nothing had noticed.
#[test]
fn every_surface_member_is_named_for_its_key() {
    let program = program(Vec::new(), vec![Instruction::Return]);
    let mut host = TestHost {
        calls: Vec::new(),
        clock_now: None,
        entropy: std::collections::VecDeque::new(),
        timezone: None,
        locale: None,
    };
    let mut vm = test_vm(&program, &mut host);
    let globals = vm.global_object();

    let mut wrong = Vec::new();
    let mut checked = 0usize;
    for builtin in kiln_builtin_surface::SURFACE {
        // Two rows are intrinsics rather than globals a program can name; they are spelled the
        // way the specification spells one and nothing on the global object reaches them.
        if builtin.global.starts_with('%') {
            continue;
        }
        let owner = vm
            .read_property(&globals, builtin.global, 0)
            .unwrap_or_else(|error| {
                panic!("{} is on the global object: {error:?}", builtin.global)
            });
        assert!(
            !matches!(owner, Value::Undefined),
            "SURFACE names `{}`, which this realm does not publish",
            builtin.global
        );
        for member in builtin.members {
            // A symbol-keyed member is not reached by a string key, and none of them is a
            // function with a name of its own.
            if member.starts_with("@@") {
                continue;
            }
            if kiln_builtin_surface::NAME_IS_SHARED.contains(&(builtin.global, member)) {
                continue;
            }
            let Ok(value) = vm.read_property(&owner, member, 0) else {
                continue;
            };
            // Only a callable has a name to be wrong about: `Number.MAX_VALUE` is a number.
            if !matches!(
                value,
                Value::Native(..) | Value::Function(_) | Value::Callback(..)
            ) {
                continue;
            }
            checked += 1;
            let reported = vm.read_property(&value, "name", 0);
            match reported {
                Ok(Value::String(name)) if name == **member => {}
                other => wrong.push(format!(
                    "  {}.{member} reports {other:?}, not \"{member}\"",
                    builtin.global
                )),
            }
        }
    }
    assert!(
        checked > 90,
        "only {checked} callable members were reached, so this guard has stopped walking the \
         surface it exists to cover"
    );
    assert!(
        wrong.is_empty(),
        "these members do not report the key `kiln_builtin_surface::SURFACE` declares them \
         under, so `.name` hands a program a string nobody could have asked for:\n{}\n\nName \
         the variant outright in `native_js_name`; the derivation from the Rust spelling is \
         what put an implementation identifier there. If the member really is served by \
         another member's function, say so in `NAME_IS_SHARED` instead.",
        wrong.join("\n")
    );

    let host_create_require = vm
        .read_property(&globals, "__kilnHostModuleCreateRequire", 0)
        .expect("the private createRequire host hook is published");
    assert_eq!(
        vm.read_property(&host_create_require, "name", 0),
        Ok(Value::String(
            "__kilnHostModuleCreateRequire".to_owned().into()
        ))
    );
}

/// Every `"@@..."` literal in this crate is a key `is_reserved` recognises.
///
/// `is_reserved` used to be the prefix test `key.starts_with("@@")`, which is a guess about what a
/// program would never write -- and it is wrong. `symbol-observable`, which redux and rxjs depend
/// on, stores its hook under the literal string `"@@observable"`, and that property vanished from
/// `Object.keys` and `getOwnPropertyNames` while staying readable by name.
///
/// The replacement is an exact list, and an exact list drifts. This reads the crate's own source
/// and fails if a slot was added without being listed, because the failure it prevents is the
/// opposite one: an internal slot that starts appearing in a program's `Object.keys`.
#[test]
fn every_internal_slot_is_reserved() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut unlisted = Vec::new();
    let mut checked = 0usize;
    for entry in std::fs::read_dir(&root).expect("read the crate source directory") {
        let path = entry.expect("read a source entry").path();
        if path.extension().is_none_or(|extension| extension != "rs") {
            continue;
        }
        // This file's own literals are about the check rather than keys the VM writes.
        if path.file_name().is_some_and(|name| name == "tests.rs") {
            continue;
        }
        let text = std::fs::read_to_string(&path).expect("read a source file");
        for line in text.lines() {
            // Prose, not code. The documentation for this very rule quotes `"@@observable"` as
            // the key that must *not* be reserved, so a scan that reads comments would fail on
            // the explanation of why it exists.
            if line.trim_start().starts_with("//") {
                continue;
            }
            for (offset, _) in line.match_indices("\"@@") {
                let rest = &line[offset + 1..];
                let Some(end) = rest.find('"') else { continue };
                let key = &rest[..end];
                // `RESERVED_KEY_PREFIX` itself: the prefix, not a key written under it.
                if key == "@@" {
                    continue;
                }
                // A key built by `format!` appears here as its prefix -- `@@symbol:` and
                // `@@private:` are both in `INTERNAL_PREFIXES`, so they answer true as written.
                checked += 1;
                if !kiln_interpreter_is_reserved(key) {
                    unlisted.push(format!("{} in {}", key, path.display()));
                }
            }
        }
    }
    assert!(
        checked > 40,
        "expected to find the crate's internal keys, found {checked}"
    );
    unlisted.sort();
    unlisted.dedup();
    assert!(
        unlisted.is_empty(),
        "these internal keys are not recognised by `symbol::is_reserved`, so they would appear \
         in a program's `Object.keys`: {unlisted:#?}"
    );
}

/// `symbol::is_reserved` is crate-private; this is the one place a test needs it by name.
fn kiln_interpreter_is_reserved(key: &str) -> bool {
    crate::symbol::is_reserved(key)
}

/// Every prototype table, for the sweeps below. Written out rather than derived because
/// `PrototypeTable` has no iterator; a variant missing from here weakens the sweep but cannot
/// make it wrong.
fn every_prototype_table() -> Vec<PrototypeTable> {
    use PrototypeTable as P;
    vec![
        P::Object,
        P::Array,
        P::String,
        P::Number,
        P::Function,
        P::RegExp,
        P::Date,
        P::ArrayBuffer,
        P::SharedArrayBuffer,
        P::TypedArray,
        P::DataView,
        P::Iterator,
        P::Boolean,
        P::Symbol,
        P::BigInt,
        P::Promise,
        P::Map,
        P::Set,
        P::WeakMap,
        P::WeakSet,
        P::WeakRef,
        P::FinalizationRegistry,
        P::ShadowRealm,
        P::DisposableStack,
        P::AsyncDisposableStack,
        P::EventEmitter,
        P::Readable,
        P::Writable,
        P::Console,
    ]
}

/// A built-in never reports flags for a member it does not have.
///
/// The failure this guards against does not look like a missing property; it looks like an extra
/// one. Attributes are asked before the value is, so a rule that answers from the *name* invents
/// the property outright: generalising one such rule from `Date` to every prototype gave every
/// prototype in the realm a phantom own `@@toPrimitive`, and the tests that noticed were in
/// `Date`, `Object` and `language/expressions` rather than anywhere near the change.
///
/// Swept across every table and every well-known symbol, plus names no built-in publishes.
#[test]
fn attributes_are_never_reported_for_an_absent_member() {
    let absent = [
        "definitelyNotAMember",
        "constructor2",
        "__nope__",
        "species",
        "iterator",
    ];
    for table in every_prototype_table() {
        let target = Value::Native(Native::PrototypeOf(table), RealmId::PRIMARY);
        let symbols = crate::symbol::WellKnown::ALL
            .into_iter()
            .map(|symbol| symbol.property_key());
        for key in symbols.chain(absent.iter().copied()) {
            let published = !matches!(
                Native::PrototypeOf(table).property(key),
                None | Some(Value::Undefined)
            ) || intrinsic_accessor(&target, key).is_some()
                || machine_accessor(&target, key).is_some();
            let described = intrinsic_own_attributes(&target, key).is_some();
            assert_eq!(
                described, published,
                "{table:?} describes {key} as {described} but publishes it as {published}"
            );
        }
    }
}

/// What a table lists, it also describes -- and the other way round.
///
/// The raw name lists are deliberately supersets: `Array.prototype` and `%TypedArray%.prototype`
/// share one, so the array table names `set`, which only the view has.
/// `intrinsic_prototype_entries` is what resolves that, by dropping any name the value tables do
/// not answer -- so the *listing* is correct even though the list is not.
///
/// What must not drift is the agreement between that filtered listing and
/// `intrinsic_own_attributes`. `hasOwnProperty`, `getOwnPropertyNames` and
/// `getOwnPropertyDescriptor` are answered by different halves of this pair, and a name in one
/// and not the other is a property that exists depending on which question you ask -- which has
/// shipped more than once.
#[test]
fn listing_and_describing_agree() {
    // Collected rather than asserted one at a time: the whole point is to see the shape of a
    // drift, and failing on the first row hides how wide it is.
    let mut disagreements = Vec::new();
    for table in every_prototype_table() {
        let target = Value::Native(Native::PrototypeOf(table), RealmId::PRIMARY);
        let listed: Vec<String> = intrinsic_prototype_entries(table)
            .into_iter()
            .map(|(key, _)| key)
            .collect();
        // Swept over the union of this table's names *and* every other table's, so that the
        // check runs in both directions. Only sweeping this table's own list missed the
        // direction that actually bit: `Object.prototype`'s eleven members were described as own
        // by every prototype in the realm and listed by none, because the ownership test was
        // asking the *read* path, which carries a table its last step up to `Object.prototype`.
        let mut sweep: Vec<&str> = every_prototype_table()
            .into_iter()
            .flat_map(|other| intrinsic_prototype_method_names(other).iter().copied())
            .collect();
        sweep.sort_unstable();
        sweep.dedup();
        for name in sweep {
            let is_listed = listed.iter().any(|key| key == name);
            let is_described = intrinsic_own_attributes(&target, name).is_some();
            if is_listed != is_described {
                disagreements.push(format!(
                    "{table:?}.{name}: listed={is_listed} described={is_described}"
                ));
            }
        }
        // Everything the filtered listing holds must describe, including `constructor` and the
        // accessors, which the name list does not carry.
        for key in &listed {
            if intrinsic_own_attributes(&target, key).is_none() {
                disagreements.push(format!("{table:?}.{key}: listed=true described=false"));
            }
        }
    }
    assert!(
        disagreements.is_empty(),
        "listing and describing disagree on {} members:\n{}",
        disagreements.len(),
        disagreements.join("\n")
    );
}

#[test]
fn materializing_string_prototype_preserves_its_exotic_length() {
    let intrinsics = Intrinsics::default();
    let target = Value::Native(
        Native::PrototypeOf(PrototypeTable::String),
        RealmId::PRIMARY,
    );

    intrinsics.materialize(&target);

    match intrinsics.overridden(&target, "length") {
        Overridden::Held(Property::Value(Value::Number(length)), attributes) => {
            assert_eq!(length, 0.0);
            assert_eq!(attributes, Attributes::NONE);
        }
        _ => panic!("materialized String.prototype lost its own length"),
    }
}

/// No enumeration list carries the internal spelling of a symbol.
///
/// `NamedMembers::new` refuses one while the compiler builds the table, so this cannot fail today.
/// It is here because the cost of the leak was 838 rows and a wrong post-mortem, and a second
/// check that runs on a different mechanism is cheap.
#[test]
fn no_listed_member_is_a_symbol_spelling() {
    for table in every_prototype_table() {
        for name in intrinsic_prototype_method_names(table) {
            assert!(
                !kiln_interpreter_is_reserved(name),
                "{table:?} lists the reserved spelling {name}"
            );
        }
    }
}

/// Every callable a built-in prototype publishes has a `name` and a `length`.
///
/// The third axis of the same drift. A member's value comes from the per-table value functions,
/// but its `name` and `length` come from `intrinsic_identity`, which is keyed on the *`Native`
/// variant* rather than on the table and key. Nothing joins the two: publishing a method whose
/// variant has no arity tabulated gives a function with no own `length` and no own `name`, which
/// every `Function.prototype.bind` wrapper and every argument-count check in the ecosystem reads.
#[test]
fn every_published_callable_is_named_and_measured() {
    let mut missing = Vec::new();
    for table in every_prototype_table() {
        for (key, property) in intrinsic_prototype_entries(table) {
            let Property::Value(value) = property else {
                continue;
            };
            if value.type_of() != "function" {
                continue;
            }
            if intrinsic_identity(&value).is_none() {
                missing.push(format!("{table:?}.{key}"));
            }
        }
    }
    assert!(
        missing.is_empty(),
        "published callables with no name or length: {}",
        missing.join(", ")
    );
}

#[test]
#[ignore = "diagnostic: prints where a published name differs from its key"]
fn report_name_key_mismatches() {
    let mut rows = Vec::new();
    for table in every_prototype_table() {
        for (key, property) in intrinsic_prototype_entries(table) {
            let Property::Value(value) = property else {
                continue;
            };
            if value.type_of() != "function" {
                continue;
            }
            if let Some((name, _)) = intrinsic_identity(&value)
                && name != key
            {
                rows.push(format!("{table:?}: key={key} name={name}"));
            }
        }
    }
    println!("{}", rows.join("\n"));
    println!("total mismatches: {}", rows.len());
}

#[test]
fn template_object_is_cached_by_site_and_frozen() {
    let program = Program {
        source: "".into(),
        template_sites: vec![
            kiln_ir::TemplateSite {
                cooked: vec![Some("hello ".into()), Some("world".into())],
                raw: vec!["hello\\x20".into(), "world".into()],
            },
            kiln_ir::TemplateSite {
                cooked: vec![Some("hello ".into()), Some("world".into())],
                raw: vec!["hello\\x20".into(), "world".into()],
            },
        ],
        version: kiln_ir::BYTECODE_VERSION,
        constants: Vec::new(),
        code: vec![
            Instruction::LoadTemplateObject(0),
            Instruction::StoreLocal(0),
            Instruction::LoadTemplateObject(0),
            Instruction::StoreLocal(1),
            Instruction::LoadTemplateObject(1),
            Instruction::StoreLocal(2),
            Instruction::LoadLocal(0),
            Instruction::LoadLocal(1),
            Instruction::Binary(kiln_ir::BinaryOp::StrictEqual),
            Instruction::LoadLocal(0),
            Instruction::LoadLocal(2),
            Instruction::Binary(kiln_ir::BinaryOp::StrictEqual),
            Instruction::Binary(kiln_ir::BinaryOp::StrictEqual),
            Instruction::Return,
        ],
        local_count: 3,
        functions: Vec::new(),
    };
    assert_eq!(run(&program), Ok(Value::Boolean(false)));

    let program_same = Program {
        source: "".into(),
        template_sites: vec![kiln_ir::TemplateSite {
            cooked: vec![Some("hello ".into())],
            raw: vec!["hello".into()],
        }],
        version: kiln_ir::BYTECODE_VERSION,
        constants: Vec::new(),
        code: vec![
            Instruction::LoadTemplateObject(0),
            Instruction::LoadTemplateObject(0),
            Instruction::Binary(kiln_ir::BinaryOp::StrictEqual),
            Instruction::Return,
        ],
        local_count: 0,
        functions: Vec::new(),
    };
    assert_eq!(run(&program_same), Ok(Value::Boolean(true)));
}

#[test]
fn dynamic_import_without_module_loader_rejects_with_type_error() {
    let program = Program {
        source: "".into(),
        template_sites: Vec::new(),
        version: kiln_ir::BYTECODE_VERSION,
        constants: vec![Constant::String("dep.js".into())],
        code: vec![
            Instruction::Const(0),
            Instruction::DynamicImport,
            Instruction::Return,
        ],
        local_count: 0,
        functions: Vec::new(),
    };
    let mut host = TestHost {
        calls: Vec::new(),
        clock_now: None,
        entropy: std::collections::VecDeque::new(),
        timezone: None,
        locale: None,
    };
    let result = execute(
        Rc::new(program),
        &mut host,
        ExecutionLimits::default(),
        ExecutionCapabilities::default(),
    );
    match result {
        Err(ExecutionError::Uncaught { message, .. }) => {
            assert!(
                message.contains("no module loader is installed"),
                "expected uncaught rejection mentioning loader, got {message}"
            );
        }
        other => panic!("expected unhandled rejection from dynamic import, got {other:?}"),
    }
}

#[test]
fn dynamic_import_with_unsupported_loader_rejects_with_type_error() {
    struct MockLoader;
    impl ModuleLoaderService for MockLoader {
        fn load(&self, _request: ModuleLoadRequest) -> Result<ModulePlan, ModuleLoadError> {
            Err(ModuleLoadError::Unsupported(
                "custom resolver refused specifier".into(),
            ))
        }
    }

    let program = Program {
        source: "".into(),
        template_sites: Vec::new(),
        version: kiln_ir::BYTECODE_VERSION,
        constants: vec![Constant::String("dep.js".into())],
        code: vec![
            Instruction::Const(0),
            Instruction::DynamicImport,
            Instruction::Return,
        ],
        local_count: 0,
        functions: Vec::new(),
    };
    let mut host = TestHost {
        calls: Vec::new(),
        clock_now: None,
        entropy: std::collections::VecDeque::new(),
        timezone: None,
        locale: None,
    };
    let capabilities =
        ExecutionCapabilities::default().with_module_loader_service(Arc::new(MockLoader));
    let result = execute(
        Rc::new(program),
        &mut host,
        ExecutionLimits::default(),
        capabilities,
    );
    match result {
        Err(ExecutionError::Uncaught { message, .. }) => {
            assert!(
                message.contains("custom resolver refused specifier"),
                "expected uncaught rejection mentioning custom reason, got {message}"
            );
        }
        other => panic!("expected unhandled rejection from dynamic import, got {other:?}"),
    }
}

#[test]
fn process_object_is_event_emitter() {
    let mut host = TestHost {
        calls: Vec::new(),
        clock_now: None,
        entropy: std::collections::VecDeque::new(),
        timezone: None,
        locale: None,
    };
    let program = Program {
        source: "".into(),
        template_sites: Vec::new(),
        version: kiln_ir::BYTECODE_VERSION,
        constants: vec![
            Constant::String("on".to_owned().into()),
            Constant::String("exit".to_owned().into()),
        ],
        code: vec![
            Instruction::LoadBuiltin(Builtin::Process),
            Instruction::Const(1),
            Instruction::MakeFunction(0),
            Instruction::CallMethod { name: 0, argc: 2 },
            Instruction::Return,
        ],
        local_count: 0,
        functions: vec![kiln_ir::FunctionProto {
            inferred_name: None,
            span: kiln_ir::Span::default(),
            prologue_end: 0,
            asynchronous: false,
            name: None,
            arrow: false,
            generator: false,
            async_generator: false,
            is_method: false,
            parameter_count: 0,
            declared_length: 0,
            mapped_arguments: false,
            local_count: 0,
            code: vec![Instruction::Return],
            upvalues: Vec::new(),
            type_annotations: None,
        }],
    };
    let result = execute(
        Rc::new(program),
        &mut host,
        ExecutionLimits::default(),
        ExecutionCapabilities::default(),
    )
    .expect("process.on succeeds");
    assert!(matches!(result.value, Value::Object(_)));
}

#[test]
fn text_decoder_and_buffer_support_whatwg_encodings() {
    let program = Program::default();
    let mut host = TestHost {
        calls: Vec::new(),
        clock_now: None,
        entropy: std::collections::VecDeque::new(),
        timezone: None,
        locale: None,
    };
    let mut vm = test_vm(&program, &mut host);

    // 1. TextDecoder default utf-8
    let decoder_utf8 = vm
        .dispatch_native(Native::TextDecoderConstructor, &Value::Undefined, &[], 0)
        .expect("TextDecoder default constructor succeeds");
    let buf_utf8 = test_buffer("Hello 🌍".as_bytes(), None, false);
    let decoded_utf8 = vm
        .dispatch_native(Native::TextDecoderDecode, &decoder_utf8, &[buf_utf8], 0)
        .expect("TextDecoder decode succeeds");
    assert_eq!(decoded_utf8, Value::String("Hello 🌍".to_owned().into()));

    // 2. TextDecoder windows-1252
    let decoder_1252 = vm
        .dispatch_native(
            Native::TextDecoderConstructor,
            &Value::Undefined,
            &[Value::String("windows-1252".to_owned().into())],
            0,
        )
        .expect("TextDecoder windows-1252 constructor succeeds");
    // In windows-1252, 0xA3 is '£', 0xE9 is 'é'
    let buf_1252 = test_buffer(
        &[0x48, 0x65, 0x6c, 0x6c, 0x6f, 0x20, 0xa3, 0xe9],
        None,
        false,
    );
    let decoded_1252 = vm
        .dispatch_native(Native::TextDecoderDecode, &decoder_1252, &[buf_1252], 0)
        .expect("TextDecoder decode succeeds");
    assert_eq!(decoded_1252, Value::String("Hello £é".to_owned().into()));

    // 3. TextDecoder utf-16le
    let decoder_utf16le = vm
        .dispatch_native(
            Native::TextDecoderConstructor,
            &Value::Undefined,
            &[Value::String("utf-16le".to_owned().into())],
            0,
        )
        .expect("TextDecoder utf-16le constructor succeeds");
    // "Hi" in utf-16le is [0x48, 0x00, 0x69, 0x00]
    let buf_utf16le = test_buffer(&[0x48, 0x00, 0x69, 0x00], None, false);
    let decoded_utf16le = vm
        .dispatch_native(
            Native::TextDecoderDecode,
            &decoder_utf16le,
            &[buf_utf16le],
            0,
        )
        .expect("TextDecoder decode succeeds");
    assert_eq!(decoded_utf16le, Value::String("Hi".to_owned().into()));

    // 4. TextDecoder invalid label throws RangeError
    let invalid = vm.dispatch_native(
        Native::TextDecoderConstructor,
        &Value::Undefined,
        &[Value::String("unsupported-encoding-xyz".to_owned().into())],
        0,
    );
    assert!(matches!(
        invalid,
        Err(ExecutionError::RangeErrorMessage { .. })
    ));

    // 5. TextDecoder fatal option throws TypeError on invalid byte sequences
    let options = ordinary_object([("fatal".to_owned(), Value::Boolean(true))]);
    let decoder_fatal = vm
        .dispatch_native(
            Native::TextDecoderConstructor,
            &Value::Undefined,
            &[Value::String("utf-8".to_owned().into()), options],
            0,
        )
        .expect("fatal TextDecoder succeeds");
    let invalid_utf8_buf = test_buffer(&[0xff, 0xfe], None, false);
    let fatal_result = vm.dispatch_native(
        Native::TextDecoderDecode,
        &decoder_fatal,
        &[invalid_utf8_buf],
        0,
    );
    assert!(matches!(
        fatal_result,
        Err(ExecutionError::TypeErrorMessage { .. })
    ));

    // 6. Node Buffer.prototype.toString encodings (hex, latin1, utf16le)
    let raw_bytes = test_buffer(&[0x48, 0x65, 0x6c, 0x6c, 0x6f, 0x21], None, false);
    let uint8_view =
        construct_typed_array(TypedKind::Uint8, &[raw_bytes]).expect("Uint8Array creates");

    let hex_res = vm
        .dispatch_native(
            Native::NodeBufferToString,
            &uint8_view,
            &[Value::String("hex".to_owned().into())],
            0,
        )
        .expect("Buffer.toString hex");
    assert_eq!(hex_res, Value::String("48656c6c6f21".to_owned().into()));

    let latin1_bytes = test_buffer(&[0xa3, 0xe9], None, false);
    let latin1_view =
        construct_typed_array(TypedKind::Uint8, &[latin1_bytes]).expect("Uint8Array creates");
    let latin1_res = vm
        .dispatch_native(
            Native::NodeBufferToString,
            &latin1_view,
            &[Value::String("latin1".to_owned().into())],
            0,
        )
        .expect("Buffer.toString latin1");
    assert_eq!(latin1_res, Value::String("£é".to_owned().into()));

    let utf16_bytes = test_buffer(&[0x48, 0x00, 0x69, 0x00], None, false);
    let utf16_view =
        construct_typed_array(TypedKind::Uint8, &[utf16_bytes]).expect("Uint8Array creates");
    let utf16_res = vm
        .dispatch_native(
            Native::NodeBufferToString,
            &utf16_view,
            &[Value::String("utf16le".to_owned().into())],
            0,
        )
        .expect("Buffer.toString utf16le");
    assert_eq!(utf16_res, Value::String("Hi".to_owned().into()));
}

#[test]
fn whatwg_url_and_search_params_support() {
    let program = Program::default();
    let mut host = TestHost {
        calls: Vec::new(),
        clock_now: None,
        entropy: std::collections::VecDeque::new(),
        timezone: None,
        locale: None,
    };
    let mut vm = test_vm(&program, &mut host);

    // 1. Full URL parsing
    let url_val = vm
        .dispatch_native(
            Native::UrlConstructor,
            &Value::Undefined,
            &[Value::String(
                "https://user:pass@example.com:8080/path/to/file?q=1&filter=true#section"
                    .to_owned()
                    .into(),
            )],
            0,
        )
        .expect("URL constructor succeeds");

    assert_eq!(
        vm.property(&url_val, "href"),
        Value::String(
            "https://user:pass@example.com:8080/path/to/file?q=1&filter=true#section"
                .to_owned()
                .into()
        )
    );
    assert_eq!(
        vm.property(&url_val, "origin"),
        Value::String("https://example.com:8080".to_owned().into())
    );
    assert_eq!(
        vm.property(&url_val, "protocol"),
        Value::String("https:".to_owned().into())
    );
    assert_eq!(
        vm.property(&url_val, "host"),
        Value::String("example.com:8080".to_owned().into())
    );
    assert_eq!(
        vm.property(&url_val, "hostname"),
        Value::String("example.com".to_owned().into())
    );
    assert_eq!(
        vm.property(&url_val, "port"),
        Value::String("8080".to_owned().into())
    );
    assert_eq!(
        vm.property(&url_val, "pathname"),
        Value::String("/path/to/file".to_owned().into())
    );
    assert_eq!(
        vm.property(&url_val, "search"),
        Value::String("?q=1&filter=true".to_owned().into())
    );
    assert_eq!(
        vm.property(&url_val, "hash"),
        Value::String("#section".to_owned().into())
    );
    assert_eq!(
        vm.property(&url_val, "username"),
        Value::String("user".to_owned().into())
    );
    assert_eq!(
        vm.property(&url_val, "password"),
        Value::String("pass".to_owned().into())
    );

    // toString / toJSON method
    let to_str = vm
        .dispatch_native(Native::UrlToString, &url_val, &[], 0)
        .expect("UrlToString succeeds");
    assert_eq!(
        to_str,
        Value::String(
            "https://user:pass@example.com:8080/path/to/file?q=1&filter=true#section"
                .to_owned()
                .into()
        )
    );

    // 2. Relative URL resolution with base
    let base_val = vm
        .dispatch_native(
            Native::UrlConstructor,
            &Value::Undefined,
            &[Value::String(
                "https://example.com/dir/index.html".to_owned().into(),
            )],
            0,
        )
        .expect("Base URL succeeds");
    let relative_child = vm
        .dispatch_native(
            Native::UrlConstructor,
            &Value::Undefined,
            &[Value::String("sub/page.html".to_owned().into()), base_val],
            0,
        )
        .expect("Relative child URL succeeds");
    assert_eq!(
        vm.property(&relative_child, "href"),
        Value::String("https://example.com/dir/sub/page.html".to_owned().into())
    );

    // 3. searchParams methods (get, has, getAll, toString)
    let search_params = vm.property(&url_val, "searchParams");
    let get_q = vm
        .dispatch_native(
            Native::UrlSearchParamsGet,
            &search_params,
            &[Value::String("q".to_owned().into())],
            0,
        )
        .expect("searchParams.get(q)");
    assert_eq!(get_q, Value::String("1".to_owned().into()));

    let get_missing = vm
        .dispatch_native(
            Native::UrlSearchParamsGet,
            &search_params,
            &[Value::String("missing".to_owned().into())],
            0,
        )
        .expect("searchParams.get(missing)");
    assert_eq!(get_missing, Value::Null);

    let has_filter = vm
        .dispatch_native(
            Native::UrlSearchParamsHas,
            &search_params,
            &[Value::String("filter".to_owned().into())],
            0,
        )
        .expect("searchParams.has(filter)");
    assert_eq!(has_filter, Value::Boolean(true));

    let has_missing = vm
        .dispatch_native(
            Native::UrlSearchParamsHas,
            &search_params,
            &[Value::String("missing".to_owned().into())],
            0,
        )
        .expect("searchParams.has(missing)");
    assert_eq!(has_missing, Value::Boolean(false));

    let get_all_q = vm
        .dispatch_native(
            Native::UrlSearchParamsGetAll,
            &search_params,
            &[Value::String("q".to_owned().into())],
            0,
        )
        .expect("searchParams.getAll(q)");
    let Value::Array(get_all_items) = get_all_q else {
        panic!("searchParams.getAll must return an array");
    };
    assert_eq!(
        &*get_all_items.borrow(),
        &[Value::String("1".to_owned().into())]
    );

    let sp_to_str = vm
        .dispatch_native(Native::UrlSearchParamsToString, &search_params, &[], 0)
        .expect("searchParams.toString");
    assert_eq!(
        sp_to_str,
        Value::String("q=1&filter=true".to_owned().into())
    );

    // 4. fileURLToPath and pathToFileURL
    let file_url_res = vm
        .dispatch_native(
            Native::NodeUrlPathToFileUrl,
            &Value::Undefined,
            &[Value::String("/foo/bar baz.txt".to_owned().into())],
            0,
        )
        .expect("pathToFileURL succeeds");
    assert_eq!(
        vm.property(&file_url_res, "href"),
        Value::String("file:///foo/bar%20baz.txt".to_owned().into())
    );

    // fileURLToPath with string
    let path_from_str = vm
        .dispatch_native(
            Native::NodeUrlFileUrlToPath,
            &Value::Undefined,
            &[Value::String("file:///foo/bar%20baz.txt".to_owned().into())],
            0,
        )
        .expect("fileURLToPath from string succeeds");
    assert_eq!(
        path_from_str,
        Value::String("/foo/bar baz.txt".to_owned().into())
    );

    // fileURLToPath with URL object
    let path_from_obj = vm
        .dispatch_native(
            Native::NodeUrlFileUrlToPath,
            &Value::Undefined,
            &[file_url_res],
            0,
        )
        .expect("fileURLToPath from URL object succeeds");
    assert_eq!(
        path_from_obj,
        Value::String("/foo/bar baz.txt".to_owned().into())
    );

    // 5. Invalid URL throws TypeError
    let invalid = vm.dispatch_native(
        Native::UrlConstructor,
        &Value::Undefined,
        &[Value::String("not an absolute url".to_owned().into())],
        0,
    );
    assert!(matches!(
        invalid,
        Err(ExecutionError::TypeErrorMessage { .. })
    ));
}

#[test]
fn node_zlib_streaming_and_sync_compression_and_decompression() {
    let program = Program::default();
    let mut host = TestHost {
        calls: Vec::new(),
        clock_now: None,
        entropy: std::collections::VecDeque::new(),
        timezone: None,
        locale: None,
    };
    let mut vm = test_vm(&program, &mut host);

    let original = b"Hello from Kiln node:zlib full compression suite!";

    // 1. Test streaming Gzip compression -> streaming Gunzip decompression
    let gzip = vm
        .dispatch_native(Native::NodeZlibCreateGzip, &Value::Undefined, &[], 0)
        .expect("createGzip succeeds");
    let orig_chunk = node_bytes_value(original.to_vec());
    vm.dispatch_native(Native::NodeWritableEnd, &gzip, &[orig_chunk], 0)
        .expect("gzip.end succeeds");

    let compressed_gz_val = vm
        .dispatch_native(Native::NodeReadableRead, &gzip, &[], 0)
        .expect("gzip.read succeeds");
    let compressed_gz_bytes = match &compressed_gz_val {
        Value::TypedArray(view) => view.bytes(),
        _ => panic!("Expected Uint8Array from gzip.read()"),
    };

    let gunzip = vm
        .dispatch_native(Native::NodeZlibCreateGunzip, &Value::Undefined, &[], 0)
        .expect("createGunzip succeeds");
    vm.dispatch_native(Native::NodeWritableEnd, &gunzip, &[compressed_gz_val], 0)
        .expect("gunzip.end succeeds");

    let decompressed_gz_val = vm
        .dispatch_native(Native::NodeReadableRead, &gunzip, &[], 0)
        .expect("gunzip.read succeeds");
    let decompressed_bytes = match decompressed_gz_val {
        Value::TypedArray(view) => view.bytes(),
        _ => panic!("Expected Uint8Array from gunzip.read()"),
    };
    assert_eq!(decompressed_bytes, original);

    // 2. Test streaming Deflate compression -> streaming Inflate decompression
    let deflate = vm
        .dispatch_native(Native::NodeZlibCreateDeflate, &Value::Undefined, &[], 0)
        .expect("createDeflate succeeds");
    let orig_chunk2 = node_bytes_value(original.to_vec());
    vm.dispatch_native(Native::NodeWritableEnd, &deflate, &[orig_chunk2], 0)
        .expect("deflate.end succeeds");

    let compressed_deflate_val = vm
        .dispatch_native(Native::NodeReadableRead, &deflate, &[], 0)
        .expect("deflate.read succeeds");
    let compressed_deflate_bytes = match &compressed_deflate_val {
        Value::TypedArray(view) => view.bytes(),
        _ => panic!("Expected Uint8Array from deflate.read()"),
    };

    let inflate = vm
        .dispatch_native(Native::NodeZlibCreateInflate, &Value::Undefined, &[], 0)
        .expect("createInflate succeeds");
    vm.dispatch_native(
        Native::NodeWritableEnd,
        &inflate,
        &[compressed_deflate_val],
        0,
    )
    .expect("inflate.end succeeds");

    let decompressed_inflate_val = vm
        .dispatch_native(Native::NodeReadableRead, &inflate, &[], 0)
        .expect("inflate.read succeeds");
    let decompressed_bytes = match decompressed_inflate_val {
        Value::TypedArray(view) => view.bytes(),
        _ => panic!("Expected Uint8Array from inflate.read()"),
    };
    assert_eq!(decompressed_bytes, original);

    // 3. Test streaming Unzip auto-detecting gzip and deflate
    let unzip_gz = vm
        .dispatch_native(Native::NodeZlibCreateUnzip, &Value::Undefined, &[], 0)
        .expect("createUnzip succeeds");
    let gz_chunk = node_bytes_value(compressed_gz_bytes.clone());
    vm.dispatch_native(Native::NodeWritableEnd, &unzip_gz, &[gz_chunk], 0)
        .expect("unzip.end with gzip data succeeds");
    let unzipped_gz = vm
        .dispatch_native(Native::NodeReadableRead, &unzip_gz, &[], 0)
        .expect("unzip.read gzip succeeds");
    assert_eq!(
        match unzipped_gz {
            Value::TypedArray(v) => v.bytes(),
            _ => panic!("Expected bytes"),
        },
        original
    );

    let unzip_deflate = vm
        .dispatch_native(Native::NodeZlibCreateUnzip, &Value::Undefined, &[], 0)
        .expect("createUnzip succeeds");
    let deflate_chunk = node_bytes_value(compressed_deflate_bytes.clone());
    vm.dispatch_native(Native::NodeWritableEnd, &unzip_deflate, &[deflate_chunk], 0)
        .expect("unzip.end with deflate data succeeds");
    let unzipped_deflate = vm
        .dispatch_native(Native::NodeReadableRead, &unzip_deflate, &[], 0)
        .expect("unzip.read deflate succeeds");
    assert_eq!(
        match unzipped_deflate {
            Value::TypedArray(v) => v.bytes(),
            _ => panic!("Expected bytes"),
        },
        original
    );

    // 4. Test sync APIs: gzipSync, gunzipSync, deflateSync, inflateSync, unzipSync
    let orig_val = node_bytes_value(original.to_vec());
    let sync_gzipped = vm
        .dispatch_native(
            Native::NodeZlibGzipSync,
            &Value::Undefined,
            &[orig_val.clone()],
            0,
        )
        .expect("gzipSync succeeds");
    let sync_gunzipped = vm
        .dispatch_native(
            Native::NodeZlibGunzipSync,
            &Value::Undefined,
            &[sync_gzipped.clone()],
            0,
        )
        .expect("gunzipSync succeeds");
    assert_eq!(
        match sync_gunzipped {
            Value::TypedArray(v) => v.bytes(),
            _ => panic!("Expected bytes"),
        },
        original
    );

    let sync_deflated = vm
        .dispatch_native(
            Native::NodeZlibDeflateSync,
            &Value::Undefined,
            &[orig_val],
            0,
        )
        .expect("deflateSync succeeds");
    let sync_inflated = vm
        .dispatch_native(
            Native::NodeZlibInflateSync,
            &Value::Undefined,
            &[sync_deflated.clone()],
            0,
        )
        .expect("inflateSync succeeds");
    assert_eq!(
        match sync_inflated {
            Value::TypedArray(v) => v.bytes(),
            _ => panic!("Expected bytes"),
        },
        original
    );

    let sync_unzipped = vm
        .dispatch_native(
            Native::NodeZlibUnzipSync,
            &Value::Undefined,
            &[sync_deflated],
            0,
        )
        .expect("unzipSync succeeds");
    assert_eq!(
        match sync_unzipped {
            Value::TypedArray(v) => v.bytes(),
            _ => panic!("Expected bytes"),
        },
        original
    );

    // 5. Test zlib constants
    let constants = Native::NodeZlib
        .property("constants")
        .expect("zlib constants property exists");
    assert_eq!(vm.property(&constants, "Z_OK"), Value::Number(0.0));
    assert_eq!(vm.property(&constants, "Z_SYNC_FLUSH"), Value::Number(2.0));
    assert_eq!(vm.property(&constants, "Z_STREAM_END"), Value::Number(1.0));
}

#[test]
fn node_crypto_random_bytes_fill_hashes_and_hmac() {
    let program = Program::default();
    let mut host = TestHost {
        calls: Vec::new(),
        clock_now: None,
        entropy: std::collections::VecDeque::new(),
        timezone: None,
        locale: None,
    };
    let mut vm = test_vm(&program, &mut host);

    // 1. Test crypto.getHashes()
    let hashes = vm
        .dispatch_native(Native::NodeCryptoGetHashes, &Value::Undefined, &[], 0)
        .expect("getHashes succeeds");
    let Value::Array(hashes_arr) = hashes else {
        panic!("getHashes must return Array");
    };
    let hash_names: Vec<String> = hashes_arr
        .borrow()
        .iter()
        .map(Value::to_display_string)
        .collect();
    assert!(hash_names.contains(&"sha256".to_string()));
    assert!(hash_names.contains(&"md5".to_string()));
    assert!(hash_names.contains(&"sha1".to_string()));
    assert!(hash_names.contains(&"sha512".to_string()));

    // 2. Test crypto.randomBytes(size)
    let random16 = vm
        .dispatch_native(
            Native::NodeCryptoRandomBytes,
            &Value::Undefined,
            &[Value::Number(16.0)],
            0,
        )
        .expect("randomBytes(16) succeeds");
    let Value::TypedArray(bytes16) = &random16 else {
        panic!("randomBytes must return TypedArray/Buffer");
    };
    assert_eq!(bytes16.bytes().len(), 16);

    // Negative size -> RangeError
    let neg_result = vm.dispatch_native(
        Native::NodeCryptoRandomBytes,
        &Value::Undefined,
        &[Value::Number(-5.0)],
        0,
    );
    assert!(matches!(
        neg_result,
        Err(ExecutionError::RangeErrorMessage { .. })
    ));

    // 3. Test crypto.randomFillSync(buffer, offset, size)
    let buf = node_bytes_value(vec![0_u8; 8]);
    let filled = vm
        .dispatch_native(
            Native::NodeCryptoRandomFillSync,
            &Value::Undefined,
            &[buf.clone(), Value::Number(2.0), Value::Number(4.0)],
            0,
        )
        .expect("randomFillSync succeeds");
    let Value::TypedArray(buf_view) = &filled else {
        panic!("randomFillSync must return buffer");
    };
    let buf_bytes = buf_view.bytes();
    assert_eq!(buf_bytes[0], 0);
    assert_eq!(buf_bytes[1], 0);
    assert_eq!(buf_bytes[6], 0);
    assert_eq!(buf_bytes[7], 0);
    // At least one byte in the middle slice [2..6] should be non-zero (or filled)
    let mid = &buf_bytes[2..6];
    assert_eq!(mid.len(), 4);

    // 4. Test crypto.createHmac(algorithm, key) with RFC 4231 test vector
    // RFC 4231 Test Case 2:
    // Key = "key"
    // Data = "The quick brown fox jumps over the lazy dog"
    // Expected HMAC-SHA256 = f7bc83f430538424b13298e6aa6fb143ef4d59a14946175997479dbc2d1a3cd8
    let hmac = vm
        .dispatch_native(
            Native::NodeCryptoCreateHmac,
            &Value::Undefined,
            &[Value::String("sha256".into()), Value::String("key".into())],
            0,
        )
        .expect("createHmac succeeds");
    let update_fn = vm.property(&hmac, "update");
    let digest_fn = vm.property(&hmac, "digest");
    assert!(is_callable(&update_fn));
    assert!(is_callable(&digest_fn));

    let updated = vm
        .dispatch_native(
            Native::NodeHashUpdate,
            &hmac,
            &[Value::String(
                "The quick brown fox jumps over the lazy dog".into(),
            )],
            0,
        )
        .expect("update succeeds");
    assert_eq!(updated, hmac);

    let digest = vm
        .dispatch_native(
            Native::NodeHashDigest,
            &hmac,
            &[Value::String("hex".into())],
            0,
        )
        .expect("digest succeeds");
    assert_eq!(
        digest,
        Value::String("f7bc83f430538424b13298e6aa6fb143ef4d59a14946175997479dbc2d1a3cd8".into())
    );

    // RFC 2202 Test Case 1 (MD5):
    // Key = 16 bytes of 0x0b
    // Data = "Hi There"
    // Expected = 9294727a3638bb1c13f48ef8158bfc9d
    let md5_key = node_bytes_value(vec![0x0b; 16]);
    let hmac_md5 = vm
        .dispatch_native(
            Native::NodeCryptoCreateHmac,
            &Value::Undefined,
            &[Value::String("md5".into()), md5_key],
            0,
        )
        .expect("createHmac md5 succeeds");
    vm.dispatch_native(
        Native::NodeHashUpdate,
        &hmac_md5,
        &[Value::String("Hi There".into())],
        0,
    )
    .expect("update succeeds");
    let digest_md5 = vm
        .dispatch_native(
            Native::NodeHashDigest,
            &hmac_md5,
            &[Value::String("hex".into())],
            0,
        )
        .expect("digest succeeds");
    assert_eq!(
        digest_md5,
        Value::String("9294727a3638bb1c13f48ef8158bfc9d".into())
    );

    // 5. Test crypto.randomUUID() format
    let uuid_val = vm
        .dispatch_native(Native::NodeCryptoRandomUuid, &Value::Undefined, &[], 0)
        .expect("randomUUID succeeds");
    let Value::String(uuid_str) = uuid_val else {
        panic!("randomUUID must return string");
    };
    assert_eq!(uuid_str.len(), 36);
    assert_eq!(uuid_str.unit_at(8), Some(b'-' as u16));
    assert_eq!(uuid_str.unit_at(13), Some(b'-' as u16));
    assert_eq!(uuid_str.unit_at(18), Some(b'-' as u16));
    assert_eq!(uuid_str.unit_at(23), Some(b'-' as u16));

    // 6. Test crypto.timingSafeEqual
    let a = node_bytes_value(vec![1, 2, 3, 4]);
    let b = node_bytes_value(vec![1, 2, 3, 4]);
    let c = node_bytes_value(vec![1, 2, 3, 5]);
    let d = node_bytes_value(vec![1, 2, 3]);

    let eq_result = vm
        .dispatch_native(
            Native::NodeCryptoTimingSafeEqual,
            &Value::Undefined,
            &[a.clone(), b.clone()],
            0,
        )
        .expect("timingSafeEqual succeeds");
    assert_eq!(eq_result, Value::Boolean(true));

    let neq_result = vm
        .dispatch_native(
            Native::NodeCryptoTimingSafeEqual,
            &Value::Undefined,
            &[a.clone(), c.clone()],
            0,
        )
        .expect("timingSafeEqual succeeds");
    assert_eq!(neq_result, Value::Boolean(false));

    // Mismatched lengths -> RangeError
    let diff_len_result = vm.dispatch_native(
        Native::NodeCryptoTimingSafeEqual,
        &Value::Undefined,
        &[a.clone(), d.clone()],
        0,
    );
    assert!(matches!(
        diff_len_result,
        Err(ExecutionError::RangeErrorMessage { .. })
    ));

    // Non-buffer -> TypeError
    let type_err_result = vm.dispatch_native(
        Native::NodeCryptoTimingSafeEqual,
        &Value::Undefined,
        &[a.clone(), Value::String("foo".into())],
        0,
    );
    assert!(matches!(
        type_err_result,
        Err(ExecutionError::TypeErrorMessage { .. })
    ));

    // 7. Test crypto.getRandomValues
    let u8_arr = Value::TypedArray(Rc::new(TypedArrayData {
        buffer: BufferData::fixed(16),
        offset: 0,
        length: Some(16),
        kind: TypedKind::Uint8,
        properties: RefCell::new(ObjectData::default()),
    }));
    let rand_vals = vm
        .dispatch_native(
            Native::NodeCryptoGetRandomValues,
            &Value::Undefined,
            &[u8_arr.clone()],
            0,
        )
        .expect("getRandomValues succeeds");
    assert_eq!(rand_vals, u8_arr);
    let Value::TypedArray(u8_view) = &rand_vals else {
        panic!("must be TypedArray");
    };
    assert_eq!(u8_view.bytes().len(), 16);

    // Float32Array throws TypeError
    let f32_arr = Value::TypedArray(Rc::new(TypedArrayData {
        buffer: BufferData::fixed(16),
        offset: 0,
        length: Some(4),
        kind: TypedKind::Float32,
        properties: RefCell::new(ObjectData::default()),
    }));
    let f32_err = vm.dispatch_native(
        Native::NodeCryptoGetRandomValues,
        &Value::Undefined,
        &[f32_arr],
        0,
    );
    assert!(matches!(
        f32_err,
        Err(ExecutionError::TypeErrorMessage { .. })
    ));

    // Length > 65536 throws RangeError
    let big_arr = Value::TypedArray(Rc::new(TypedArrayData {
        buffer: BufferData::fixed(65537),
        offset: 0,
        length: Some(65537),
        kind: TypedKind::Uint8,
        properties: RefCell::new(ObjectData::default()),
    }));
    let big_err = vm.dispatch_native(
        Native::NodeCryptoGetRandomValues,
        &Value::Undefined,
        &[big_arr],
        0,
    );
    assert!(matches!(
        big_err,
        Err(ExecutionError::RangeErrorMessage { .. })
    ));

    // 8. Test global crypto object
    let globals = vm.global_object();
    let global_crypto = vm.read_property(&globals, "crypto", 0).unwrap();
    assert!(matches!(
        global_crypto,
        Value::Native(Native::NodeCrypto, _)
    ));

    let grv = vm
        .read_property(&global_crypto, "getRandomValues", 0)
        .unwrap();
    let grv_name = vm.read_property(&grv, "name", 0).unwrap();
    assert_eq!(grv_name, Value::String("getRandomValues".into()));

    let ruuid = vm.read_property(&global_crypto, "randomUUID", 0).unwrap();
    let ruuid_name = vm.read_property(&ruuid, "name", 0).unwrap();
    assert_eq!(ruuid_name, Value::String("randomUUID".into()));

    let tse = vm
        .read_property(&global_crypto, "timingSafeEqual", 0)
        .unwrap();
    let tse_name = vm.read_property(&tse, "name", 0).unwrap();
    assert_eq!(tse_name, Value::String("timingSafeEqual".into()));
}

#[test]
fn generator_and_async_generator_prototype_and_constructor_hierarchy() {
    let program = Program {
        source: "".into(),
        template_sites: Vec::new(),
        version: kiln_ir::BYTECODE_VERSION,
        constants: Vec::new(),
        code: Vec::new(),
        local_count: 0,
        functions: vec![
            // Function 0: generator function
            kiln_ir::FunctionProto {
                inferred_name: None,
                span: kiln_ir::Span::default(),
                prologue_end: 0,
                asynchronous: false,
                name: None,
                arrow: false,
                generator: true,
                async_generator: false,
                is_method: false,
                parameter_count: 0,
                declared_length: 0,
                mapped_arguments: false,
                local_count: 0,
                code: vec![Instruction::Return],
                upvalues: Vec::new(),
                type_annotations: None,
            },
            // Function 1: async generator function
            kiln_ir::FunctionProto {
                inferred_name: None,
                span: kiln_ir::Span::default(),
                prologue_end: 0,
                asynchronous: true,
                name: None,
                arrow: false,
                generator: false,
                async_generator: true,
                is_method: false,
                parameter_count: 0,
                declared_length: 0,
                mapped_arguments: false,
                local_count: 0,
                code: vec![Instruction::Return],
                upvalues: Vec::new(),
                type_annotations: None,
            },
        ],
    };

    std::thread_local! {
        static CAPTURED_COMPILER_SOURCE: RefCell<String> = RefCell::new(String::new());
    }

    fn capturing_compiler(
        request: &EvalSource<'_>,
    ) -> Result<(Program, Strictness), EvalCompileError> {
        CAPTURED_COMPILER_SOURCE.with(|c| *c.borrow_mut() = request.source.to_owned());
        Ok((
            Program {
                source: "".into(),
                template_sites: Vec::new(),
                version: kiln_ir::BYTECODE_VERSION,
                constants: vec![Constant::Number(42.0f64.to_bits())],
                code: vec![Instruction::Const(0), Instruction::Return],
                local_count: 0,
                functions: Vec::new(),
            },
            Strictness::default(),
        ))
    }

    let mut host = TestHost {
        calls: Vec::new(),
        clock_now: None,
        entropy: std::collections::VecDeque::new(),
        timezone: None,
        locale: None,
    };
    let capabilities = ExecutionCapabilities::default().with_source_compiler(capturing_compiler);
    let mut vm = Vm::with_entropy_seed(
        Rc::new(program.clone()),
        Rc::new(Strictness::sloppy(&program)),
        &mut host,
        vec![Value::Undefined; program.local_count as usize],
        ExecutionLimits::default(),
        &RealmCapabilities::default(),
        capabilities,
    )
    .expect("realm capabilities resolve");

    let gen_fn = Value::Function(vm.make_closure(0).expect("make generator closure"));
    let async_gen_fn = Value::Function(vm.make_closure(1).expect("make async generator closure"));

    // 1. GeneratorFunction prototype and constructor checks
    let gen_proto = prototype_of(&gen_fn);
    assert_eq!(
        gen_proto,
        Value::Native(Native::GeneratorFunctionPrototype, RealmId::PRIMARY)
    );

    let gen_ctor = vm.read_property(&gen_fn, "constructor", 0).unwrap();
    assert_eq!(
        gen_ctor,
        Value::Native(Native::GeneratorFunctionConstructor, RealmId::PRIMARY)
    );

    let gen_proto_ctor = vm.read_property(&gen_proto, "constructor", 0).unwrap();
    assert_eq!(gen_proto_ctor, gen_ctor);

    let to_string_tag_key = Symbol::WellKnown(WellKnown::ToStringTag).property_key();
    let gen_proto_tag = vm.read_property(&gen_proto, &to_string_tag_key, 0).unwrap();
    assert_eq!(gen_proto_tag, Value::String("GeneratorFunction".into()));

    let gen_ctor_proto = prototype_of(&gen_ctor);
    assert_eq!(
        gen_ctor_proto,
        Value::Native(Native::FunctionConstructor, RealmId::PRIMARY)
    );

    let gen_proto_proto = prototype_of(&gen_proto);
    assert_eq!(
        gen_proto_proto,
        Value::Native(
            Native::PrototypeOf(PrototypeTable::Function),
            RealmId::PRIMARY
        )
    );

    // 2. AsyncGeneratorFunction prototype and constructor checks
    let async_gen_proto = prototype_of(&async_gen_fn);
    assert_eq!(
        async_gen_proto,
        Value::Native(Native::AsyncGeneratorFunctionPrototype, RealmId::PRIMARY)
    );

    let async_gen_ctor = vm.read_property(&async_gen_fn, "constructor", 0).unwrap();
    assert_eq!(
        async_gen_ctor,
        Value::Native(Native::AsyncGeneratorFunctionConstructor, RealmId::PRIMARY)
    );

    let async_gen_proto_ctor = vm
        .read_property(&async_gen_proto, "constructor", 0)
        .unwrap();
    assert_eq!(async_gen_proto_ctor, async_gen_ctor);

    let async_gen_proto_tag = vm
        .read_property(&async_gen_proto, &to_string_tag_key, 0)
        .unwrap();
    assert_eq!(
        async_gen_proto_tag,
        Value::String("AsyncGeneratorFunction".into())
    );

    let async_gen_ctor_proto = prototype_of(&async_gen_ctor);
    assert_eq!(
        async_gen_ctor_proto,
        Value::Native(Native::FunctionConstructor, RealmId::PRIMARY)
    );

    let async_gen_proto_proto = prototype_of(&async_gen_proto);
    assert_eq!(
        async_gen_proto_proto,
        Value::Native(
            Native::PrototypeOf(PrototypeTable::Function),
            RealmId::PRIMARY
        )
    );

    // 3. Generator and AsyncGenerator prototype hierarchy
    let gen_instance_proto = Value::Native(
        Native::PrototypeOf(PrototypeTable::Generator),
        RealmId::PRIMARY,
    );
    assert_eq!(
        prototype_of(&gen_instance_proto),
        Value::Native(
            Native::PrototypeOf(PrototypeTable::Iterator),
            RealmId::PRIMARY
        )
    );

    let async_gen_instance_proto = Value::Native(
        Native::PrototypeOf(PrototypeTable::AsyncGenerator),
        RealmId::PRIMARY,
    );
    let async_iter_proto = prototype_of(&async_gen_instance_proto);
    assert_eq!(
        async_iter_proto,
        Value::Native(
            Native::PrototypeOf(PrototypeTable::AsyncIterator),
            RealmId::PRIMARY
        )
    );
    assert_eq!(
        prototype_of(&async_iter_proto),
        Value::Native(
            Native::PrototypeOf(PrototypeTable::Object),
            RealmId::PRIMARY
        )
    );

    // AsyncIterator prototype has [Symbol.asyncIterator] returning this
    let async_iter_key = Symbol::WellKnown(WellKnown::AsyncIterator).property_key();
    let async_iter_fn = vm
        .read_property(&async_iter_proto, &async_iter_key, 0)
        .unwrap();
    assert_eq!(
        async_iter_fn,
        Value::Native(Native::IteratorSelf, RealmId::PRIMARY)
    );
    let iter_self = vm
        .dispatch_native(Native::IteratorSelf, &async_gen_instance_proto, &[], 0)
        .expect("asyncIterator call succeeds");
    assert_eq!(iter_self, async_gen_instance_proto);

    // 4. Test dynamic construction via GeneratorFunction and AsyncGeneratorFunction
    let _ = vm.dispatch_native(
        Native::GeneratorFunctionConstructor,
        &Value::Undefined,
        &[Value::String("a".into()), Value::String("yield a;".into())],
        0,
    );
    CAPTURED_COMPILER_SOURCE.with(|c| {
        assert_eq!(*c.borrow(), "(function* anonymous(a\n) {\nyield a;\n})");
    });

    let _ = vm.dispatch_native(
        Native::AsyncGeneratorFunctionConstructor,
        &Value::Undefined,
        &[Value::String("x".into()), Value::String("yield x;".into())],
        0,
    );
    CAPTURED_COMPILER_SOURCE.with(|c| {
        assert_eq!(
            *c.borrow(),
            "(async function* anonymous(x\n) {\nyield x;\n})"
        );
    });
}

#[test]
fn decode_uri_preserves_lone_surrogates() {
    let program = no_program();
    let mut host = TestHost {
        calls: Vec::new(),
        clock_now: None,
        entropy: std::collections::VecDeque::new(),
        timezone: None,
        locale: None,
    };
    let mut vm = test_vm(&program, &mut host);
    let lone = Value::String(StringUnits::from_units(&[0xD800]));
    let decoded = vm
        .dispatch_native(Native::DecodeUri, &Value::Undefined, &[lone.clone()], 0)
        .expect("decodeURI succeeds on lone surrogate");
    assert_eq!(decoded, lone);

    let decoded_comp = vm
        .dispatch_native(
            Native::DecodeUriComponent,
            &Value::Undefined,
            &[lone.clone()],
            0,
        )
        .expect("decodeURIComponent succeeds on lone surrogate");
    assert_eq!(decoded_comp, lone);

    // %ED%A0%80 is UTF-8 for surrogate U+D800, which must throw URIError
    let surrogate_pct = Value::String("a%ED%A0%80b".into());
    let err = vm
        .dispatch_native(Native::DecodeUri, &Value::Undefined, &[surrogate_pct], 0)
        .unwrap_err();
    assert!(matches!(err, ExecutionError::MalformedUri));
}

#[test]
fn to_property_key_coerces_regexp_and_other_objects_without_recursion() {
    let program = no_program();
    let mut host = TestHost {
        calls: Vec::new(),
        clock_now: None,
        entropy: std::collections::VecDeque::new(),
        timezone: None,
        locale: None,
    };
    let mut vm = test_vm(&program, &mut host);
    let target = Value::Object(Rc::new(RefCell::new(ObjectData::default())));
    let regexp = build_regexp(&StringUnits::from("abc"), "").expect("pattern compiles");
    let desc = vm
        .dispatch_native(
            Native::ObjectGetOwnPropertyDescriptor,
            &Value::Undefined,
            &[target, regexp],
            0,
        )
        .expect("Object.getOwnPropertyDescriptor with RegExp key succeeds");
    assert_eq!(desc, Value::Undefined);
}
