use super::*;

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

use kiln_host_abi::{HostError, HostOp};
use kiln_ir::Builtin;

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

/// An object with the given own properties, in order.
fn object(pairs: &[(&str, Value)]) -> Value {
    let mut data = ObjectData::default();
    for (key, value) in pairs {
        data.set(key, value.clone());
    }
    Value::Object(Rc::new(RefCell::new(data)))
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

fn parse_json(text: &str) -> Result<Value, ExecutionError> {
    let text = StringUnits::from(text);
    parse_json_text(&text)
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

mod language;
mod node;
mod objects;
mod runtime;
mod values;
