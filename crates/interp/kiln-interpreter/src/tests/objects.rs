use super::*;

#[test]
fn numeric_array_writes_preserve_property_key_boundaries_and_refusals() {
    let program = no_program();
    let mut host = TestHost {
        calls: Vec::new(),
        clock_now: None,
        entropy: Default::default(),
        timezone: None,
        locale: None,
    };
    let mut vm = test_vm(&program, &mut host);
    for strict in [false, true] {
        let items = ArrayData::new(Vec::new());
        let array = Value::Array(Rc::clone(&items));
        for (number, name) in [
            (-0.0, "0"),
            (-1.0, "-1"),
            (1.5, "1.5"),
            (f64::from(u32::MAX), "4294967295"),
            (f64::INFINITY, "Infinity"),
            (f64::NAN, "NaN"),
        ] {
            vm.assign_with_strict(
                &array,
                &Value::Number(number),
                Value::Boolean(true),
                0,
                strict,
            )
            .unwrap();
            assert_eq!(
                vm.read_computed(&array, &Value::String(name.into()), 0)
                    .unwrap(),
                Value::Boolean(true)
            );
        }
        assert_eq!(items.borrow().len(), 1, "only numeric -0 is an element");
        vm.assign_with_strict(
            &array,
            &Value::String("-0".into()),
            Value::Boolean(false),
            0,
            strict,
        )
        .unwrap();
        assert_eq!(items.borrow()[0], Value::Boolean(true));
        items.length_writable.set(false);
        let result =
            vm.assign_with_strict(&array, &Value::Number(1.0), Value::Boolean(true), 0, strict);
        let string_result = vm.assign_with_strict(
            &array,
            &Value::String("1".into()),
            Value::Boolean(true),
            0,
            strict,
        );
        assert_eq!(
            result, string_result,
            "numeric and string keys share length refusal"
        );
        assert_eq!(items.borrow().len(), 1);
        items.properties.borrow_mut().prevent_extensions();
        assert_eq!(
            vm.assign_with_strict(&array, &Value::Number(1.0), Value::Boolean(true), 0, strict)
                .is_err(),
            strict,
        );
        items.properties.borrow_mut().freeze();
        assert_eq!(
            vm.assign_with_strict(
                &array,
                &Value::Number(0.0),
                Value::Boolean(false),
                0,
                strict
            )
            .is_err(),
            strict,
        );
        assert_eq!(items.borrow()[0], Value::Boolean(true));
    }
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
fn regexp_escape_appears_in_constructor_own_property_names() {
    let intrinsics = Intrinsics::default();
    let constructor = Value::Native(Native::RegExpConstructor, RealmId::PRIMARY);
    assert!(own_property_keys(&intrinsics, &constructor).contains(&"escape".to_owned()));
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
fn a_parsed_object_keeps_the_last_value_for_a_duplicate_key() {
    let Ok(Value::Object(object)) = parse_json(r#"{"a":1,"a":2}"#) else {
        panic!("expected an object");
    };
    assert_eq!(object.borrow().keys().count(), 1);
    assert!(matches!(object.borrow().get_own("a"), Some(Value::Number(n)) if *n == 2.0));
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
fn a_symbol_shows_its_description_rather_than_its_reserved_key() {
    // The key is `@@iterator`, which is an implementation detail and not what a console says.
    let symbol = Value::Symbol(Symbol::WellKnown(WellKnown::Iterator));
    assert_eq!(inspect(&symbol), "Symbol(Symbol.iterator)");
    assert_eq!(symbol.property_key_lossy(), "@@iterator");
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
