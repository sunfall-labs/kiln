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
        vm.dispatch_native(
            Native::Date(DateMethod::SetTime),
            &date,
            &[Value::Number(86_400_000.5)],
            0
        ),
        Ok(Value::Number(86_400_000.0))
    );
    assert_eq!(
        vm.dispatch_native(Native::Date(DateMethod::GetTime), &date, &[], 0),
        Ok(Value::Number(86_400_000.0))
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

/// An error with no message renders as a bare name, with no trailing colon.
#[test]
fn error_to_string_omits_an_empty_message() {
    let bare = object(&[
        ("name", Value::String("Error".to_owned().into())),
        ("message", Value::String(String::new().into())),
    ]);
    assert_eq!(error_to_string(&bare), "Error");
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
    // Negative zero itself stays negative zero.
    assert!(round(-0.0).is_sign_negative());
    assert!(round(0.0).is_sign_positive());
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
fn repeating_an_empty_string_with_a_huge_count_finishes_without_iteration() {
    let program = no_program();
    let mut host = TestHost {
        calls: Vec::new(),
        clock_now: None,
        entropy: Default::default(),
        timezone: None,
        locale: None,
    };
    let mut vm = test_vm(&program, &mut host);
    assert_eq!(
        vm.dispatch_native(
            Native::StringRepeat,
            &Value::String("".into()),
            &[Value::Number(1e15)],
            0
        ),
        Ok(Value::String("".into()))
    );
}

#[test]
fn numeric_unary_dispatch_preserves_ieee_edges_and_step_accounting() {
    for (operator, number, expected) in [
        (UnaryOp::Negate, 0.0, Value::Number(-0.0)),
        (UnaryOp::Plus, -0.0, Value::Number(-0.0)),
        (UnaryOp::ToNumeric, f64::NAN, Value::Number(f64::NAN)),
        (
            UnaryOp::Increment,
            f64::INFINITY,
            Value::Number(f64::INFINITY),
        ),
        (UnaryOp::Decrement, 0.0, Value::Number(-1.0)),
        (UnaryOp::BitwiseNot, 4294967296.0, Value::Number(-1.0)),
        (UnaryOp::Not, f64::NAN, Value::Boolean(true)),
        (UnaryOp::TypeOf, 3.0, Value::String("number".into())),
        (UnaryOp::Void, 3.0, Value::Undefined),
    ] {
        let program = program(
            vec![Constant::Number(number.to_bits())],
            vec![
                Instruction::Const(0),
                Instruction::Unary(operator),
                Instruction::Return,
            ],
        );
        let mut host = TestHost {
            calls: Vec::new(),
            clock_now: None,
            entropy: Default::default(),
            timezone: None,
            locale: None,
        };
        let mut vm = test_vm(&program, &mut host);
        let result = vm.run().unwrap();
        assert!(
            same_value(&result.value, &expected),
            "{operator:?}: {:?}",
            result.value
        );
        assert_eq!(result.steps, 3);
    }
}
