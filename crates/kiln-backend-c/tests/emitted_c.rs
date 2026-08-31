//! Snapshot coverage of the C99 the backend emits.
//!
//! `kiln-backend-c` is one of the two engines whose answers must agree, and the failure it is
//! built to avoid is a silent wrong one. A codegen edit that quietly changes the emitted
//! translation unit is otherwise invisible in review, because nothing in the tree records what
//! the unit used to look like. These snapshots record it.
//!
//! Two things are snapshotted, and they are snapshotted differently because they are written
//! differently:
//!
//! * the **program body** -- everything from the `/* Program hash: ... */` banner onwards. This
//!   is the half assembled instruction by instruction by `emit_instruction`, so nothing in the
//!   tree says what it is supposed to look like and a change to it is invisible in review. It is
//!   snapshotted verbatim.
//! * the **runtime prelude** -- the fixed C99 header every unit begins with. It is one 10,000
//!   line string literal in `src/lib.rs`, so an edit to it is already a diff a reviewer reads;
//!   copying it into this directory would add ten thousand lines that review nothing. What is
//!   *not* visible in that diff is the ABI it declares, so the snapshot here is the prelude's
//!   declaration surface -- its `#define`s, its tag values and every top-level signature -- which
//!   is the part a body, a linker and the host all have to agree about.
//!
//! Every body scenario also asserts the prelude it came with is byte-identical to every other
//! one's, so a prelude that started varying with the program would fail here rather than in a
//! cache somewhere.
//!
//! Snapshots are only a legitimate test if the emitter is deterministic, which ADR 0009 asserts.
//! `emission_is_deterministic_within_a_process` and `equal_programs_emit_equal_units` check that
//! claim directly rather than assuming it; running the suite twice checks it across processes,
//! where a hash seed differs.

use std::collections::BTreeMap;

use kiln_backend_c::{
    CModule, EmitError, emit, emit_c, emit_module, emit_module_with_logical_path,
};
use kiln_ir::{
    BYTECODE_VERSION, BinaryOp, Builtin, Constant, ExportBinding, FunctionProto, HostOp,
    ImportBinding, Instruction, ModuleProgram, Program, UnaryOp, Upvalue,
};

// -- construction helpers -----------------------------------------------------------------

fn number(value: f64) -> Constant {
    Constant::Number(value.to_bits())
}

fn program(constants: Vec<Constant>, code: Vec<Instruction>, local_count: u32) -> Program {
    Program {
        version: BYTECODE_VERSION,
        constants,
        code,
        local_count,
        functions: Vec::new(),
    }
}

fn function(
    name: &str,
    parameter_count: u32,
    local_count: u32,
    code: Vec<Instruction>,
    upvalues: Vec<Upvalue>,
) -> FunctionProto {
    FunctionProto {
        prologue_end: 0,
        asynchronous: false,
        name: Some(name.to_owned()),
        arrow: false,
        generator: false,
        async_generator: false,
        parameter_count,
        declared_length: parameter_count,
        local_count,
        code,
        upvalues,
    }
}

// -- prelude / body split -----------------------------------------------------------------

/// The banner that begins the program-dependent half of a unit.
///
/// Everything before it is the fixed prelude; everything from it onwards is what this particular
/// bytecode produced.
fn split(source: &str) -> (&str, &str) {
    let banner = source
        .find("/* Program hash: ")
        .or_else(|| source.find("/* Module hash: "))
        .expect("every emitted unit carries a hash banner between the prelude and the body");
    source.split_at(banner)
}

/// The program-dependent half, after asserting the prelude is the one already snapshotted.
///
/// Every scenario goes through here, so the prelude is checked as many times as there are
/// scenarios while being *recorded* once.
fn body(module: &CModule) -> &str {
    let (prelude, body) = split(&module.source);
    assert_eq!(
        prelude,
        split(
            &emit(&minimal_program())
                .expect("minimal program emits")
                .source
        )
        .0,
        "the runtime prelude must be identical in every emitted unit"
    );
    body
}

fn minimal_program() -> Program {
    program(
        vec![Constant::Undefined],
        vec![Instruction::Const(0), Instruction::Return],
        0,
    )
}

// -- the fixed prelude --------------------------------------------------------------------

/// The prelude's declaration surface: what a generated body, a linker and the host can name.
///
/// Function bodies are indented and everything declared at file scope is not, which is what this
/// leans on. Enum members are the exception -- they are indented and they are ABI, because a tag
/// value crosses the boundary -- so they are kept by name.
fn abi_surface(prelude: &str) -> String {
    let mut surface = String::new();
    for line in prelude.lines() {
        let trimmed = line.trim_end();
        let keep =
            if trimmed.starts_with("KILN_") || trimmed.trim_start().starts_with("KILN_VALUE_") {
                true
            } else {
                !trimmed.is_empty()
                    && !trimmed.starts_with(' ')
                    && !trimmed.starts_with('\t')
                    && !trimmed.starts_with("/*")
                    && !trimmed.starts_with(" *")
                    && !trimmed.starts_with('}')
            };
        if keep {
            surface.push_str(trimmed.trim_start());
            surface.push('\n');
        }
    }
    surface
}

#[derive(Debug, PartialEq, Eq)]
struct NumericIdDefine {
    name: String,
    value: u64,
}

/// Parse the numeric ID declarations from the emitted prelude rather than duplicating the
/// registry in this test. A malformed declaration must be an error: silently skipping one would
/// make the uniqueness checks appear to cover the whole prelude while leaving a collision free to
/// reach C compilation.
fn parse_numeric_id_defines(prelude: &str) -> Result<Vec<NumericIdDefine>, String> {
    const PREFIXES: [&str; 2] = ["#define KILN_C99_METHOD_", "#define KILN_C99_BUILTIN_"];
    let mut defines = Vec::new();
    for (line_index, line) in prelude.lines().enumerate() {
        let trimmed = line.trim_start();
        let Some(prefix) = PREFIXES.iter().find(|prefix| trimmed.starts_with(**prefix)) else {
            continue;
        };
        if trimmed != line {
            return Err(format!(
                "line {} indents numeric ID define `{line}`",
                line_index + 1
            ));
        }
        let rest = &line[prefix.len()..];
        let mut fields = rest.split_whitespace();
        let suffix = fields
            .next()
            .ok_or_else(|| format!("line {} has no numeric ID name", line_index + 1))?;
        if suffix.is_empty()
            || !suffix
                .bytes()
                .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
        {
            return Err(format!(
                "line {} has malformed numeric ID name `{suffix}`",
                line_index + 1
            ));
        }
        let value = fields
            .next()
            .ok_or_else(|| format!("line {} has no numeric ID value", line_index + 1))?
            .parse::<u64>()
            .map_err(|error| {
                format!(
                    "line {} has malformed numeric ID value: {error}",
                    line_index + 1
                )
            })?;
        if let Some(extra) = fields.next() {
            return Err(format!(
                "line {} has unexpected text after numeric ID: `{extra}`",
                line_index + 1
            ));
        }
        defines.push(NumericIdDefine {
            name: format!("{}{}", prefix.strip_prefix("#define ").unwrap(), suffix),
            value,
        });
    }
    Ok(defines)
}

fn assert_numeric_id_invariants(prelude: &str) {
    let defines = parse_numeric_id_defines(prelude).expect("numeric ID declarations parse");
    let mut by_name = BTreeMap::<String, u64>::new();
    let mut methods = BTreeMap::<u64, Vec<String>>::new();
    let mut builtins = BTreeMap::<u64, Vec<String>>::new();
    for define in defines {
        assert!(define.value > 0, "numeric ID `{}` is zero", define.name);
        assert!(
            by_name.insert(define.name.clone(), define.value).is_none(),
            "numeric ID `{}` is defined more than once",
            define.name
        );
        let ids = if define.name.starts_with("KILN_C99_METHOD_") {
            &mut methods
        } else {
            assert!(
                define.name.starts_with("KILN_C99_BUILTIN_"),
                "unexpected numeric ID family `{}`",
                define.name
            );
            &mut builtins
        };
        ids.entry(define.value).or_default().push(define.name);
    }

    // `TEMPORAL_LAST` is deliberately an alias for whatever the highest Temporal code currently
    // is, because the dispatcher routes Temporal by range rather than by equality. It moves every
    // time the Temporal surface grows -- it aliased `OP_LAST` until the two `Instant.fromEpoch*`
    // statics were wired -- so this names the pair rather than pinning one member.
    const TEMPORAL_ALIASES: [&str; 2] = [
        "KILN_C99_METHOD_TEMPORAL_INSTANT_FROM_EPOCH_NANOSECONDS",
        "KILN_C99_METHOD_TEMPORAL_LAST",
    ];
    for (value, names) in &methods {
        if names.len() > 1 {
            let mut sorted = names.clone();
            sorted.sort();
            assert_eq!(
                sorted,
                TEMPORAL_ALIASES,
                "duplicate method ID {value} is claimed by {}",
                names.join(", ")
            );
            // A tripwire against accidental renumbering, not a fixed fact: this rises whenever
            // the Temporal surface grows, and the two `Instant.fromEpoch*` statics took it from
            // 335 to 337. Update it deliberately, with the Temporal range still contiguous.
            assert_eq!(*value, 337, "Temporal method aliases moved from ID 337");
        }
    }

    let builtin_values: Vec<_> = builtins.keys().copied().collect();
    let builtin_max = *builtin_values
        .last()
        .expect("prelude declares at least one builtin ID");
    let missing_builtins: Vec<_> = (1..=builtin_max)
        .filter(|value| !builtins.contains_key(value))
        .collect();
    assert!(
        missing_builtins.is_empty(),
        "builtin IDs missing {missing_builtins:?}; declared IDs are {builtin_values:?}"
    );

    // 336 is the first ID appended after the historical receiver/range layout. New runtime
    // facade methods must append at max+1, never fill an old gap or reuse an old value.
    const APPEND_ONLY_METHOD_BASE: u64 = 336;
    let custom_values: Vec<_> = methods
        .keys()
        .copied()
        .filter(|value| *value >= APPEND_ONLY_METHOD_BASE)
        .collect();
    let custom_max = *custom_values
        .last()
        .expect("prelude declares the append-only method tail");
    let missing_custom: Vec<_> = (APPEND_ONLY_METHOD_BASE..=custom_max)
        .filter(|value| !methods.contains_key(value))
        .collect();
    assert!(
        missing_custom.is_empty(),
        "custom method IDs missing {missing_custom:?}; declared IDs are {custom_values:?}"
    );
}

#[test]
fn runtime_prelude_abi_surface() {
    let unit = emit(&minimal_program()).expect("minimal program emits");
    let (prelude, _) = split(&unit.source);
    assert_numeric_id_invariants(prelude);
    insta::assert_snapshot!(abi_surface(prelude));
}

#[test]
fn numeric_id_parser_rejects_malformed_definitions() {
    for source in [
        "#define KILN_C99_METHOD_BROKEN\n",
        "#define KILN_C99_METHOD_BROKEN nope\n",
        "#define KILN_C99_METHOD_BROKEN 1 trailing\n",
        " #define KILN_C99_METHOD_BROKEN 1\n",
        "#define KILN_C99_METHOD_broken 1\n",
    ] {
        assert!(
            parse_numeric_id_defines(source).is_err(),
            "malformed definition was accepted: {source:?}"
        );
    }
}

#[test]
fn runtime_prelude_size() {
    // A guard on the half that is not snapshotted verbatim: the surface above would not notice a
    // rewritten function body, and this notices that something moved without pretending to say
    // what. Update it deliberately.
    let unit = emit(&minimal_program()).expect("minimal program emits");
    let (prelude, _) = split(&unit.source);
    insta::assert_snapshot!(format!(
        "prelude lines: {}\nprelude bytes: {}",
        prelude.lines().count(),
        prelude.len()
    ));
}

#[test]
fn minimal_unit_body() {
    let unit = emit(&minimal_program()).expect("minimal program emits");
    insta::assert_snapshot!(body(&unit));
}

// -- scalar arithmetic --------------------------------------------------------------------

fn arithmetic_program() -> Program {
    // Numbers are emitted as raw IEEE-754 bits, so the awkward ones are here on purpose: a
    // change that started formatting them as C literals instead would lose -0.0 and NaN.
    program(
        vec![
            number(2.0),
            number(3.5),
            number(-0.0),
            number(f64::NAN),
            number(1e21),
        ],
        vec![
            Instruction::Const(0),
            Instruction::Const(1),
            Instruction::Binary(BinaryOp::Add),
            Instruction::Const(4),
            Instruction::Binary(BinaryOp::Multiply),
            Instruction::StoreLocal(0),
            Instruction::LoadLocal(0),
            Instruction::Unary(UnaryOp::Negate),
            Instruction::Const(2),
            Instruction::Binary(BinaryOp::Subtract),
            Instruction::Const(3),
            Instruction::Binary(BinaryOp::StrictEqual),
            Instruction::Unary(UnaryOp::Not),
            Instruction::StoreLocal(1),
            Instruction::LoadLocal(0),
            Instruction::Const(0),
            Instruction::Binary(BinaryOp::Remainder),
            Instruction::Const(0),
            Instruction::Binary(BinaryOp::Exponentiate),
            Instruction::Const(0),
            Instruction::Binary(BinaryOp::BitwiseXor),
            Instruction::Const(0),
            Instruction::Binary(BinaryOp::UnsignedShiftRight),
            Instruction::Unary(UnaryOp::TypeOf),
            Instruction::Return,
        ],
        2,
    )
}

#[test]
fn scalar_arithmetic() {
    let unit = emit(&arithmetic_program()).expect("arithmetic emits");
    insta::assert_snapshot!(body(&unit));
}

// -- control flow -------------------------------------------------------------------------

#[test]
fn if_else() {
    // if (true) { x = 1 } else { x = 2 }; x
    let unit = emit(&program(
        vec![number(1.0), number(2.0), Constant::Boolean(true)],
        vec![
            Instruction::Const(2),
            Instruction::JumpIfFalse(4),
            Instruction::Const(0),
            Instruction::Jump(5),
            Instruction::Const(1),
            Instruction::StoreLocal(0),
            Instruction::LoadLocal(0),
            Instruction::Return,
        ],
        1,
    ))
    .expect("if/else emits");
    insta::assert_snapshot!(body(&unit));
}

#[test]
fn while_loop() {
    // let i = 0; while (i < 3) { i = i + 1 } ; i
    //
    // The back edge is the point: a label the emitter jumps *backwards* to is where a C
    // compiler's notion of a loop comes from, and there is no other instruction that makes one.
    let unit = emit(&program(
        vec![number(0.0), number(3.0), number(1.0)],
        vec![
            Instruction::Const(0),
            Instruction::StoreLocal(0),
            Instruction::LoadLocal(0),
            Instruction::Const(1),
            Instruction::Binary(BinaryOp::LessThan),
            Instruction::JumpIfFalse(11),
            Instruction::LoadLocal(0),
            Instruction::Const(2),
            Instruction::Binary(BinaryOp::Add),
            Instruction::StoreLocal(0),
            Instruction::Jump(2),
            Instruction::LoadLocal(0),
            Instruction::Return,
        ],
        1,
    ))
    .expect("while loop emits");
    insta::assert_snapshot!(body(&unit));
}

#[test]
fn short_circuit_jumps() {
    // The three conditional jumps that are not `JumpIfFalse`, which nothing else reaches.
    let unit = emit(&program(
        vec![Constant::Null, number(1.0)],
        vec![
            Instruction::Const(0),
            Instruction::Dup,
            Instruction::JumpIfTruthy(5),
            Instruction::Dup,
            Instruction::JumpIfNotNullish(5),
            Instruction::Pop,
            Instruction::Const(1),
            Instruction::Return,
        ],
        0,
    ))
    .expect("short-circuit jumps emit");
    insta::assert_snapshot!(body(&unit));
}

// -- calls --------------------------------------------------------------------------------

fn call_program() -> Program {
    // let base = 10; const add = (n) => base + n; add(5)
    //
    // `base` is captured, so it is a cell rather than a slot, and the closure reaches it through
    // an upvalue. That is three emission paths -- NewCell, StoreCell, LoadUpvalue -- that a call
    // without a capture would not touch.
    let mut top = program(
        vec![number(10.0), number(5.0)],
        vec![
            Instruction::NewCell(0),
            Instruction::Const(0),
            Instruction::StoreCell(0),
            Instruction::MakeFunction(0),
            Instruction::StoreLocal(1),
            Instruction::LoadLocal(1),
            Instruction::Const(1),
            Instruction::Call { argc: 1 },
            Instruction::Return,
        ],
        2,
    );
    top.functions = vec![function(
        "add",
        1,
        2,
        vec![
            Instruction::LoadUpvalue(0),
            Instruction::LoadLocal(0),
            Instruction::Binary(BinaryOp::Add),
            Instruction::StoreLocal(1),
            Instruction::LoadLocal(1),
            Instruction::Return,
        ],
        vec![Upvalue::ParentLocal(0)],
    )];
    top
}

#[test]
fn function_call_with_closure() {
    let unit = emit(&call_program()).expect("call emits");
    insta::assert_snapshot!(body(&unit));
}

#[test]
fn typed_array_borrowed_generic_methods() {
    let callback = FunctionProto {
        code: vec![
            Instruction::LoadArguments,
            Instruction::Const(6),
            Instruction::GetComputed,
            Instruction::Return,
        ],
        ..function("receiver", 0, 0, Vec::new(), Vec::new())
    };
    let unit = emit(&Program {
        version: BYTECODE_VERSION,
        constants: vec![
            number(1.0),
            number(2.0),
            Constant::String("prototype".into()),
            Constant::String("map".into()),
            Constant::String("call".into()),
            Constant::String("constructor".into()),
            Constant::String("2".into()),
        ],
        code: vec![
            Instruction::LoadBuiltin(Builtin::Array),
            Instruction::GetProperty(2),
            Instruction::GetProperty(3),
            Instruction::LoadBuiltin(Builtin::Uint8Array),
            Instruction::Const(0),
            Instruction::Const(1),
            Instruction::NewArray { count: 2 },
            Instruction::New { argc: 1 },
            Instruction::MakeFunction(0),
            Instruction::CallMethod { name: 4, argc: 2 },
            Instruction::GetProperty(5),
            Instruction::Return,
        ],
        local_count: 0,
        functions: vec![callback],
    })
    .expect("borrowed typed-array method emits");
    insta::assert_snapshot!(body(&unit));
}

// -- strings ------------------------------------------------------------------------------

#[test]
fn string_operations() {
    // Concatenation, a property read, and two method calls. The first constant is deliberately
    // hostile: `c_string` has to escape the quotes, the newline and the tab, and split the
    // non-ASCII bytes into hex escapes that cannot swallow a following digit.
    let unit = emit(&program(
        vec![
            Constant::String("h\u{e9}llo \"world\"\n\t7".to_owned()),
            Constant::String("length".to_owned()),
            Constant::String("toUpperCase".to_owned()),
            Constant::String(" & more".to_owned()),
            Constant::String("concat".to_owned()),
        ],
        vec![
            Instruction::Const(0),
            Instruction::Const(3),
            Instruction::Binary(BinaryOp::Add),
            Instruction::StoreLocal(0),
            Instruction::LoadLocal(0),
            Instruction::GetProperty(1),
            Instruction::Pop,
            Instruction::LoadLocal(0),
            Instruction::CallMethod { name: 2, argc: 0 },
            Instruction::Const(3),
            Instruction::CallMethod { name: 4, argc: 1 },
            Instruction::Return,
        ],
        1,
    ))
    .expect("string operations emit");
    insta::assert_snapshot!(body(&unit));
}

// -- the host boundary --------------------------------------------------------------------

#[test]
fn host_call_and_builtin() {
    // String(42) + "hello", handed to the one host operation that exists.
    let unit = emit(&program(
        vec![Constant::String("hello".to_owned()), number(42.0)],
        vec![
            Instruction::LoadBuiltin(Builtin::String),
            Instruction::Const(1),
            Instruction::Call { argc: 1 },
            Instruction::Const(0),
            Instruction::Binary(BinaryOp::Add),
            Instruction::HostCall {
                op: HostOp::WriteStdout,
                argc: 1,
            },
            Instruction::Return,
        ],
        0,
    ))
    .expect("host call emits");
    insta::assert_snapshot!(body(&unit));
}

// -- aggregates ---------------------------------------------------------------------------

#[test]
fn objects_and_arrays() {
    let unit = emit(&program(
        vec![
            Constant::String("a".to_owned()),
            number(1.0),
            Constant::String("b".to_owned()),
            number(2.0),
        ],
        vec![
            Instruction::Const(0),
            Instruction::Const(1),
            Instruction::Const(2),
            Instruction::Const(3),
            Instruction::NewObject { count: 2 },
            Instruction::StoreLocal(0),
            Instruction::Const(1),
            Instruction::Const(3),
            Instruction::NewArray { count: 2 },
            Instruction::StoreLocal(1),
            Instruction::LoadLocal(0),
            Instruction::GetProperty(0),
            Instruction::Pop,
            Instruction::LoadLocal(1),
            Instruction::Const(1),
            Instruction::GetComputed,
            Instruction::Pop,
            Instruction::LoadLocal(1),
            Instruction::Const(1),
            Instruction::Const(3),
            Instruction::SetComputed,
            Instruction::Pop,
            Instruction::LoadLocal(0),
            Instruction::Const(3),
            Instruction::SetProperty(2),
            Instruction::Return,
        ],
        2,
    ))
    .expect("objects and arrays emit");
    insta::assert_snapshot!(body(&unit));
}

// -- exceptions ---------------------------------------------------------------------------

#[test]
fn try_catch_throw() {
    // try { if (flag) throw "boom"; x = 7 } catch (e) { x = e } ; x
    //
    // Reaches the handler prologue, the unwind dispatch and `kiln_uncaught`, none of which a
    // body without a `PushHandler` emits at all.
    let unit = emit(&program(
        vec![
            Constant::Boolean(true),
            Constant::String("boom".to_owned()),
            number(7.0),
        ],
        vec![
            Instruction::PushHandler(9),
            Instruction::Const(0),
            Instruction::JumpIfFalse(5),
            Instruction::Const(1),
            Instruction::Throw,
            Instruction::Const(2),
            Instruction::StoreLocal(0),
            Instruction::PopHandler,
            Instruction::Jump(10),
            Instruction::StoreLocal(0),
            Instruction::LoadLocal(0),
            Instruction::Return,
        ],
        1,
    ))
    .expect("try/catch emits");
    insta::assert_snapshot!(body(&unit));
}

// -- module linkage -----------------------------------------------------------------------

fn linked_module() -> ModuleProgram {
    // import { add } from "./math.js"; const total = add(1, 1); export { total }
    //
    // The import arrives in a slot the body reads as a cell, which is the one branch that emits
    // the "box it if the linker handed over a plain value" guard.
    ModuleProgram {
        program: program(
            vec![number(1.0)],
            vec![
                Instruction::LoadCell(0),
                Instruction::Const(0),
                Instruction::Const(0),
                Instruction::Call { argc: 2 },
                Instruction::StoreLocal(2),
                Instruction::LoadLocal(2),
                Instruction::Return,
            ],
            3,
        ),
        dependencies: vec!["./math.js".to_owned()],
        imports: vec![ImportBinding {
            source: "./math.js".to_owned(),
            imported: "add".to_owned(),
            local: "add".to_owned(),
            local_slot: 0,
        }],
        exports: vec![ExportBinding {
            exported: "total".to_owned(),
            local_slot: 2,
        }],
    }
}

#[test]
fn module_with_imports_and_exports() {
    let unit = emit_module(&linked_module()).expect("module emits");
    insta::assert_snapshot!(body(&unit));
}

#[test]
fn module_symbol_under_a_logical_path() {
    let module = linked_module();
    let by_content = emit_module(&module).expect("module emits");
    let by_path =
        emit_module_with_logical_path(&module, "src/app/main.js").expect("logical path emits");

    assert_eq!(
        by_path.program_hash, by_content.program_hash,
        "the logical path names the unit; it does not change what the bytecode hashes to"
    );
    assert_eq!(
        by_path.source,
        by_content
            .source
            .replace(&by_content.symbol, &by_path.symbol),
        "the two spellings must differ only in the exported symbol"
    );

    insta::assert_snapshot!(format!(
        "content-addressed: {}\nlogical path:      {}\nprogram hash:      {:016x}",
        by_content.symbol, by_path.symbol, by_path.program_hash
    ));
}

// -- module scope -------------------------------------------------------------------------

#[test]
fn module_scope_slots() {
    // `LoadGlobal`/`StoreGlobal` move the frame's locals out of the C frame and into a static
    // array, because an exported closure reads them after the entry point has returned.
    let mut top = program(
        vec![number(3.0)],
        vec![
            Instruction::Const(0),
            Instruction::StoreGlobal(0),
            Instruction::MakeFunction(0),
            Instruction::StoreLocal(1),
            Instruction::LoadLocal(1),
            Instruction::Return,
        ],
        2,
    );
    top.functions = vec![function(
        "read",
        0,
        0,
        vec![Instruction::LoadGlobal(0), Instruction::Return],
        Vec::new(),
    )];
    let unit = emit(&top).expect("module scope emits");
    insta::assert_snapshot!(body(&unit));
}

// -- refusals -----------------------------------------------------------------------------

#[test]
fn rejected_programs() {
    // The refusals are as much a part of the contract as the C: a backend that silently accepted
    // any of these would emit a translation unit whose indexing is undefined.
    let mut report = String::new();
    let mut record = |label: &str, error: EmitError| {
        report.push_str(label);
        report.push_str("\n  ");
        report.push_str(&error.to_string());
        report.push('\n');
    };

    let bad = |constants: Vec<Constant>, code: Vec<Instruction>, local_count: u32| {
        emit(&program(constants, code, local_count)).expect_err("must be rejected")
    };

    record("empty program", bad(Vec::new(), Vec::new(), 0));
    record(
        "unsupported bytecode version",
        emit(&Program {
            version: BYTECODE_VERSION + 1,
            ..minimal_program()
        })
        .expect_err("must be rejected"),
    );
    record(
        "constant index out of bounds",
        bad(
            Vec::new(),
            vec![Instruction::Const(0), Instruction::Return],
            0,
        ),
    );
    record(
        "local slot out of bounds",
        bad(
            Vec::new(),
            vec![Instruction::LoadLocal(4), Instruction::Return],
            1,
        ),
    );
    record(
        "jump target out of bounds",
        bad(
            Vec::new(),
            vec![Instruction::Jump(9), Instruction::Return],
            0,
        ),
    );
    record(
        "property name is not a string",
        bad(
            vec![number(1.0)],
            vec![
                Instruction::Const(0),
                Instruction::GetProperty(0),
                Instruction::Return,
            ],
            0,
        ),
    );
    record(
        "stack underflow",
        bad(
            Vec::new(),
            vec![Instruction::Binary(BinaryOp::Add), Instruction::Return],
            0,
        ),
    );
    record(
        "inconsistent stack height at a merge",
        bad(
            vec![number(1.0), Constant::Boolean(true)],
            vec![
                Instruction::Const(1),
                Instruction::JumpIfFalse(3),
                Instruction::Const(0),
                Instruction::Const(0),
                Instruction::Return,
            ],
            0,
        ),
    );
    record(
        "control falls off the end",
        bad(vec![number(1.0)], vec![Instruction::Const(0)], 0),
    );
    record(
        "builtin the backend does not implement",
        bad(
            Vec::new(),
            vec![
                Instruction::LoadBuiltin(Builtin::WeakMap),
                Instruction::Return,
            ],
            0,
        ),
    );
    record("function captures a slot its creator lacks", {
        let mut top = program(
            Vec::new(),
            vec![Instruction::MakeFunction(0), Instruction::Return],
            0,
        );
        top.functions = vec![function(
            "orphan",
            0,
            0,
            vec![Instruction::LoadUpvalue(0), Instruction::Return],
            vec![Upvalue::ParentLocal(7)],
        )];
        emit(&top).expect_err("must be rejected")
    });
    record("function with more parameters than slots", {
        let mut top = program(
            Vec::new(),
            vec![Instruction::MakeFunction(0), Instruction::Return],
            0,
        );
        top.functions = vec![function(
            "wide",
            3,
            1,
            vec![Instruction::Return],
            Vec::new(),
        )];
        emit(&top).expect_err("must be rejected")
    });

    for path in [
        "",
        "/abs/main.js",
        "C:/checkout/main.js",
        "src\\app\\main.js",
        "src/../main.js",
        "src//main.js",
    ] {
        record(
            &format!("logical path {path:?}"),
            emit_module_with_logical_path(&linked_module(), path).expect_err("must be rejected"),
        );
    }

    insta::assert_snapshot!(report);
}

// -- determinism, which is what makes all of the above a valid test ------------------------

#[test]
fn emission_is_deterministic_within_a_process() {
    let cases: Vec<CModule> = vec![
        emit(&minimal_program()).unwrap(),
        emit(&arithmetic_program()).unwrap(),
        emit(&call_program()).unwrap(),
        emit_module(&linked_module()).unwrap(),
    ];
    for _ in 0..16 {
        assert_eq!(emit(&minimal_program()).unwrap(), cases[0]);
        assert_eq!(emit(&arithmetic_program()).unwrap(), cases[1]);
        assert_eq!(emit(&call_program()).unwrap(), cases[2]);
        assert_eq!(emit_module(&linked_module()).unwrap(), cases[3]);
    }
}

#[test]
fn equal_programs_emit_equal_units() {
    // Built twice from scratch, so nothing is shared between the two: any address, allocation
    // order or hash seed reaching the output would separate them.
    let left = emit(&call_program()).unwrap();
    let right = emit_c(&call_program()).unwrap();
    assert_eq!(left.symbol, right.symbol);
    assert_eq!(left.program_hash, right.program_hash);
    assert_eq!(left.source, right.source);
}

#[test]
fn emitted_units_carry_no_ambient_state() {
    // A checkout path, a source location or a process id in the output would make the unit
    // uncacheable and every snapshot above meaningless the moment it moved machines.
    for unit in [
        emit(&call_program()).unwrap(),
        emit(&arithmetic_program()).unwrap(),
        emit_module(&linked_module()).unwrap(),
    ] {
        for forbidden in [
            "/Users/",
            "/home/",
            "/private/",
            "/var/folders/",
            ".rs:",
            "\\Users\\",
        ] {
            assert!(
                !unit.source.contains(forbidden),
                "emitted C contains ambient state: {forbidden}"
            );
        }
        assert!(!unit.symbol.is_empty());
    }
}
