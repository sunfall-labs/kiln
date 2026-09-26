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
        constants: constants.0,
        code,
        local_count: 0,
        functions: callback.into_iter().collect(),
        template_sites: Vec::new(),
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
fn reason_generic_only() {
    let mut constants = Constants::default();
    let callback = arithmetic(&mut constants);
    let program = one_site(Some(callback), 0, constants);
    let policy = EmissionPolicy {
        generic_only: true,
        ..Default::default()
    };
    assert_eq!(only_reason(&program, policy), Some(Reason::GenericOnly));
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
fn reason_not_typed() {
    // `x => x + 1 + 1 + ...`, qualifying at the source and past the typed-entry size limit.
    let mut constants = Constants::default();
    let one = constants.number(1.0);
    let mut code = vec![Instruction::LoadLocal(0)];
    for _ in 0..(crate::MAX_SPECIALIZED_INSTRUCTIONS / 2 + 1) {
        code.push(Instruction::Const(one));
        code.push(Instruction::Binary(BinaryOp::Add));
    }
    code.push(Instruction::Return);
    let program = one_site(Some(proto(code, 1, Vec::new())), 0, constants);
    assert_eq!(
        only_reason(&program, EmissionPolicy::default()),
        Some(Reason::NotTyped)
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
        None,
    );
    assert!(plan.entry.parallel_safe[0]);
    plan.set_parallel(&[false]);
    assert!(!plan.entry.parallel_safe[0]);
    assert_eq!(
        plan.entry.parallel[0].as_ref().err().map(|r| r.reason),
        Some(Reason::CloneRejected)
    );
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
            constants: constants.0,
            code,
            local_count: 1,
            functions: vec![callback],
            template_sites: Vec::new(),
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
            code.push(Instruction::LoadBuiltin(Builtin::Math));
            lower(e, constants, code);
            let name = constants.string(f);
            code.push(Instruction::CallMethod { name, argc: 1 });
        }
        Expr::Math2(f, a, b) => {
            code.push(Instruction::LoadBuiltin(Builtin::Math));
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
        constants: constants.0,
        code,
        local_count: 3,
        functions,
        template_sites: Vec::new(),
    };
    let generated = emit_with_policy(&program, EmissionPolicy::default()).expect("emits");
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
