//! `parallelMap` against `map`, natively (issue 02), and the site reasons `--explain-parallel`
//! prints that are decided by the plan and the call site rather than by the source rules
//! (issue 03; the source rules' reasons are pinned in `parallel.rs`).
//!
//! The property test generates callbacks from the qualifying grammar -- the element, the index,
//! captured bindings written once (tier 1), numeric literals, arithmetic and the listed `Math`
//! functions -- runs each through `parallelMap` and through `map` over inputs that include
//! `NaN`, `-0` and the infinities, and requires the two results to be the same bits. The pool
//! is a test fixture that splits every call across four POSIX threads regardless of size, so
//! the chunk workers really run concurrently here, and the fixture counts its calls so the test
//! also proves each site took the parallel lowering rather than quietly falling back.

use crate::parallel::{Reason, SiteRecord};
use crate::{BYTECODE_VERSION, CModule, EmissionPolicy, emit_with_policy};
use kiln_ir::{BinaryOp, Builtin, Constant, FunctionProto, Instruction, Program, UnaryOp, Upvalue};
use proptest::prelude::*;
use std::io::Write as _;
use std::process::{Command, Stdio};

fn proto(code: Vec<Instruction>, parameter_count: u32, upvalues: Vec<Upvalue>) -> FunctionProto {
    FunctionProto {
        inferred_name: None,
        span: kiln_ir::Span::default(),
        prologue_end: 0,
        declared_length: parameter_count,
        name: None,
        parameter_count,
        local_count: parameter_count,
        code,
        arrow: true,
        generator: false,
        asynchronous: false,
        async_generator: false,
        is_method: false,
        is_class: false,
        mapped_arguments: false,
        upvalues,
        type_annotations: None,
    }
}

/// A constant table that hands out indices, deduplicating numbers by their bits.
#[derive(Default)]
struct Constants(Vec<Constant>);

impl Constants {
    fn number(&mut self, value: f64) -> u32 {
        let bits = value.to_bits();
        if let Some(at) = self
            .0
            .iter()
            .position(|c| matches!(c, Constant::Number(b) if *b == bits))
        {
            return at as u32;
        }
        self.0.push(Constant::Number(bits));
        (self.0.len() - 1) as u32
    }

    fn string(&mut self, value: &str) -> u32 {
        if let Some(at) = self
            .0
            .iter()
            .position(|c| matches!(c, Constant::String(s) if s == value))
        {
            return at as u32;
        }
        self.0.push(Constant::String(value.into()));
        (self.0.len() - 1) as u32
    }
}

fn sites(program: &Program, policy: EmissionPolicy) -> Vec<SiteRecord> {
    let generated = emit_with_policy(program, policy).expect("program emits");
    SiteRecord::parse_all(&generated.source)
}

/// `[].parallelMap(<callback>, ...extra)` at the top level, returned.
fn one_site(callback: Option<FunctionProto>, extra: usize, constants: Constants) -> Program {
    let mut constants = constants;
    let name = constants.string("parallelMap");
    let one = constants.number(1.0);
    let mut code = vec![Instruction::NewArray { count: 0 }];
    match &callback {
        Some(_) => code.push(Instruction::MakeFunction(0)),
        None => code.push(Instruction::Const(one)),
    }
    for _ in 0..extra {
        code.push(Instruction::Const(one));
    }
    code.push(Instruction::CallMethod {
        name,
        argc: 1 + extra as u16,
    });
    code.push(Instruction::Return);
    Program {
        source: "".into(),
        version: BYTECODE_VERSION,
        source_origin: None,
        static_source_identity: None,
        static_source_provenance: None,
        constants: constants.0,
        code,
        local_count: 0,
        functions: callback.into_iter().collect(),
        template_sites: Vec::new(),
        debug_call_sites: Vec::new(),
    }
}

/// `x => x * 2 + 1`.
fn arithmetic(constants: &mut Constants) -> FunctionProto {
    let two = constants.number(2.0);
    let one = constants.number(1.0);
    proto(
        vec![
            Instruction::LoadLocal(0),
            Instruction::Const(two),
            Instruction::Binary(BinaryOp::Multiply),
            Instruction::Const(one),
            Instruction::Binary(BinaryOp::Add),
            Instruction::Return,
        ],
        1,
        Vec::new(),
    )
}

fn only_reason(program: &Program, policy: EmissionPolicy) -> Option<Reason> {
    let records = sites(program, policy);
    assert_eq!(records.len(), 1, "one site: {records:?}");
    records[0].refusal.as_ref().map(|r| r.reason)
}

#[test]
fn a_qualifying_site_is_recorded_parallel() {
    let mut constants = Constants::default();
    let callback = arithmetic(&mut constants);
    let program = one_site(Some(callback), 0, constants);
    assert_eq!(only_reason(&program, EmissionPolicy::default()), None);
}

#[test]
fn reason_dev_profile() {
    let mut constants = Constants::default();
    let callback = arithmetic(&mut constants);
    let program = one_site(Some(callback), 0, constants);
    let policy = EmissionPolicy {
        generic_only: true,
        ..Default::default()
    };
    assert_eq!(only_reason(&program, policy), Some(Reason::DevProfile));
}

/// Issue 13: a unit emitted generic-only (a release module past the specialization limit)
/// still lowers a qualifying site, and still names the real reason for one that is not.
#[test]
fn a_generic_only_unit_still_lowers_its_qualifying_sites() {
    let policy = EmissionPolicy {
        generic_only: true,
        parallel_in_generic: true,
        ..Default::default()
    };
    let mut constants = Constants::default();
    let callback = arithmetic(&mut constants);
    let program = one_site(Some(callback), 0, constants);
    assert_eq!(only_reason(&program, policy.clone()), None);
    let mut constants = Constants::default();
    let callback = arithmetic(&mut constants);
    let program = one_site(Some(callback), 1, constants);
    assert_eq!(
        only_reason(&program, policy.clone()),
        Some(Reason::ExtraArguments)
    );
    let program = one_site(None, 0, Constants::default());
    assert_eq!(only_reason(&program, policy), Some(Reason::CallbackUnknown));
}

/// The same, run: in a generic-only unit every qualifying site -- element, index, captures,
/// `Math` -- takes the pool and answers `map`'s bits.
#[test]
fn a_generic_only_unit_runs_its_qualifying_sites_in_the_pool() {
    let element = || Box::new(Expr::Element);
    let callbacks = vec![
        (
            Expr::Binary(BinaryOp::Multiply, element(), Box::new(Expr::Capture(0))),
            1,
        ),
        (
            Expr::Binary(BinaryOp::Add, element(), Box::new(Expr::Index)),
            2,
        ),
        (Expr::Math1("sqrt", element()), 1),
        (
            Expr::Binary(
                BinaryOp::Divide,
                Box::new(Expr::Number(1.0)),
                Box::new(Expr::Binary(
                    BinaryOp::Subtract,
                    element(),
                    Box::new(Expr::Capture(1)),
                )),
            ),
            1,
        ),
    ];
    let policy = EmissionPolicy {
        generic_only: true,
        parallel_in_generic: true,
        ..Default::default()
    };
    let input = [
        0.0,
        -0.0,
        1.5,
        -2.0,
        f64::NAN,
        f64::INFINITY,
        4.0,
        1e-310,
        9.0,
    ];
    if let Some(failure) = compare_with(&callbacks, &input, [1.25, -0.0], policy) {
        panic!("{failure}");
    }
}

#[test]
fn reason_extra_arguments() {
    let mut constants = Constants::default();
    let callback = arithmetic(&mut constants);
    let program = one_site(Some(callback), 1, constants);
    assert_eq!(
        only_reason(&program, EmissionPolicy::default()),
        Some(Reason::ExtraArguments)
    );
}

#[test]
fn reason_callback_unknown() {
    let program = one_site(None, 0, Constants::default());
    assert_eq!(
        only_reason(&program, EmissionPolicy::default()),
        Some(Reason::CallbackUnknown)
    );
}

#[test]
fn reason_returns_non_number() {
    // `x => !x`: every rule holds, and the answer is a boolean.
    let callback = proto(
        vec![
            Instruction::LoadLocal(0),
            Instruction::Unary(UnaryOp::Not),
            Instruction::Return,
        ],
        1,
        Vec::new(),
    );
    let program = one_site(Some(callback), 0, Constants::default());
    assert_eq!(
        only_reason(&program, EmissionPolicy::default()),
        Some(Reason::ReturnsNonNumber)
    );
}

#[test]
fn a_callback_with_no_typed_entry_runs_as_a_tier_2_clone() {
    // `x => x + 1 + 1 + ...`, qualifying at the source and past the typed-entry size limit. Tier
    // 1 needs a typed entry and says `not-typed`; the heap clone of tier 2 needs none, and every
    // callback tier 1's source rules admit, tier 2's admit too, so the site runs in the pool.
    let mut constants = Constants::default();
    let one = constants.number(1.0);
    let mut code = vec![Instruction::LoadLocal(0)];
    for _ in 0..(crate::MAX_SPECIALIZED_INSTRUCTIONS / 2 + 1) {
        code.push(Instruction::Const(one));
        code.push(Instruction::Binary(BinaryOp::Add));
    }
    code.push(Instruction::Return);
    let program = one_site(Some(proto(code, 1, Vec::new())), 0, constants);
    let records = sites(&program, EmissionPolicy::default());
    assert_eq!(records.len(), 1, "{records:?}");
    assert!(
        records[0].refusal.is_none() && records[0].heap,
        "{records:?}"
    );
}

#[test]
fn reason_clone_rejected() {
    // The machine check in `lib.rs` is the last word: a proto whose emitted clone it rejects is
    // sequential at every site, and says so. Narrow the plan the way `lib.rs` does.
    let mut constants = Constants::default();
    let callback = arithmetic(&mut constants);
    let program = one_site(Some(callback), 0, constants);
    let graphs: Vec<_> = program
        .functions
        .iter()
        .map(|p| crate::graph::build_function_graph(p, &program).unwrap())
        .collect();
    let entry = crate::graph::build_function_graph(
        &FunctionProto {
            code: program.code.clone(),
            ..proto(Vec::new(), 0, Vec::new())
        },
        &program,
    )
    .unwrap();
    let mut plan = crate::direct::DirectPlan::plan(
        &program,
        &graphs,
        &entry,
        &[false],
        false,
        false,
        false,
        false,
        None,
    );
    assert!(plan.entry.parallel_safe[0]);
    plan.set_parallel(&[false]);
    assert!(!plan.entry.parallel_safe[0]);
    // The verdict is kept (a `parallelSort` comparator needs its proven order and no clone);
    // a site whose callback would run on the pool reads `parallel_safe` and says
    // `clone-rejected`.
    assert!(plan.entry.parallel[0].is_ok());
}

#[test]
fn tier_1_a_capture_written_once_is_parallel_and_one_written_twice_is_not() {
    // let c = <cell 0>; c = 1.5; [c = 2.5;] [].parallelMap(x => x * c)
    for writes in [1usize, 2] {
        let mut constants = Constants::default();
        let name = constants.string("parallelMap");
        let a = constants.number(1.5);
        let b = constants.number(2.5);
        let callback = proto(
            vec![
                Instruction::LoadLocal(0),
                Instruction::LoadUpvalue(0),
                Instruction::Binary(BinaryOp::Multiply),
                Instruction::Return,
            ],
            1,
            vec![Upvalue::NamedParentLocal {
                slot: 0,
                name: constants.string("c"),
            }],
        );
        let mut code = vec![
            Instruction::NewCell(0),
            Instruction::Const(a),
            Instruction::StoreCell(0),
        ];
        if writes == 2 {
            code.extend([Instruction::Const(b), Instruction::StoreCell(0)]);
        }
        code.extend([
            Instruction::NewArray { count: 0 },
            Instruction::MakeFunction(0),
            Instruction::CallMethod { name, argc: 1 },
            Instruction::Return,
        ]);
        let program = Program {
            source: "".into(),
            version: BYTECODE_VERSION,
            source_origin: None,
            static_source_identity: None,
            static_source_provenance: None,
            constants: constants.0,
            code,
            local_count: 1,
            functions: vec![callback],
            template_sites: Vec::new(),
            debug_call_sites: Vec::new(),
        };
        let records = sites(&program, EmissionPolicy::default());
        assert_eq!(records.len(), 1, "{records:?}");
        if writes == 1 {
            assert_eq!(records[0].refusal, None, "{records:?}");
            assert_eq!(records[0].captures, 1);
        } else {
            let refusal = records[0].refusal.clone().expect("sequential");
            assert_eq!(refusal.reason, Reason::CapturesReassigned);
            assert_eq!(refusal.detail.as_deref(), Some("c"));
            assert_eq!(refusal.writes, Some(2));
        }
    }
}

// --- The property test ------------------------------------------------------------------------

const MATH_1: &[&str] = &[
    "sqrt", "abs", "floor", "ceil", "round", "trunc", "sign", "sin", "cos", "atan", "exp", "log",
    "cbrt", "tanh", "log1p",
];
const MATH_2: &[&str] = &["min", "max", "pow", "atan2", "hypot"];
const OPS: &[BinaryOp] = &[
    BinaryOp::Add,
    BinaryOp::Subtract,
    BinaryOp::Multiply,
    BinaryOp::Divide,
    BinaryOp::Remainder,
    BinaryOp::Exponentiate,
    BinaryOp::BitwiseAnd,
    BinaryOp::BitwiseOr,
    BinaryOp::BitwiseXor,
    BinaryOp::ShiftLeft,
    BinaryOp::ShiftRight,
    BinaryOp::UnsignedShiftRight,
];

/// A callback body from the qualifying grammar of tiers 0 and 1.
#[derive(Debug, Clone)]
enum Expr {
    Element,
    Index,
    Capture(u32),
    Number(f64),
    Negate(Box<Expr>),
    BitwiseNot(Box<Expr>),
    Binary(BinaryOp, Box<Expr>, Box<Expr>),
    Math1(&'static str, Box<Expr>),
    Math2(&'static str, Box<Expr>, Box<Expr>),
}

fn number() -> impl Strategy<Value = f64> {
    prop_oneof![
        Just(0.0),
        Just(-0.0),
        Just(1.0),
        Just(-1.5),
        Just(f64::NAN),
        Just(f64::INFINITY),
        Just(f64::NEG_INFINITY),
        Just(2147483648.0),
        Just(4294967295.0),
        Just(9007199254740993.0),
        Just(1e-310),
        (-1.0e6f64..1.0e6),
        (-40i32..40).prop_map(f64::from),
    ]
}

fn expr() -> impl Strategy<Value = Expr> {
    let leaf = prop_oneof![
        3 => Just(Expr::Element),
        1 => Just(Expr::Index),
        2 => (0u32..2).prop_map(Expr::Capture),
        2 => number().prop_map(Expr::Number),
    ];
    leaf.prop_recursive(4, 24, 2, |inner| {
        prop_oneof![
            1 => inner.clone().prop_map(|e| Expr::Negate(Box::new(e))),
            1 => inner.clone().prop_map(|e| Expr::BitwiseNot(Box::new(e))),
            4 => (0..OPS.len(), inner.clone(), inner.clone())
                .prop_map(|(op, l, r)| Expr::Binary(OPS[op], Box::new(l), Box::new(r))),
            2 => (0..MATH_1.len(), inner.clone())
                .prop_map(|(f, e)| Expr::Math1(MATH_1[f], Box::new(e))),
            1 => (0..MATH_2.len(), inner.clone(), inner)
                .prop_map(|(f, a, b)| Expr::Math2(MATH_2[f], Box::new(a), Box::new(b))),
        ]
    })
}

fn lower(expr: &Expr, constants: &mut Constants, code: &mut Vec<Instruction>) {
    match expr {
        Expr::Element => code.push(Instruction::LoadLocal(0)),
        Expr::Index => code.push(Instruction::LoadLocal(1)),
        Expr::Capture(k) => code.push(Instruction::LoadUpvalue(*k)),
        Expr::Number(v) => code.push(Instruction::Const(constants.number(*v))),
        Expr::Negate(e) => {
            lower(e, constants, code);
            code.push(Instruction::Unary(UnaryOp::Negate));
        }
        Expr::BitwiseNot(e) => {
            lower(e, constants, code);
            code.push(Instruction::Unary(UnaryOp::BitwiseNot));
        }
        Expr::Binary(op, l, r) => {
            lower(l, constants, code);
            lower(r, constants, code);
            code.push(Instruction::Binary(*op));
        }
        Expr::Math1(f, e) => {
            code.push(Instruction::LoadIntrinsicBuiltin(Builtin::Math));
            lower(e, constants, code);
            let name = constants.string(f);
            code.push(Instruction::CallMethod { name, argc: 1 });
        }
        Expr::Math2(f, a, b) => {
            code.push(Instruction::LoadIntrinsicBuiltin(Builtin::Math));
            lower(a, constants, code);
            lower(b, constants, code);
            let name = constants.string(f);
            code.push(Instruction::CallMethod { name, argc: 2 });
        }
    }
}

/// The fixture pool: every call split across four threads, whatever its size, and counted.
const POOL_FIXTURE: &str = r#"
#include <pthread.h>
static int kiln_test_pmap_calls = 0;
typedef struct { const void *in; void *out; size_t start, end; kiln_parallel_chunk_fn chunk; void *ctx; } kiln_test_pmap_job;
static void *kiln_test_pmap_run(void *p) {
    kiln_test_pmap_job *job = (kiln_test_pmap_job *)p;
    job->chunk(job->in, job->out, job->start, job->end, job->ctx);
    return NULL;
}
size_t kiln_parallel_threads(void) { return 4; }
int32_t kiln_parallel_map(const void *in, size_t count, void *out, kiln_parallel_chunk_fn chunk, void *ctx) {
    enum { THREADS = 4 };
    pthread_t threads[THREADS];
    kiln_test_pmap_job jobs[THREADS];
    int started[THREADS] = { 0 };
    size_t per = (count + THREADS - 1) / THREADS;
    size_t t;
    if (chunk == NULL) return -1;
    kiln_test_pmap_calls++;
    for (t = 0; t < THREADS; ++t) {
        size_t start = t * per, end = start + per < count ? start + per : count;
        if (start >= end) continue;
        jobs[t].in = in; jobs[t].out = out; jobs[t].start = start; jobs[t].end = end;
        jobs[t].chunk = chunk; jobs[t].ctx = ctx;
        started[t] = pthread_create(&threads[t], NULL, kiln_test_pmap_run, &jobs[t]) == 0;
        if (!started[t]) kiln_test_pmap_run(&jobs[t]);
    }
    for (t = 0; t < THREADS; ++t) if (started[t]) pthread_join(threads[t], NULL);
    return 0;
}
"#;

fn run_threaded(generated: &CModule, wrapper: &str) -> std::process::Output {
    let executable = std::env::temp_dir().join(format!(
        "kiln-backend-c-parallel-{}-{}",
        std::process::id(),
        generated.program_hash
    ));
    let mut child = Command::new("cc")
        .args([
            "-std=c99",
            "-Werror=implicit-function-declaration",
            "-pthread",
            "-x",
            "c",
            "-",
            "-lm",
            "-o",
        ])
        .arg(&executable)
        .stdin(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("a C compiler is available for backend tests");
    child
        .stdin
        .take()
        .expect("stdin is piped")
        .write_all(
            crate::tests::linked_source(&generated.source, &format!("{POOL_FIXTURE}\n{wrapper}"))
                .as_bytes(),
        )
        .expect("source is accepted by cc stdin");
    let compile = child.wait_with_output().expect("cc exits");
    if let Some(keep) = std::env::var_os("KILN_TEST_KEEP_C") {
        let _ = std::fs::write(
            keep,
            crate::tests::linked_source(&generated.source, &format!("{POOL_FIXTURE}\n{wrapper}")),
        );
    }
    assert!(
        compile.status.success(),
        "generated C did not compile:\n{}",
        String::from_utf8_lossy(&compile.stderr)
    );
    let run = Command::new(&executable)
        .output()
        .expect("generated executable runs");
    let _ = std::fs::remove_file(&executable);
    run
}

/// Every callback run through `parallelMap` and `map` over `input`, with captures `c0`, `c1`;
/// returns the failure text, or `None` when every pair is identical bit for bit and every
/// `parallelMap` took the pool.
fn compare(callbacks: &[(Expr, u32)], input: &[f64], captures: [f64; 2]) -> Option<String> {
    compare_with(callbacks, input, captures, EmissionPolicy::default())
}

/// [`compare`], emitted under `policy`.
fn compare_with(
    callbacks: &[(Expr, u32)],
    input: &[f64],
    captures: [f64; 2],
    policy: EmissionPolicy,
) -> Option<String> {
    let mut constants = Constants::default();
    let parallel_map = constants.string("parallelMap");
    let map = constants.string("map");
    let names = [constants.string("c0"), constants.string("c1")];
    let upvalues: Vec<Upvalue> = (0..2)
        .map(|k| Upvalue::NamedParentLocal {
            slot: k,
            name: names[k as usize],
        })
        .collect();
    let mut functions = Vec::new();
    for (expr, arity) in callbacks {
        let mut code = Vec::new();
        lower(expr, &mut constants, &mut code);
        code.push(Instruction::Return);
        // One proto per call, so each closure is made at exactly one site and the compiler
        // knows the callback at both.
        functions.push(proto(code.clone(), *arity, upvalues.clone()));
        functions.push(proto(code, *arity, upvalues.clone()));
    }
    // Cells 0 and 1 are the captures, each written once; local 2 is the input.
    let mut code = vec![Instruction::NewCell(0), Instruction::NewCell(1)];
    for (k, value) in captures.iter().enumerate() {
        code.push(Instruction::Const(constants.number(*value)));
        code.push(Instruction::StoreCell(k as u32));
    }
    for value in input {
        code.push(Instruction::Const(constants.number(*value)));
    }
    code.push(Instruction::NewArray {
        count: input.len() as u32,
    });
    code.push(Instruction::StoreLocal(2));
    for k in 0..callbacks.len() {
        code.extend([
            Instruction::LoadLocal(2),
            Instruction::MakeFunction((2 * k) as u32),
            Instruction::CallMethod {
                name: parallel_map,
                argc: 1,
            },
            Instruction::LoadLocal(2),
            Instruction::MakeFunction((2 * k + 1) as u32),
            Instruction::CallMethod { name: map, argc: 1 },
        ]);
    }
    code.push(Instruction::NewArray {
        count: (2 * callbacks.len()) as u32,
    });
    code.push(Instruction::Return);
    let program = Program {
        source: "".into(),
        version: BYTECODE_VERSION,
        source_origin: None,
        static_source_identity: None,
        static_source_provenance: None,
        constants: constants.0,
        code,
        local_count: 3,
        functions,
        template_sites: Vec::new(),
        debug_call_sites: Vec::new(),
    };
    let generated = emit_with_policy(&program, policy).expect("emits");
    let records = SiteRecord::parse_all(&generated.source);
    if let Some(sequential) = records.iter().find(|r| r.refusal.is_some()) {
        return Some(format!(
            "a qualifying callback was not lowered parallel: {sequential:?} in {callbacks:?}"
        ));
    }
    // A scalar unit's result crosses the boundary as text, so the program returns
    // `[p0, m0, p1, m1, ...]` and the comparison is on its text: numbers print exactly (the
    // shortest round-trip form), and `-0`, which prints as `0`, is told apart by the reciprocal
    // variant of every callback, which `compare` adds.
    let wrapper = format!(
        "int main(void) {{\n\
             kiln_value result; size_t i;\n\
             kiln_test_realm_active = 1; /* run the body in place, so the text outlives it */\n\
             int status = {symbol}(NULL, &result);\n\
             if (status != 0 || result.tag != KILN_VALUE_STRING) {{ printf(\"status %d tag %d\\n\", status, (int)result.tag); return 2; }}\n\
             const kiln_string *text = (const kiln_string *)result.ptr;\n\
             for (i = 0; i < text->unit_len; ++i) putchar((int)kiln_string_unit_at(text, i));\n\
             printf(\"\\n%d\\n\", kiln_test_pmap_calls);\n\
             return 0;\n\
         }}\n",
        symbol = generated.symbol,
    );
    let run = run_threaded(&generated, &wrapper);
    let stdout = String::from_utf8_lossy(&run.stdout);
    if !run.status.success() {
        return Some(format!(
            "{:?}\n{stdout}{}\n{callbacks:?}",
            run.status,
            String::from_utf8_lossy(&run.stderr)
        ));
    }
    let mut lines = stdout.lines();
    let text = lines.next().unwrap_or_default();
    let calls: usize = lines
        .next()
        .and_then(|l| l.trim().parse().ok())
        .unwrap_or(0);
    let tokens: Vec<&str> = text.split(',').collect();
    let n = input.len();
    if tokens.len() != 2 * callbacks.len() * n {
        return Some(format!("unexpected result shape: {text}\n{callbacks:?}"));
    }
    for (k, callback) in callbacks.iter().enumerate() {
        let parallel = &tokens[2 * k * n..(2 * k + 1) * n];
        let sequential = &tokens[(2 * k + 1) * n..(2 * k + 2) * n];
        if parallel != sequential {
            return Some(format!(
                "callback {k} {callback:?} over {input:?} with captures {captures:?}:\n  parallelMap {parallel:?}\n  map         {sequential:?}"
            ));
        }
    }
    (calls != callbacks.len()).then(|| {
        format!(
            "{calls} pool calls for {} parallelMap sites: a site fell back to sequential\n{callbacks:?}",
            callbacks.len()
        )
    })
}

proptest! {
    // Each case is one C compile of the whole runtime, so a case carries many callbacks and
    // there are few cases; shrinking is capped for the same reason.
    #![proptest_config(ProptestConfig {
        cases: 3,
        max_shrink_iters: 8,
        failure_persistence: None,
        ..ProptestConfig::default()
    })]

    #[test]
    fn parallel_map_is_map_bit_for_bit(
        callbacks in prop::collection::vec((expr(), 1u32..=2), 8..12),
        input in prop::collection::vec(number(), 1..40),
        captures in (number(), number()),
    ) {
        // A callback that names the index must declare it.
        let callbacks: Vec<(Expr, u32)> = callbacks
            .into_iter()
            .map(|(e, arity)| {
                let arity = if uses_index(&e) { 2 } else { arity };
                (e, arity)
            })
            .collect();
        // Each callback also as `1 / (callback)`, so a `-0` the text would print as `0` shows.
        let callbacks: Vec<(Expr, u32)> = callbacks
            .into_iter()
            .flat_map(|(e, arity)| {
                let reciprocal =
                    Expr::Binary(BinaryOp::Divide, Box::new(Expr::Number(1.0)), Box::new(e.clone()));
                [(e, arity), (reciprocal, arity)]
            })
            .collect();
        if let Some(failure) = compare(&callbacks, &input, [captures.0, captures.1]) {
            prop_assert!(false, "{}", failure);
        }
    }
}

fn uses_index(expr: &Expr) -> bool {
    match expr {
        Expr::Index => true,
        Expr::Element | Expr::Capture(_) | Expr::Number(_) => false,
        Expr::Negate(e) | Expr::BitwiseNot(e) | Expr::Math1(_, e) => uses_index(e),
        Expr::Binary(_, a, b) | Expr::Math2(_, a, b) => uses_index(a) || uses_index(b),
    }
}

/// The property test's first find: the typed `Math.min`/`Math.max`/`Math.pow`/two-argument
/// lowering declared its operands as `double d0, d1`, which shadowed the SSA values `d0` and
/// `d1`, so `Math.min(-i, x)` read its own first operand as its second -- in plain `map`, not
/// only here. `parallelMap` had it right; `map` answered `-i` for every element.
#[test]
fn math_min_operands_are_not_shadowed_by_their_temporaries() {
    let e = Expr::Math2(
        "min",
        Box::new(Expr::Negate(Box::new(Expr::Index))),
        Box::new(Expr::Element),
    );
    let failure = compare(&[(e, 2)], &[-1.5, 5.0, -7.0], [1.0, 2.0]);
    assert!(failure.is_none(), "{failure:?}");
}

/// Integer operands do not make a remainder an integer: a zero divisor yields NaN,
/// and an exact division of a negative dividend yields negative zero. Exercise both
/// the boxed return and double consumers (reciprocals expose the sign of zero).
#[test]
fn integer_remainders_preserve_nan_and_negative_zero_when_boxed() {
    let integer_element = Expr::Binary(
        BinaryOp::BitwiseOr,
        Box::new(Expr::Element),
        Box::new(Expr::Number(0.0)),
    );
    let divisors = [
        Expr::Binary(
            BinaryOp::ShiftRight,
            Box::new(Expr::Number(1e-310)),
            Box::new(Expr::Index),
        ),
        Expr::Number(2.0),
        Expr::Binary(
            BinaryOp::BitwiseOr,
            Box::new(Expr::Index),
            Box::new(Expr::Number(0.0)),
        ),
    ];
    let callbacks = divisors
        .into_iter()
        .flat_map(|divisor| {
            let remainder = Expr::Binary(
                BinaryOp::Remainder,
                Box::new(integer_element.clone()),
                Box::new(divisor),
            );
            let reciprocal = Expr::Binary(
                BinaryOp::Divide,
                Box::new(Expr::Number(1.0)),
                Box::new(remainder.clone()),
            );
            [(remainder, 2), (reciprocal, 2)]
        })
        .collect::<Vec<_>>();
    let failure = compare(
        &callbacks,
        &[-4.0, -0.0, -4.0, f64::NAN, f64::INFINITY, 4.0, -6.0],
        [0.0, 0.0],
    );
    assert!(failure.is_none(), "{failure:?}");
}

/// Issue 14: with `parallel_workers_unit`, a generic-only module's qualifying callback gets its
/// parallel clone and chunk workers in a unit of their own, which defines the workers under the
/// module's link symbol and compiles on the prelude's declarations alone; the module declares
/// them and keeps its call site's spelling. A module with no qualifying site emits exactly what
/// it did without the flag, and no extra unit.
#[test]
fn a_generic_only_module_puts_its_parallel_workers_in_their_own_unit() {
    let module_of = |program: Program| kiln_ir::ModuleProgram {
        program,
        ..Default::default()
    };
    let generic = EmissionPolicy {
        decoupled_prelude: true,
        compact: true,
        generic_only: true,
        parallel_in_generic: true,
        ..Default::default()
    };
    let split = EmissionPolicy {
        parallel_workers_unit: true,
        ..generic.clone()
    };
    let emit = |program: Program, policy: &EmissionPolicy| {
        crate::emit_module_with_logical_path_and_policy(
            &module_of(program),
            "main.js",
            policy.clone(),
        )
        .expect("emits")
    };
    let site = || {
        let mut constants = Constants::default();
        let callback = arithmetic(&mut constants);
        one_site(Some(callback), 0, constants)
    };

    let together = emit(site(), &generic);
    assert_eq!(together.parallel_workers, None);
    assert!(together.source.contains("_fn_0_p("));

    let apart = emit(site(), &split);
    let workers = apart.parallel_workers.as_deref().expect("a worker unit");
    let symbol = &apart.symbol;
    assert!(
        !apart.source.contains("_fn_0_p("),
        "the clone left the module"
    );
    for kind in crate::emit_graph::parallel_chunk_kinds(1, false) {
        let name = format!("{symbol}_pmap{kind}_0(");
        assert!(workers.contains(&format!("void __attribute__((unused)) {name}")));
        assert!(
            apart.source.contains(&format!("\nvoid {name}")),
            "declared: {name}"
        );
        assert!(
            apart
                .source
                .contains(&format!("#define ku_pmap{kind}_0 {symbol}_pmap{kind}_0\n"))
        );
    }
    assert_eq!(
        SiteRecord::parse_all(&apart.source),
        SiteRecord::parse_all(&together.source)
    );
    assert!(!workers.contains("kiln_host"), "a worker reaches no realm");

    // The unit compiles alone against the prelude's declarations.
    let dir = std::env::temp_dir().join(format!(
        "kiln-backend-c-workers-{}-{}",
        std::process::id(),
        apart.program_hash
    ));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join(crate::RUNTIME_PRELUDE_HEADER_NAME),
        crate::runtime_prelude_header(),
    )
    .unwrap();
    std::fs::write(dir.join("workers.c"), workers).unwrap();
    let compile = Command::new("cc")
        .current_dir(&dir)
        .args([
            "-std=c11",
            "-O2",
            "-Werror=implicit-function-declaration",
            "-c",
            "workers.c",
            "-o",
            "workers.o",
        ])
        .output()
        .expect("a C compiler is available for backend tests");
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        compile.status.success(),
        "worker unit did not compile:\n{}",
        String::from_utf8_lossy(&compile.stderr)
    );

    let none = || one_site(None, 0, Constants::default());
    let plain = emit(none(), &generic);
    let flagged = emit(none(), &split);
    assert_eq!(flagged.parallel_workers, None);
    assert_eq!(flagged.source, plain.source);
    assert_eq!(flagged.symbol, plain.symbol);
}

/// `[].<method>(<callbacks>..., <extra constants>...)` at the top level: each callback its own
/// proto, in order.
fn family_site(
    method: &str,
    callbacks: Vec<FunctionProto>,
    extra: usize,
    mut constants: Constants,
) -> Program {
    let name = constants.string(method);
    let zero = constants.number(0.0);
    let mut code = vec![Instruction::NewArray { count: 0 }];
    for index in 0..callbacks.len() {
        code.push(Instruction::MakeFunction(index as u32));
    }
    for _ in 0..extra {
        code.push(Instruction::Const(zero));
    }
    code.push(Instruction::CallMethod {
        name,
        argc: (callbacks.len() + extra) as u16,
    });
    code.push(Instruction::Return);
    Program {
        source: "".into(),
        version: BYTECODE_VERSION,
        source_origin: None,
        static_source_identity: None,
        static_source_provenance: None,
        constants: constants.0,
        code,
        local_count: 0,
        functions: callbacks,
        template_sites: Vec::new(),
        debug_call_sites: Vec::new(),
    }
}

/// `(a, b) => a <op> b`.
fn binary(op: BinaryOp) -> FunctionProto {
    proto(
        vec![
            Instruction::LoadLocal(0),
            Instruction::LoadLocal(1),
            Instruction::Binary(op),
            Instruction::Return,
        ],
        2,
        Vec::new(),
    )
}

/// The family (parallel issue 10): each site is recorded under its own method, a predicate
/// (a boolean answer) qualifies for `parallelFilter`, `parallelReduce` needs both callbacks and
/// its three arguments, and `parallelSort` needs no comparator or a proven one.
#[test]
fn the_family_sites_are_decided_per_method() {
    use crate::parallel::ParallelMethod;
    let decided = |program: Program| {
        let records = sites(&program, EmissionPolicy::default());
        assert_eq!(records.len(), 1, "one site: {records:?}");
        (
            records[0].method,
            records[0].refusal.as_ref().map(|r| r.reason),
            records[0].default_order,
        )
    };
    // `x => x > 1`: a predicate.
    let mut constants = Constants::default();
    let one = constants.number(1.0);
    let predicate = proto(
        vec![
            Instruction::LoadLocal(0),
            Instruction::Const(one),
            Instruction::Binary(BinaryOp::GreaterThan),
            Instruction::Return,
        ],
        1,
        Vec::new(),
    );
    assert_eq!(
        decided(family_site(
            "parallelFilter",
            vec![predicate.clone()],
            0,
            constants
        )),
        (ParallelMethod::Filter, None, false)
    );
    // ...which `parallelFlatMap` cannot take: its answers are elements.
    let mut constants = Constants::default();
    constants.number(1.0);
    assert_eq!(
        decided(family_site(
            "parallelFlatMap",
            vec![predicate],
            0,
            constants
        )),
        (
            ParallelMethod::FlatMap,
            Some(Reason::ReturnsNonNumber),
            false
        )
    );
    let mut constants = Constants::default();
    let callback = arithmetic(&mut constants);
    assert_eq!(
        decided(family_site("parallelFlatMap", vec![callback], 0, constants)),
        (ParallelMethod::FlatMap, None, false)
    );
    let fold = || binary(BinaryOp::Add);
    assert_eq!(
        decided(family_site(
            "parallelReduce",
            vec![fold(), fold()],
            1,
            Constants::default()
        )),
        (ParallelMethod::Reduce, None, false)
    );
    assert_eq!(
        decided(family_site(
            "parallelReduce",
            vec![fold(), fold()],
            0,
            Constants::default()
        )),
        (ParallelMethod::Reduce, Some(Reason::ArgumentShape), false)
    );
    let mut constants = Constants::default();
    let one_parameter = arithmetic(&mut constants);
    assert_eq!(
        decided(family_site(
            "parallelReduce",
            vec![one_parameter, fold()],
            1,
            constants
        )),
        (ParallelMethod::Reduce, Some(Reason::Arity), false)
    );
    assert_eq!(
        decided(family_site(
            "parallelSort",
            vec![binary(BinaryOp::Subtract)],
            0,
            Constants::default()
        )),
        (ParallelMethod::Sort, None, false)
    );
    assert_eq!(
        decided(family_site(
            "parallelSort",
            vec![binary(BinaryOp::Multiply)],
            0,
            Constants::default()
        )),
        (
            ParallelMethod::Sort,
            Some(Reason::ComparatorUnproven),
            false
        )
    );
    assert_eq!(
        decided(family_site(
            "parallelSort",
            Vec::new(),
            0,
            Constants::default()
        )),
        (ParallelMethod::Sort, None, true)
    );
}

/// Tier 2 (issue 07): callbacks that read the heap run in the pool and answer what the
/// sequential method answers. Records `{ p, q }` (and one hole), read by name by a `parallelMap`
/// and a `parallelFilter` callback and by a `parallelSort` key comparator; the fixture's four
/// threads really run the chunks concurrently, and it counts that every site took the pool.
#[test]
fn tier_2_reads_run_in_the_pool_and_match_the_sequential_methods() {
    let mut constants = Constants::default();
    let parallel_map = constants.string("parallelMap");
    let map = constants.string("map");
    let parallel_filter = constants.string("parallelFilter");
    let filter = constants.string("filter");
    let length = constants.string("length");
    let p = constants.string("p");
    let q = constants.string("q");
    let zero = constants.number(0.0);
    // `r => r.p * r.q`
    let product = || {
        proto(
            vec![
                Instruction::LoadLocal(0),
                Instruction::GetProperty(p),
                Instruction::LoadLocal(0),
                Instruction::GetProperty(q),
                Instruction::Binary(BinaryOp::Multiply),
                Instruction::Return,
            ],
            1,
            Vec::new(),
        )
    };
    // `r => r.q > 0`
    let positive = || {
        proto(
            vec![
                Instruction::LoadLocal(0),
                Instruction::GetProperty(q),
                Instruction::Const(zero),
                Instruction::Binary(BinaryOp::GreaterThan),
                Instruction::Return,
            ],
            1,
            Vec::new(),
        )
    };
    let n = 64u32;
    let mut code = Vec::new();
    for k in 0..n {
        code.push(Instruction::Const(p));
        code.push(Instruction::Const(
            constants.number(f64::from(k) * 1.5 - 7.0),
        ));
        code.push(Instruction::Const(q));
        code.push(Instruction::Const(constants.number(f64::from(k % 5) - 1.0)));
        code.push(Instruction::NewObject { count: 2 });
    }
    code.push(Instruction::NewArray { count: n });
    code.push(Instruction::StoreLocal(0));
    code.extend([
        Instruction::LoadLocal(0),
        Instruction::MakeFunction(0),
        Instruction::CallMethod {
            name: parallel_map,
            argc: 1,
        },
        Instruction::LoadLocal(0),
        Instruction::MakeFunction(1),
        Instruction::CallMethod { name: map, argc: 1 },
        Instruction::LoadLocal(0),
        Instruction::MakeFunction(2),
        Instruction::CallMethod {
            name: parallel_filter,
            argc: 1,
        },
        Instruction::GetProperty(length),
        Instruction::LoadLocal(0),
        Instruction::MakeFunction(3),
        Instruction::CallMethod {
            name: filter,
            argc: 1,
        },
        Instruction::GetProperty(length),
        Instruction::NewArray { count: 4 },
        Instruction::Return,
    ]);
    let program = Program {
        source: "".into(),
        version: BYTECODE_VERSION,
        source_origin: None,
        static_source_identity: None,
        static_source_provenance: None,
        constants: constants.0,
        code,
        local_count: 1,
        functions: vec![product(), product(), positive(), positive()],
        template_sites: Vec::new(),
        debug_call_sites: Vec::new(),
    };
    let generated = emit_with_policy(&program, EmissionPolicy::default()).expect("emits");
    let records = SiteRecord::parse_all(&generated.source);
    assert_eq!(records.len(), 2, "{records:?}");
    for record in &records {
        assert!(
            record.refusal.is_none() && record.heap,
            "a tier-2 site: {record:?}"
        );
    }
    let wrapper = format!(
        "int main(void) {{\n\
             kiln_value result; size_t i;\n\
             kiln_test_realm_active = 1;\n\
             int status = {symbol}(NULL, &result);\n\
             if (status != 0 || result.tag != KILN_VALUE_STRING) {{ printf(\"status %d tag %d\\n\", status, (int)result.tag); return 2; }}\n\
             const kiln_string *text = (const kiln_string *)result.ptr;\n\
             for (i = 0; i < text->unit_len; ++i) putchar((int)kiln_string_unit_at(text, i));\n\
             printf(\"\\n%d\\n\", kiln_test_pmap_calls);\n\
             return 0;\n\
         }}\n",
        symbol = generated.symbol,
    );
    let run = run_threaded(&generated, &wrapper);
    let stdout = String::from_utf8_lossy(&run.stdout);
    assert!(
        run.status.success(),
        "{:?}\n{stdout}{}",
        run.status,
        String::from_utf8_lossy(&run.stderr)
    );
    let mut lines = stdout.lines();
    let tokens: Vec<&str> = lines.next().unwrap_or_default().split(',').collect();
    let calls: usize = lines
        .next()
        .and_then(|l| l.trim().parse().ok())
        .unwrap_or(0);
    let n = n as usize;
    assert_eq!(tokens.len(), 2 * n + 2, "{stdout}");
    assert_eq!(tokens[..n], tokens[n..2 * n], "parallelMap is map");
    assert_eq!(tokens[2 * n], tokens[2 * n + 1], "parallelFilter is filter");
    let expected: Vec<String> = (0..n)
        .map(|k| {
            let v = (k as f64 * 1.5 - 7.0) * ((k % 5) as f64 - 1.0);
            if v == 0.0 {
                "0".to_owned()
            } else {
                format!("{v}")
            }
        })
        .collect();
    assert_eq!(tokens[..n], expected, "the products");
    assert_eq!(calls, 2, "both sites took the pool:\n{stdout}");
}

/// Tier 3 (issue 08): callbacks that build strings and objects run in the pool, each chunk in
/// its thread's arena, and the arenas join the heap: the built values read back as `map`'s. Records
/// `{ p, q }`; `r => r.p + "|" + r.q` against `map`, and `r => ({ ...r, s: r.q })` read back
/// through `.s` against `r => r.q`. The fixture's four threads really run the chunks concurrently,
/// and both tier-3 sites took the pool.
#[test]
fn tier_3_builds_run_in_worker_arenas_and_match_the_sequential_methods() {
    let mut constants = Constants::default();
    let parallel_map = constants.string("parallelMap");
    let map = constants.string("map");
    let p = constants.string("p");
    let q = constants.string("q");
    let s = constants.string("s");
    let bar = constants.string("|");
    // `r => r.p + "|" + r.q`
    let joined = || {
        proto(
            vec![
                Instruction::LoadLocal(0),
                Instruction::GetProperty(p),
                Instruction::Const(bar),
                Instruction::Binary(BinaryOp::Add),
                Instruction::LoadLocal(0),
                Instruction::GetProperty(q),
                Instruction::Binary(BinaryOp::Add),
                Instruction::Return,
            ],
            1,
            Vec::new(),
        )
    };
    // `r => ({ ...r, s: r.q })`
    let enriched = proto(
        vec![
            Instruction::NewObject { count: 0 },
            Instruction::LoadLocal(0),
            Instruction::SpreadObject,
            Instruction::Const(s),
            Instruction::LoadLocal(0),
            Instruction::GetProperty(q),
            Instruction::DefineNamedProperty {
                name_function: true,
            },
            Instruction::Return,
        ],
        1,
        Vec::new(),
    );
    let read = |name| {
        proto(
            vec![
                Instruction::LoadLocal(0),
                Instruction::GetProperty(name),
                Instruction::Return,
            ],
            1,
            Vec::new(),
        )
    };
    let n = 64u32;
    let mut code = Vec::new();
    for k in 0..n {
        code.push(Instruction::Const(p));
        code.push(Instruction::Const(constants.number(f64::from(k) * 0.5)));
        code.push(Instruction::Const(q));
        code.push(Instruction::Const(constants.number(f64::from(k % 5))));
        code.push(Instruction::NewObject { count: 2 });
    }
    code.push(Instruction::NewArray { count: n });
    code.push(Instruction::StoreLocal(0));
    code.extend([
        Instruction::LoadLocal(0),
        Instruction::MakeFunction(0),
        Instruction::CallMethod {
            name: parallel_map,
            argc: 1,
        },
        Instruction::LoadLocal(0),
        Instruction::MakeFunction(1),
        Instruction::CallMethod { name: map, argc: 1 },
        Instruction::LoadLocal(0),
        Instruction::MakeFunction(2),
        Instruction::CallMethod {
            name: parallel_map,
            argc: 1,
        },
        Instruction::MakeFunction(3),
        Instruction::CallMethod { name: map, argc: 1 },
        Instruction::LoadLocal(0),
        Instruction::MakeFunction(4),
        Instruction::CallMethod { name: map, argc: 1 },
        Instruction::NewArray { count: 4 },
        Instruction::Return,
    ]);
    let program = Program {
        source: "".into(),
        version: BYTECODE_VERSION,
        source_origin: None,
        static_source_identity: None,
        static_source_provenance: None,
        constants: constants.0,
        code,
        local_count: 1,
        functions: vec![joined(), joined(), enriched, read(s), read(q)],
        template_sites: Vec::new(),
        debug_call_sites: Vec::new(),
    };
    let generated = emit_with_policy(&program, EmissionPolicy::default()).expect("emits");
    // Both sites sit at the same (empty) source position, so their records are one.
    let records = SiteRecord::parse_all(&generated.source);
    assert_eq!(records.len(), 1, "{records:?}");
    for record in &records {
        assert!(
            record.refusal.is_none() && record.alloc,
            "a tier-3 site: {record:?}"
        );
    }
    let wrapper = format!(
        "int main(void) {{\n\
             kiln_value result; size_t i;\n\
             kiln_test_realm_active = 1;\n\
             int status = {symbol}(NULL, &result);\n\
             if (status != 0 || result.tag != KILN_VALUE_STRING) {{ printf(\"status %d tag %d\\n\", status, (int)result.tag); return 2; }}\n\
             const kiln_string *text = (const kiln_string *)result.ptr;\n\
             for (i = 0; i < text->unit_len; ++i) putchar((int)kiln_string_unit_at(text, i));\n\
             printf(\"\\n%d\\n\", kiln_test_pmap_calls);\n\
             return 0;\n\
         }}\n",
        symbol = generated.symbol,
    );
    let run = run_threaded(&generated, &wrapper);
    let stdout = String::from_utf8_lossy(&run.stdout);
    assert!(
        run.status.success(),
        "{:?}\n{stdout}{}",
        run.status,
        String::from_utf8_lossy(&run.stderr)
    );
    let mut lines = stdout.lines();
    let tokens: Vec<&str> = lines.next().unwrap_or_default().split(',').collect();
    let calls: usize = lines
        .next()
        .and_then(|l| l.trim().parse().ok())
        .unwrap_or(0);
    let n = n as usize;
    assert_eq!(tokens.len(), 4 * n, "{stdout}");
    assert_eq!(
        tokens[..n],
        tokens[n..2 * n],
        "the joined strings are map's"
    );
    assert_eq!(
        tokens[2 * n..3 * n],
        tokens[3 * n..],
        "the built objects read back"
    );
    assert_eq!(tokens[3], "1.5|3", "{stdout}");
    assert_eq!(calls, 2, "both sites took the pool:\n{stdout}");
}

/// Tier 4 (parallel issue 09): callbacks that call pure built-ins -- `trim`, `split`, `map` with
/// the `Number` global, `join`, `toUpperCase` -- and a closure written in the callback, run on the
/// pool and answer exactly what `map` answers.
#[test]
fn tier_4_pure_calls_run_in_the_pool_and_match_the_sequential_methods() {
    let mut constants = Constants::default();
    let parallel_map = constants.string("parallelMap");
    let map = constants.string("map");
    let trim = constants.string("trim");
    let split = constants.string("split");
    let semicolon = constants.string(";");
    let join = constants.string("join");
    let bar = constants.string("|");
    let upper = constants.string("toUpperCase");
    // `l => l.trim().split(";").map(Number).join("|")`
    let parse = || {
        proto(
            vec![
                Instruction::LoadLocal(0),
                Instruction::CallMethod {
                    name: trim,
                    argc: 0,
                },
                Instruction::Const(semicolon),
                Instruction::CallMethod {
                    name: split,
                    argc: 1,
                },
                Instruction::LoadIntrinsicBuiltin(Builtin::Number),
                Instruction::CallMethod { name: map, argc: 1 },
                Instruction::Const(bar),
                Instruction::CallMethod {
                    name: join,
                    argc: 1,
                },
                Instruction::Return,
            ],
            1,
            Vec::new(),
        )
    };
    // `l => [l].map(x => x.toUpperCase()).join("|")`, the inner arrow being proto 4.
    let shout = || {
        proto(
            vec![
                Instruction::LoadLocal(0),
                Instruction::NewArray { count: 1 },
                Instruction::MakeFunction(4),
                Instruction::CallMethod { name: map, argc: 1 },
                Instruction::Const(bar),
                Instruction::CallMethod {
                    name: join,
                    argc: 1,
                },
                Instruction::Return,
            ],
            1,
            Vec::new(),
        )
    };
    let upper_case = proto(
        vec![
            Instruction::LoadLocal(0),
            Instruction::CallMethod {
                name: upper,
                argc: 0,
            },
            Instruction::Return,
        ],
        1,
        Vec::new(),
    );
    let n = 64u32;
    let mut code = Vec::new();
    for k in 0..n {
        let text = format!(" a{k};{} ", k % 5);
        code.push(Instruction::Const(constants.string(&text)));
    }
    code.push(Instruction::NewArray { count: n });
    code.push(Instruction::StoreLocal(0));
    code.extend([
        Instruction::LoadLocal(0),
        Instruction::MakeFunction(0),
        Instruction::CallMethod {
            name: parallel_map,
            argc: 1,
        },
        Instruction::LoadLocal(0),
        Instruction::MakeFunction(1),
        Instruction::CallMethod { name: map, argc: 1 },
        Instruction::LoadLocal(0),
        Instruction::MakeFunction(2),
        Instruction::CallMethod {
            name: parallel_map,
            argc: 1,
        },
        Instruction::LoadLocal(0),
        Instruction::MakeFunction(3),
        Instruction::CallMethod { name: map, argc: 1 },
        Instruction::NewArray { count: 4 },
        Instruction::Return,
    ]);
    let program = Program {
        source: "".into(),
        version: BYTECODE_VERSION,
        source_origin: None,
        static_source_identity: None,
        static_source_provenance: None,
        constants: constants.0,
        code,
        local_count: 1,
        functions: vec![parse(), parse(), shout(), shout(), upper_case],
        template_sites: Vec::new(),
        debug_call_sites: Vec::new(),
    };
    let generated = emit_with_policy(&program, EmissionPolicy::default()).expect("emits");
    let records = SiteRecord::parse_all(&generated.source);
    assert!(!records.is_empty());
    for record in &records {
        assert!(
            record.refusal.is_none() && record.calls,
            "a tier-4 site: {record:?}"
        );
    }
    let wrapper = format!(
        "int main(void) {{\n\
             kiln_value result; size_t i;\n\
             kiln_test_realm_active = 1;\n\
             int status = {symbol}(NULL, &result);\n\
             if (status != 0 || result.tag != KILN_VALUE_STRING) {{ printf(\"status %d tag %d\\n\", status, (int)result.tag); return 2; }}\n\
             const kiln_string *text = (const kiln_string *)result.ptr;\n\
             for (i = 0; i < text->unit_len; ++i) putchar((int)kiln_string_unit_at(text, i));\n\
             printf(\"\\n%d\\n\", kiln_test_pmap_calls);\n\
             return 0;\n\
         }}\n",
        symbol = generated.symbol,
    );
    let run = run_threaded(&generated, &wrapper);
    let stdout = String::from_utf8_lossy(&run.stdout);
    assert!(
        run.status.success(),
        "{:?}\n{stdout}{}",
        run.status,
        String::from_utf8_lossy(&run.stderr)
    );
    let mut lines = stdout.lines();
    let tokens: Vec<&str> = lines.next().unwrap_or_default().split(',').collect();
    let calls: usize = lines
        .next()
        .and_then(|l| l.trim().parse().ok())
        .unwrap_or(0);
    let n = n as usize;
    assert_eq!(tokens.len(), 4 * n, "{stdout}");
    assert_eq!(tokens[..n], tokens[n..2 * n], "the parsed lines are map's");
    assert_eq!(
        tokens[2 * n..3 * n],
        tokens[3 * n..],
        "the shouted lines are map's"
    );
    assert_eq!(tokens[3], "NaN|3", "{stdout}");
    assert_eq!(tokens[2 * n + 3], " A3;3 ", "{stdout}");
    assert_eq!(calls, 2, "both sites took the pool:\n{stdout}");
}
