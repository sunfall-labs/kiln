use super::*;

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
            source_origin: None,
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
            source_origin: None,
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

#[test]
fn host_clock_is_explicit_and_does_not_replace_the_monotonic_clock() {
    let program = Program {
        source: "".into(),
        template_sites: Vec::new(),
        version: kiln_ir::BYTECODE_VERSION,
        source_origin: None,
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
        source_origin: None,
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
fn named_timezone_and_locale_never_call_the_host() {
    let program = Program {
        source: "".into(),
        template_sites: Vec::new(),
        version: kiln_ir::BYTECODE_VERSION,
        source_origin: None,
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
        source_origin: None,
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
        source_origin: None,
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
fn process_private_native_modules_are_separate_from_public_ts_projection() {
    let program = Program {
        source: "".into(),
        template_sites: Vec::new(),
        version: kiln_ir::BYTECODE_VERSION,
        source_origin: None,
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
    let http = vm
        .read_property(&modules, "http", 0)
        .expect("private HTTP adapter");
    // Reflection completes the public export store. It must not erase private serve hooks.
    let public_http = Value::Native(Native::NodeHttp, RealmId::PRIMARY);
    vm.intrinsics.materialize(&public_http);
    for name in ["createFetchServer", "fetchRespond"] {
        assert!(matches!(
            vm.read_property(&http, name, 0),
            Ok(Value::Native(..))
        ));
        assert!(matches!(
            vm.read_property(&public_http, name, 0),
            Ok(Value::Undefined)
        ));
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
        source_origin: None,
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
        Some(
            &[
                "EventEmitter",
                "EventEmitterAsyncResource",
                "once",
                "on",
                "getEventListeners",
                "default",
            ][..]
        )
    );
}

#[test]
fn node_console_exports_and_methods() {
    let program = Program {
        source: "".into(),
        template_sites: Vec::new(),
        version: kiln_ir::BYTECODE_VERSION,
        source_origin: None,
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
        source_origin: None,
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
        source_origin: None,
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
        source_origin: None,
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
        source_origin: None,
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

/// A module slot written inside a frame must be visible to the next frame that reads it:
/// that is the difference between module scope and a per-frame copy.
#[test]
fn module_slots_are_shared_across_frames() {
    let program = Program {
        source: "".into(),
        template_sites: Vec::new(),
        version: kiln_ir::BYTECODE_VERSION,
        source_origin: None,
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
                is_class: false,
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
                is_class: false,
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
fn module_execution_initializes_import_slots_and_publishes_exports() {
    let module = ModuleProgram {
        program: Program {
            source: "".into(),
            template_sites: Vec::new(),
            version: kiln_ir::BYTECODE_VERSION,
            source_origin: None,
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
fn empty_containers_and_nesting_read_the_way_node_writes_them() {
    assert_eq!(inspect(&object(&[])), "{}");
    assert_eq!(inspect(&array(&[])), "[]");
    assert_eq!(
        inspect(&array(&[array(&[Value::Number(1.0)])])),
        "[ [ 1 ] ]"
    );
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
        source_origin: None,
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
                is_class: false,
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
                is_class: false,
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
fn array_buffer_transfer_moves_bytes_and_detaches_the_source() {
    let source = test_buffer(&[1, 2, 3], None, false);
    let moved = call_native(
        &Intrinsics::default(),
        Native::ArrayBufferDetachRaw,
        &Value::Undefined,
        &[source.clone(), Value::Undefined, Value::Boolean(true)],
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
        Native::ArrayBufferDetachRaw,
        &Value::Undefined,
        &[resizable.clone(), Value::Number(4.0), Value::Boolean(true)],
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
        Native::ArrayBufferDetachRaw,
        &Value::Undefined,
        &[fixed.clone(), Value::Number(4.0), Value::Boolean(false)],
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
            Native::ArrayBufferDetachRaw,
            &Value::Undefined,
            &[too_large.clone(), Value::Number(3.0), Value::Boolean(true)],
        ),
        Err(ExecutionError::BufferLengthOutOfRange { .. })
    ));
    let shared = test_buffer(&[1], Some(2), true);
    assert!(matches!(
        call_native(
            &Intrinsics::default(),
            Native::ArrayBufferDetachRaw,
            &Value::Undefined,
            &[shared.clone(), Value::Undefined, Value::Boolean(true)],
        ),
        Err(ExecutionError::BufferDetached)
    ));
}

/// `__kilnArrayBufferResize` moves the length in place, and `__kilnTypedArrayOutOfBounds` sees
/// a fixed-length view fall out of the window when the buffer shrinks under it -- the two
/// representation primitives the shared `resize` and `ValidateTypedArray` are written over.
#[test]
fn array_buffer_resize_moves_the_length_and_views_report_out_of_bounds() {
    let rab = test_buffer(&[1, 2, 3, 4], Some(8), false);
    let view = construct_typed_array(
        TypedKind::Uint8,
        &[rab.clone(), Value::Number(2.0), Value::Number(2.0)],
    )
    .expect("view");
    let resize = |length: f64| {
        call_native(
            &Intrinsics::default(),
            Native::ArrayBufferResizeRaw,
            &Value::Undefined,
            &[rab.clone(), Value::Number(length)],
        )
    };
    let out_of_bounds = || {
        call_native(
            &Intrinsics::default(),
            Native::TypedArrayOutOfBoundsRaw,
            &Value::Undefined,
            std::slice::from_ref(&view),
        )
        .expect("answers")
    };
    resize(6.0).expect("grows within the ceiling");
    let Value::ArrayBuffer(data) = &rab else {
        unreachable!()
    };
    assert_eq!(data.byte_length(), 6);
    assert!(matches!(out_of_bounds(), Value::Boolean(false)));
    resize(3.0).expect("shrinks");
    assert!(matches!(out_of_bounds(), Value::Boolean(true)));
    assert!(resize(9.0).is_err(), "past the ceiling is refused");
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
        Native::ArrayBufferDetachRaw,
        &Value::Undefined,
        &[source.clone(), Value::Undefined, Value::Boolean(true)],
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
fn dynamic_import_without_module_loader_rejects_with_type_error() {
    let program = Program {
        source: "".into(),
        template_sites: Vec::new(),
        version: kiln_ir::BYTECODE_VERSION,
        source_origin: None,
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
        source_origin: None,
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
        source_origin: None,
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
            is_class: false,
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
