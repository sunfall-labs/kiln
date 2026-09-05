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

use std::collections::{BTreeMap, BTreeSet};

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
        source: "".into(),
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
        span: kiln_ir::Span::default(),
        prologue_end: 0,
        asynchronous: false,
        name: Some(name.to_owned()),
        arrow: false,
        generator: false,
        async_generator: false,
        parameter_count,
        declared_length: parameter_count,
        mapped_arguments: false,
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

    // Two invariants about method IDs, and the reason they are worth a test rather than a
    // comment: they collide on almost every merge, because two people adding builtins both reach
    // for the next free number. The resolution is not a matter of taste, it follows from how each
    // block is dispatched, and rediscovering that from a merge conflict costs an hour.
    //
    //   * `Temporal` is dispatched by an explicit *range* (`code >= TEMPORAL_FROM && code <=
    //     TEMPORAL_LAST` in `kiln_c99_invoke_method`), so its block must stay contiguous and
    //     nothing else may sit inside it.
    //   * The node, buffer and url codes are matched by *equality*, one `if` per code, so they do
    //     not care what number they hold and can be renumbered freely.
    //
    // So when the two collide: shift the equality-matched block, never the range-matched one.
    let temporal_ids: Vec<u64> = methods
        .iter()
        .filter(|(_, names)| {
            names
                .iter()
                .all(|n| n.starts_with("KILN_C99_METHOD_TEMPORAL_"))
        })
        .map(|(value, _)| *value)
        .collect();
    let temporal_low = *temporal_ids.iter().min().expect("Temporal methods exist");
    let temporal_high = *temporal_ids.iter().max().expect("Temporal methods exist");
    for (value, names) in &methods {
        if *value >= temporal_low && *value <= temporal_high {
            assert!(
                names
                    .iter()
                    .all(|n| n.starts_with("KILN_C99_METHOD_TEMPORAL_")),
                "method ID {value} ({}) sits inside the Temporal range {temporal_low}..={temporal_high}, \
                 which is dispatched by range and must stay contiguous. Renumber this one upward \
                 instead -- it is matched by equality, so its value does not matter.",
                names.join(", ")
            );
        }
    }

    // `TEMPORAL_LAST` is an alias for whatever the highest Temporal code currently is, which is
    // exactly what makes the range check above correct. Deriving it rather than pinning a number
    // means growing the Temporal surface does not require editing this test -- the old pinned
    // form failed twice for the right reason and had to be hand-updated both times.
    let last = methods
        .iter()
        .find(|(_, names)| names.iter().any(|n| *n == "KILN_C99_METHOD_TEMPORAL_LAST"))
        .map(|(value, _)| *value)
        .expect("KILN_C99_METHOD_TEMPORAL_LAST is defined");
    assert_eq!(
        last, temporal_high,
        "KILN_C99_METHOD_TEMPORAL_LAST must alias the highest Temporal code, or the range \
         dispatch in kiln_c99_invoke_method silently stops reaching the newest ones"
    );

    for (value, names) in &methods {
        if names.len() > 1 {
            let mut sorted = names.clone();
            sorted.sort();
            let all_temporal = sorted
                .iter()
                .all(|n| n.starts_with("KILN_C99_METHOD_TEMPORAL_"));
            assert!(
                all_temporal && sorted.iter().any(|n| n == "KILN_C99_METHOD_TEMPORAL_LAST"),
                "duplicate method ID {value} is claimed by {}. The only intended alias is \
                 TEMPORAL_LAST naming the highest Temporal code.",
                names.join(", ")
            );
        }
    }

    // A builtin ID that was in use and is deliberately not reused. Dense numbering is the rule
    // -- a hole is nearly always a merge that dropped a define -- but a code that has *moved out*
    // of a range whose members are defined by being in it cannot be filled by the next arrival
    // without making that range's promise false for one member.
    //
    // 77 was `DataView`, back when `DataView` was one of the unimplemented globals published so
    // that `typeof` would tell the truth about them. It became a real constructor with a code
    // outside that range, and 77 now sits inside `UNAVAILABLE_FIRST..=UNAVAILABLE_LAST`, where
    // every code means "this backend does not implement this global". Handing it to the next
    // global to be implemented would put an implemented one inside the unimplemented range.
    const RETIRED_BUILTINS: &[u64] = &[77];

    // Two builtins may not share a code, and until a `URIError` that read as an object went
    // looking for a reason, nothing said so. The density check below cannot see a collision: it
    // asks `builtins` -- keyed *by value* -- whether each number is present, so two names on one
    // number look exactly like one name on one number, and the run stays dense and green.
    //
    // What it cost: `SYMBOL_PROTOTYPE` and `URI_ERROR` were both 65. `kiln_c99_is_namespace(65)`
    // is true, correctly, for the prototype -- so `URIError` became a namespace. `typeof URIError`
    // answered `"object"` where the interpreter says `"function"`, and `URIError.name` answered
    // `undefined`, which is the silent shape: `typeof X === "undefined"` is how real code
    // feature-detects, so the compiled program takes the other branch rather than failing.
    //
    // The identical guard has existed for *method* codes for as long as the density check has.
    // Only the builtin half was missing, so this is that half, not a new idea.
    //
    // The one intended alias is a range marker naming the member at the end of its range:
    // `UNAVAILABLE_FIRST` is `INTL` and `UNAVAILABLE_LAST` is `EVAL`, by construction.
    for (value, names) in &builtins {
        if names.len() > 1 {
            let mut sorted = names.clone();
            sorted.sort();
            let markers = sorted
                .iter()
                .filter(|n| {
                    *n == "KILN_C99_BUILTIN_UNAVAILABLE_FIRST"
                        || *n == "KILN_C99_BUILTIN_UNAVAILABLE_LAST"
                })
                .count();
            assert!(
                sorted.len() == 2 && markers == 1,
                "duplicate builtin ID {value} is claimed by {}. Two builtins sharing a code make                  the later one read as the earlier: whichever predicate answers for one answers                  for both, and the wrong one is a wrong answer rather than a refusal. The only                  intended aliases are UNAVAILABLE_FIRST and UNAVAILABLE_LAST naming the builtins                  at the ends of their range.",
                sorted.join(", ")
            );
        }
    }

    let builtin_values: Vec<_> = builtins.keys().copied().collect();
    let builtin_max = *builtin_values
        .last()
        .expect("prelude declares at least one builtin ID");
    for retired in RETIRED_BUILTINS {
        assert!(
            !builtins.contains_key(retired),
            "builtin ID {retired} is listed as retired but the prelude declares it; either the \
             declaration is a reuse of a retired number or the retirement is over and the row \
             above should go"
        );
    }
    let missing_builtins: Vec<_> = (1..=builtin_max)
        .filter(|value| !builtins.contains_key(value) && !RETIRED_BUILTINS.contains(value))
        .collect();
    assert!(
        missing_builtins.is_empty(),
        "builtin IDs missing {missing_builtins:?}; declared IDs are {builtin_values:?}"
    );

    // 336 is the first ID appended after the historical receiver/range layout. New runtime
    // facade methods must append at max+1, never fill an old gap or reuse an old value.
    const APPEND_ONLY_METHOD_BASE: u64 = 336;
    // The same reservation `validate_method_ids` records, for the same reason: several branches
    // are open at once, so the numbers are handed out in advance and the branch holding the
    // lower range lands into a hole. Keep the two lists in step.
    const RESERVED_METHOD_IDS: &[u64] = &[442, 443, 444, 445, 446, 447, 448, 449];
    let custom_values: Vec<_> = methods
        .keys()
        .copied()
        .filter(|value| *value >= APPEND_ONLY_METHOD_BASE)
        .collect();
    let custom_max = *custom_values
        .last()
        .expect("prelude declares the append-only method tail");
    let missing_custom: Vec<_> = (APPEND_ONLY_METHOD_BASE..=custom_max)
        .filter(|value| !methods.contains_key(value) && !RESERVED_METHOD_IDS.contains(value))
        .collect();
    assert!(
        missing_custom.is_empty(),
        "custom method IDs missing {missing_custom:?}; declared IDs are {custom_values:?}"
    );
}

/// Every method code a name lookup can walk over must have a name.
///
/// `kiln_c99_lookup_method(key, key_len, FIRST, LAST)` resolves a property name by walking the
/// codes from `FIRST` to `LAST` and comparing `kiln_c99_method_name(code)` to the key. A code in
/// that span with no `case` in the name function returns `NULL`, is skipped, and becomes a method
/// that exists in every dispatch table but that no program can ever call. Nothing fails: the C
/// compiles, the tests that invoke the method by code still pass, and the method is simply
/// invisible from JavaScript.
///
/// This is the same hazard as a `case` naming a `#define` that does not exist -- the C lives in a
/// Rust string literal, so only a C compiler would notice, and here not even that -- one layer up.
#[test]
fn every_method_a_name_lookup_walks_over_has_a_name() {
    let unit = emit(&minimal_program()).expect("minimal program emits");
    let (prelude, _) = split(&unit.source);

    let values: BTreeMap<String, u64> = parse_numeric_id_defines(prelude)
        .expect("numeric ID declarations parse")
        .into_iter()
        .map(|define| (define.name, define.value))
        .collect();

    // Anchor on the *definition*, not the forward declaration: the two differ only by `{` versus
    // `;`, and a guard that matched the declaration would read an empty body and pass no matter
    // what. Assert the declaration exists too, so this stays true if the two ever swap order.
    const SIGNATURE: &str = "static const char *kiln_c99_method_name(uint64_t code)";
    assert!(
        prelude.contains(&format!("{SIGNATURE};")),
        "expected a forward declaration of kiln_c99_method_name; the prelude's shape has changed \
         and this guard's anchors need rechecking"
    );
    let opening = format!("{SIGNATURE} {{");
    let start = prelude
        .find(&opening)
        .expect("kiln_c99_method_name must be defined in the prelude");
    let definition = &prelude[start..];
    let end = definition
        .find("default: return NULL;")
        .expect("the name function ends in a NULL default");
    let definition = &definition[..end];

    let mut named: BTreeSet<u64> = BTreeSet::new();
    for line in definition.lines() {
        let Some(rest) = line.trim().strip_prefix("case ") else {
            continue;
        };
        let Some((name, _)) = rest.split_once(':') else {
            continue;
        };
        if let Some(&value) = values.get(name.trim()) {
            named.insert(value);
        }
    }
    assert!(
        named.len() > 200,
        "parsed only {} name cases out of kiln_c99_method_name, so this guard has stopped reading \
         the real function body",
        named.len(),
    );

    // Two families are answered by table-driven helpers above the switch rather than by a `case`
    // each, so their codes are named without appearing here.
    let mut delegated: BTreeSet<u64> = BTreeSet::new();
    for (first, last) in [
        (
            "KILN_C99_METHOD_TEMPORAL_OP",
            "KILN_C99_METHOD_TEMPORAL_OP_LAST",
        ),
        (
            "KILN_C99_METHOD_TEMPORAL_NOW_READ",
            "KILN_C99_METHOD_TEMPORAL_NOW_READ_LAST",
        ),
    ] {
        if let (Some(&first), Some(&last)) = (values.get(first), values.get(last)) {
            delegated.extend(first..=last);
        }
    }

    // Every code that some `#define` gives a name to, so a gap in a range is not mistaken for a
    // method: only codes that actually exist are required to be nameable.
    let defined: BTreeSet<u64> = values
        .iter()
        .filter(|(name, _)| name.starts_with("KILN_C99_METHOD_"))
        .map(|(_, &value)| value)
        .collect();

    let flattened: String = prelude.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut spans = Vec::new();
    // The emitted C wraps these calls at varying widths, so parse the argument list rather than
    // matching one spelling of the whitespace between the arguments.
    for call in flattened.split("kiln_c99_lookup_method(").skip(1) {
        let Some((arguments, _)) = call.split_once(')') else {
            continue;
        };
        let bounds: Vec<&str> = arguments.split(',').map(str::trim).collect();
        if let [_key, _key_len, first, last] = bounds.as_slice()
            && let (Some(&first), Some(&last)) = (values.get(*first), values.get(*last))
        {
            spans.push((first.min(last), first.max(last)));
        }
    }
    assert!(
        spans.len() > 10,
        "found only {} name-lookup ranges in the prelude, so this guard is no longer reading the \
         call sites it exists to cover",
        spans.len(),
    );

    let mut unreachable = BTreeSet::new();
    for (first, last) in spans {
        for code in first..=last {
            if defined.contains(&code) && !named.contains(&code) && !delegated.contains(&code) {
                unreachable.insert(code);
            }
        }
    }

    let mut report = String::new();
    for code in &unreachable {
        let names: Vec<&str> = values
            .iter()
            .filter(|(name, value)| *value == code && name.starts_with("KILN_C99_METHOD_"))
            .map(|(name, _)| name.as_str())
            .collect();
        report.push_str(&format!("\n  {code} ({})", names.join(", ")));
    }
    assert!(
        unreachable.is_empty(),
        "these method codes sit inside a kiln_c99_lookup_method range but have no case in \
         kiln_c99_method_name, so kiln_c99_method_name returns NULL for them and the lookup skips \
         them -- the method is dispatchable by code and unreachable by name from JavaScript:{report}\
         \n\nAdd `case <NAME>: return \"<jsName>\";` to kiln_c99_method_name, spelled exactly as \
         the JavaScript property is."
    );
}

/// A `case KILN_C99_...: return "text";` switch, read out of the prelude as a map.
///
/// The C lives in a Rust string literal, so nothing but a reader like this one can tell whether a
/// switch covers the codes something else hands it. The parser is deliberately literal: it takes
/// only the one-line `case X: return "y";` form, which is the form every one of these switches is
/// written in, so a case it cannot read is a case it does not claim to have checked.
fn parse_name_switch(prelude: &str, signature: &str) -> BTreeMap<String, String> {
    let opening = format!("{signature} {{");
    let start = prelude
        .find(&opening)
        .unwrap_or_else(|| panic!("{signature} must be defined in the prelude"));
    let definition = &prelude[start..];
    let end = definition
        .find("default:")
        .expect("a name switch ends in a default arm");
    let mut named = BTreeMap::new();
    for line in definition[..end].lines() {
        let Some(rest) = line.trim().strip_prefix("case ") else {
            continue;
        };
        let Some((constant, tail)) = rest.split_once(':') else {
            continue;
        };
        let Some(tail) = tail.trim().strip_prefix("return \"") else {
            continue;
        };
        let Some((text, _)) = tail.split_once('"') else {
            continue;
        };
        named.insert(constant.trim().to_owned(), text.to_owned());
    }
    named
}

/// A `{"name", CONSTANT}` initialiser table, read out of the prelude as pairs.
///
/// `kiln_c99_global_object` builds the global object from two of these -- one of built-in codes
/// and one of method codes -- and they are the realm's own statement of what it publishes and
/// under which name.
fn parse_global_table(prelude: &str, marker: &str) -> Vec<(String, String)> {
    let start = prelude
        .find(marker)
        .unwrap_or_else(|| panic!("the prelude must contain the global table after `{marker}`"));
    let table = &prelude[start..];
    let end = table
        .find("};")
        .expect("a global table initialiser ends with `};`");
    let mut rows = Vec::new();
    let flattened: String = table[..end]
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    for entry in flattened.split('{').skip(1) {
        let Some((body, _)) = entry.split_once('}') else {
            continue;
        };
        let Some((name, constant)) = body.split_once(',') else {
            continue;
        };
        let name = name.trim();
        let Some(name) = name.strip_prefix('"').and_then(|n| n.strip_suffix('"')) else {
            continue;
        };
        rows.push((name.to_owned(), constant.trim().to_owned()));
    }
    rows
}

/// Every name the realm publishes on its global object must report that same name.
///
/// WHY THIS EXISTS. `X.name` is read by real code -- a decorator preserves it, a stack trace
/// prints it, `fn.name === "Buffer"` tests it -- so it is a program-visible answer and not
/// decoration. The realm publishes its globals from two tables in `kiln_c99_global_object`, and it
/// answers `X.name` from two entirely different switches: `kiln_c99_builtin_name` for a built-in
/// and `kiln_c99_method_name` for a method. Nothing connected the two, so a global added to the
/// publishing table and forgotten in the naming switch answered whatever the fall-through gave it.
///
/// It gave two different wrong answers, and both shipped: `AbortController`, `AbortSignal` and
/// `TextEncoder` fell past `kiln_c99_builtin_name` into the error-name lookup and reported
/// "anonymous", while `setImmediate` and `clearImmediate` had no case in `kiln_c99_method_name`
/// at all and reported the empty string. Five wrong answers from one missing connection --- which
/// is what makes this a guard rather than five fixes.
///
/// `every_method_a_name_lookup_walks_over_has_a_name` is the neighbouring guard and does not
/// cover this: it reads the codes inside a `kiln_c99_lookup_method` range, and these globals are
/// resolved by name off the global object instead, so they are in no range.
#[test]
fn every_global_reports_the_name_it_is_published_under() {
    let unit = emit(&minimal_program()).expect("minimal program emits");
    let (prelude, _) = split(&unit.source);

    let builtin_names = parse_name_switch(
        prelude,
        "static const char *kiln_c99_builtin_name(uint64_t builtin)",
    );
    // The error constructors are named by the function that also decides whether a built-in *is*
    // an error, and `kiln_c99_builtin_name` delegates to it rather than repeating the seven.
    let error_names = parse_name_switch(
        prelude,
        "static const char *kiln_c99_error_name(uint64_t builtin)",
    );
    let method_names = parse_name_switch(
        prelude,
        "static const char *kiln_c99_method_name(uint64_t code)",
    );
    assert!(
        builtin_names.len() > 40 && error_names.len() > 5 && method_names.len() > 200,
        "parsed {} builtin, {} error and {} method name cases, so this guard has stopped reading \
         the real switch bodies",
        builtin_names.len(),
        error_names.len(),
        method_names.len(),
    );

    let constructors = parse_global_table(prelude, "} constructors[] =");
    let functions = parse_global_table(prelude, "} functions[] =");
    assert!(
        constructors.len() > 50 && functions.len() > 10,
        "parsed {} constructor rows and {} function rows out of kiln_c99_global_object, so this \
         guard has stopped reading the tables it exists to cover",
        constructors.len(),
        functions.len(),
    );

    let mut wrong = Vec::new();
    for (published, constant) in &constructors {
        let reported = builtin_names
            .get(constant)
            .or_else(|| error_names.get(constant));
        match reported {
            Some(reported) if reported == published => {}
            Some(reported) => wrong.push(format!(
                "  globalThis.{published} is {constant}, which kiln_c99_builtin_name calls \
                 \"{reported}\""
            )),
            None => wrong.push(format!(
                "  globalThis.{published} is {constant}, which has no case in \
                 kiln_c99_builtin_name -- it will report \"anonymous\""
            )),
        }
    }
    for (published, constant) in &functions {
        match method_names.get(constant) {
            Some(reported) if reported == published => {}
            Some(reported) => wrong.push(format!(
                "  globalThis.{published} is {constant}, which kiln_c99_method_name calls \
                 \"{reported}\""
            )),
            None => wrong.push(format!(
                "  globalThis.{published} is {constant}, which has no case in \
                 kiln_c99_method_name -- it will report \"\""
            )),
        }
    }
    assert!(
        wrong.is_empty(),
        "these globals do not report the name they are published under, so `X.name` hands a \
         program a string nobody could have asked for:\n{}\n\nAdd or correct the `case` in the \
         naming switch named above; the name must be spelled exactly as the global is.",
        wrong.join("\n")
    );
}

/// The shared surface is the only place a table-driven method's name is written down.
///
/// `kiln_c99_surface_method_name` reads the name out of the surface row, and
/// `kiln_c99_method_name` asks it first, so a member that is reachable has a name by
/// construction. That is the fix for fifteen rows that had a key here and no `case` there and
/// reported the empty string.
///
/// What construction cannot rule out is a *stale* `case` that survives with a different string:
/// the table would win silently and the switch would keep asserting something else, which is the
/// two-copies problem the change was meant to end. So this pins three things:
///
///   * the delegation is actually there -- without it every surface name falls back to the switch
///     and the fifteen come straight back;
///   * no hand-written `case` disagrees with the row for the same code;
///   * a code serving several members is declared canonical-first, because the scan returns the
///     first row it matches and that is the name all of them will report.
#[test]
fn a_table_driven_methods_name_comes_from_its_surface_row() {
    let unit = emit(&minimal_program()).expect("minimal program emits");
    let (prelude, _) = split(&unit.source);

    assert!(
        prelude.contains("kiln_c99_surface_method_name(code)"),
        "kiln_c99_method_name no longer asks the surface table for a name, so every member whose \
         name lives only in its row reports the empty string again"
    );

    let method_names = parse_name_switch(
        prelude,
        "static const char *kiln_c99_method_name(uint64_t code)",
    );
    assert!(
        method_names.len() > 200,
        "parsed only {} cases out of kiln_c99_method_name, so this guard has stopped reading it",
        method_names.len()
    );

    let mut disagree = Vec::new();
    let mut canonical: BTreeMap<String, &'static str> = BTreeMap::new();
    let mut aliases: Vec<(&'static str, &'static str)> = Vec::new();
    for member in kiln_backend_c::BUILTIN_SURFACE {
        if member.kind != "code" {
            continue;
        }
        let code = member
            .answers
            .trim_matches(|c| c == '(' || c == ')')
            .to_owned();
        if let Some(hand) = method_names.get(&code)
            && hand != member.name
        {
            disagree.push(format!(
                "  {}.{} is {code}: its row names it \"{}\", its case names it \"{hand}\"",
                member.owner, member.name, member.name
            ));
        }
        match canonical.get(&code) {
            // The first row wins the scan, so a later row with a different name is an alias and
            // reports the first one's name.
            Some(first) if *first != member.name => aliases.push((member.owner, member.name)),
            Some(_) => {}
            None => {
                canonical.insert(code, member.name);
            }
        }
    }
    assert!(
        disagree.is_empty(),
        "a hand-written case in kiln_c99_method_name disagrees with the surface row for the same \
         method code. The row wins, so the case is dead text asserting something untrue:\n{}\n\n\
         Delete the case; the row is the name.",
        disagree.join("\n")
    );

    // Every alias the table creates must be declared, and every declared alias must be a real one.
    // A stale entry in `NAME_IS_SHARED` would excuse a member whose name had genuinely drifted.
    let declared: BTreeSet<(&str, &str)> = kiln_builtin_surface::NAME_IS_SHARED
        .iter()
        .copied()
        .collect();
    let found: BTreeSet<(&str, &str)> = aliases.into_iter().collect();
    let undeclared: Vec<_> = found.difference(&declared).collect();
    let stale: Vec<_> = declared.difference(&found).collect();
    assert!(
        undeclared.is_empty(),
        "these members share another member's method code, so they report that member's name \
         rather than their own: {undeclared:?}. Either give them a code of their own, or declare \
         them in kiln_builtin_surface::NAME_IS_SHARED, which is what says the debt is known."
    );
    assert!(
        stale.is_empty(),
        "kiln_builtin_surface::NAME_IS_SHARED excuses {stale:?}, which no longer shares a method \
         code with anything. A stale entry there silences a real drift; delete it."
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
fn emitted_http_socket_close_is_runtime_owned_and_modeled() {
    let unit = emit(&minimal_program()).expect("minimal program emits");
    let source = unit.source;
    assert!(source.contains("#define KILN_REALM_HTTP_CLOSE_GRACEFUL 0"));
    assert!(source.contains("#define KILN_REALM_HTTP_CLOSE_ABORT 1"));
    assert!(
        source.contains("int kiln_realm_http_server_close_active(uint64_t resource, int mode);")
    );

    let branch_start = source
        .find("if (socket->server != NULL && socket->server->protocol_http)")
        .expect("HTTP socket close branch is emitted");
    let generic_start = source[branch_start..]
        .find("    kiln_c99_node_cancel_request(&socket->write_request")
        .map(|offset| branch_start + offset)
        .expect("generic node:net close branch follows HTTP branch");
    let http_branch = &source[branch_start..generic_start];
    assert!(http_branch.contains("kiln_realm_http_server_close_active("));
    assert!(http_branch.contains("KILN_REALM_HTTP_CLOSE_ABORT"));
    assert!(!http_branch.contains("KILN_REALM_OP_CLOSE_RESOURCE"));

    let server_close = source
        .find("static int kiln_c99_node_server_maybe_emit_close(")
        .map(|offset| &source[offset..])
        .expect("server close completion gate is emitted");
    let server_close_end = server_close
        .find("\n}\n")
        .map(|offset| &server_close[..offset])
        .expect("server close completion gate has a body");
    assert!(server_close_end.contains("server->listener != 0"));
    assert!(server_close_end.contains("if (!socket->closed) return KILN_C99_OK;"));
    assert!(server_close_end.contains("server->closed = 1;"));
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
fn method_dispatch_membership_is_generated_from_registry() {
    let unit = emit(&minimal_program()).expect("minimal program emits");
    let (prelude, _) = split(&unit.source);
    assert!(prelude.contains("static uint32_t kiln_c99_method_groups(uint64_t code)"));
    assert!(prelude.contains("case KILN_C99_METHOD_ARRAY_AT: return UINT32_C(2);"));
    assert!(
        prelude
            .contains("case KILN_C99_METHOD_MATH_FLOOR: return UINT32_C(8) | UINT32_C(1048576);")
    );
    assert!(prelude.contains("(groups & KILN_C99_GROUP_GENERIC)"));
    assert!(prelude.contains("(groups & KILN_C99_GROUP_STRING_DISPATCH)"));
    assert!(!prelude.contains("code < KILN_C99_METHOD_ARRAY_AT"));
    assert!(!prelude.contains("code <= KILN_C99_METHOD_ARRAY_AT"));
}

/// A method that belongs to more than one dispatch group cannot be routed by group: whichever
/// group is tested first wins, and that order is a property of how the ifs were written rather
/// than of the call. `setEncoding` is the only such name -- a `net.Socket`, an
/// `http.IncomingMessage` and a `stream.Readable` all own it -- and when group order decided it,
/// every HTTP response's `setEncoding` was answered by the socket handler and threw
/// `is not a Socket`, with every live-request test still green.
///
/// So the generated dispatch must ask the receiver first. This is the cheap guard on that
/// ordering; `native_node_http_client_response_set_encoding_delivers_text_chunks` in
/// `kiln-native` is the one that runs the program.
#[test]
fn set_encoding_is_routed_by_receiver_before_any_dispatch_group() {
    let unit = emit(&minimal_program()).expect("minimal program emits");
    let (prelude, _) = split(&unit.source);
    // The last one, because the prelude forward-declares the dispatcher long before it defines
    // it and the declaration would put the whole runtime inside the window.
    let dispatch = prelude
        .rsplit_once("static int kiln_c99_invoke_method(")
        .expect("the prelude defines the method dispatcher")
        .1;
    assert!(
        dispatch.starts_with("kiln_host *host"),
        "the anchor found a call rather than the definition"
    );
    let routed = dispatch
        .find("code == KILN_C99_METHOD_NODE_READABLE_SET_ENCODING")
        .expect("setEncoding is routed explicitly rather than by group");
    for group in [
        "KILN_C99_GROUP_NODE_NET",
        "KILN_C99_GROUP_NODE_HTTP",
        "KILN_C99_GROUP_NODE_STREAM",
    ] {
        let tested = dispatch
            .find(&format!("(groups & {group})"))
            .unwrap_or_else(|| panic!("{group} is tested in the dispatcher"));
        assert!(
            routed < tested,
            "setEncoding must be routed by receiver before {group} claims it"
        );
    }
    // And the registry must still declare all three, so the entry reads as what it is rather
    // than as a method that happens to live in two of the three handlers.
    assert!(
        prelude.contains(
            "case KILN_C99_METHOD_NODE_READABLE_SET_ENCODING: \
             return UINT32_C(8192) | UINT32_C(2048) | UINT32_C(262144);"
        ),
        "setEncoding must declare its NodeHttp, NodeNet and NodeStream membership"
    );
}

#[test]
fn runtime_prelude_size() {
    // A guard on the half that is not snapshotted verbatim: the surface above would not notice a
    // rewritten function body, and this notices that something moved without pretending to say
    // what.
    //
    // This was an exact `insta` snapshot of the line and byte counts, and an exact count of a file
    // everybody appends to conflicts on *every* merge. It never survived one: the resolution was
    // always to regenerate, because the number carries no meaning a reviewer can check -- nobody
    // has ever looked at "1077518" and known whether it was right. Worse, it never fired alone.
    // Every time it went red, the verbatim body snapshots went red beside it and said what had
    // actually changed, so the exact number contributed a merge conflict and no information.
    //
    // The band is set at roughly +/-9% of the 21,982 lines emitted today, which is tight enough to
    // be worth having: a section that stopped being emitted, or one emitted twice, moves the count
    // by far more than 9%, and a 10% move in either direction (19,784 or 24,180) falls outside.
    //
    // It is loose enough to be nearly free. The prelude grew 33 lines across the last four commits
    // that touched it, so ~8 lines a commit leaves a couple of hundred commits of headroom before
    // anyone edits this -- against a snapshot that conflicted on every single merge. When it does
    // need widening, that is a real change of scale and the right moment for somebody to look.
    //
    // Widened once on 2026-09-01, when the collector (ADR 0041) added its core, a tracer and a
    // finaliser per heap kind, and the string-builder seam: about 700 lines, to 24,704. The band
    // is again roughly +/-9% of that.
    //
    // Widened again on 2026-09-02 for `node:path` and `String.fromCharCode`. The prelude is 27,116
    // lines at that point -- and would have been ~120 more, except that `dirname`, `basename`,
    // `extname` and `relative` call the Rust provider rather than carrying a second hand-written
    // copy of node's algorithms. A ceiling of 27,000 is the one this change would have needed
    // without that; it is worth knowing which of the two numbers is the honest one.
    //
    // Widened once more when the builtin-surface work landed on top of that: `DataView`'s twenty
    // accessors and their handler, the iterator helpers and their lazy pipeline, the change-by-copy
    // array methods and the collection and spread work are about 2,700 lines between them, to
    // Widened once more for the native in-process dynamic evaluator and Ajv schema compilation
    // engine: an AST-free recursive descent lexer, parser, lexical scope stack, and dynamic
    // closure trampoline supporting runtime code generation without external engine dependencies,
    // bringing the prelude to 32,507 lines. The ceiling is widened to 35,500.
    //
    // Widened on 2026-09-05, at 36,507 lines. The NaN-boxing seam (ADR 0053) put a `kiln_val`
    // wrapper in front of most of the generic paths -- about 190 boundary conversions -- and the
    // graph emitter added `kiln_c99_val_call_method`, a method call end to end on machine values.
    // The wrappers are scaffolding: they disappear as the generic paths take `kiln_val` themselves,
    // and this ceiling is expected to come back down with them. 38,000 for now.
    const FLOOR_LINES: usize = 27_100;
    const CEILING_LINES: usize = 38_000;

    let unit = emit(&minimal_program()).expect("minimal program emits");
    let (prelude, _) = split(&unit.source);
    let lines = prelude.lines().count();
    assert!(
        (FLOOR_LINES..=CEILING_LINES).contains(&lines),
        "the runtime prelude is {lines} lines, outside the expected {FLOOR_LINES}..={CEILING_LINES}.\
         \n\nThat is a change of scale rather than an ordinary edit: either a large block stopped \
         being emitted, or one is being emitted repeatedly. Check the emitted unit before widening \
         this band -- the band is meant to move when the prelude legitimately grows, but only after \
         somebody has looked at why."
    );
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
        source: "".into(),
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
        strictness: kiln_ir::Strictness::default(),
        ..Default::default()
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
    let expected_path_source = by_content
        .source
        .replace(&by_content.symbol, &by_path.symbol)
        .replace(&format!("\"{}\"", by_path.symbol), "\"src/app/main.js\"");
    assert_eq!(
        by_path.source, expected_path_source,
        "the two spellings must differ only in the exported symbol and diagnostic site"
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

#[test]
fn fused_property_read_and_write_emission() {
    let p = program(
        vec![
            Constant::String("x".to_string()),
            Constant::String("y".to_string()),
            Constant::String("z".to_string()),
            number(42.0),
        ],
        vec![
            // let b = a.x; (LoadLocal(0), GetProperty(0), StoreLocal(1))
            Instruction::LoadLocal(0),
            Instruction::GetProperty(0),
            Instruction::StoreLocal(1),
            // a.y = b; (LoadLocal(0), LoadLocal(1), SetProperty(1), Pop)
            Instruction::LoadLocal(0),
            Instruction::LoadLocal(1),
            Instruction::SetProperty(1),
            Instruction::Pop,
            // a.z = 42; (LoadLocal(0), Const(3), SetPropertyStrict(2), Pop)
            Instruction::LoadLocal(0),
            Instruction::Const(3),
            Instruction::SetPropertyStrict {
                name: 2,
                strict: true,
            },
            Instruction::Pop,
            Instruction::Return,
        ],
        2,
    );
    let unit = emit(&p).expect("fused property program emits");
    let b = body(&unit);
    // The graph carries the object and the value as SSA locals straight into the helpers: no
    // operand stack exists in the body for a read or a write to go through.
    assert!(b.contains("kiln_c99_val_read_property(host, v"));
    assert!(b.contains("kiln_c99_val_write_property(host, v"));
    assert!(
        !b.contains("stack["),
        "a graph body has no operand stack:\n{b}"
    );
}

#[test]
fn fused_call_method_statement_and_assignment() {
    let p = program(
        vec![
            Constant::String("foo".to_string()),
            Constant::String("bar".to_string()),
            Constant::String("baz".to_string()),
        ],
        vec![
            Instruction::LoadLocal(0),
            Instruction::CallMethod { name: 0, argc: 0 },
            Instruction::Pop,
            Instruction::LoadLocal(0),
            Instruction::CallMethod { name: 1, argc: 0 },
            Instruction::StoreLocal(1),
            Instruction::LoadLocal(0),
            Instruction::CallMethod { name: 2, argc: 0 },
            Instruction::Dup,
            Instruction::StoreLocal(2),
            Instruction::Pop,
            Instruction::Return,
        ],
        3,
    );
    let unit = emit(&p).expect("fused call_method program emits");
    let b = body(&unit);
    // Three method calls, each on its receiver value, and no operand stack between them: the
    // result a statement discards is simply never read, and one assigned is the local's value.
    assert_eq!(
        b.matches("kiln_c99_val_call_method(host, v").count(),
        3,
        "{b}"
    );
    assert!(
        !b.contains("stack["),
        "a graph body has no operand stack:\n{b}"
    );
}

#[test]
fn in_place_induction_variable_loop_emission() {
    let p = program(
        vec![
            Constant::Number(1.0f64.to_bits()),
            Constant::Number(2.0f64.to_bits()),
        ],
        vec![
            // i = i + 1
            Instruction::LoadLocal(0),
            Instruction::Const(0),
            Instruction::Binary(BinaryOp::Add),
            Instruction::StoreLocal(0),
            // i = i - 2
            Instruction::LoadLocal(0),
            Instruction::Const(1),
            Instruction::Binary(BinaryOp::Subtract),
            Instruction::StoreLocal(0),
            Instruction::Return,
        ],
        1,
    );
    let unit = emit(&p).expect("induction variable program emits");
    let b = body(&unit);
    // Each step is one guarded double operation between SSA values; nothing is stored to and
    // re-read from a local between the two steps.
    assert!(b.contains("kiln_c99_val_from_double(a + b)"), "{b}");
    assert!(b.contains("kiln_c99_val_from_double(a - b)"), "{b}");
    assert!(!b.contains("locals[0] ="), "{b}");
    assert!(
        !b.contains("stack["),
        "a graph body has no operand stack:\n{b}"
    );
}

#[test]
fn polymorphic_inline_cache_prelude_surface() {
    let unit = emit(&minimal_program()).expect("minimal program emits");
    let (prelude, _) = split(&unit.source);
    // Asserts 4-shape polymorphic unrolling exists in read_property and write_property
    assert!(prelude.contains("for (p = 0; p < site->pic_count; ++p)"));
    assert!(prelude.contains("object->shape == site->entries[p].shape"));
}

#[test]
fn nan_boxed_kiln_val_prelude_surface() {
    let unit = emit(&minimal_program()).expect("minimal program emits");
    let (prelude, _) = split(&unit.source);
    assert!(prelude.contains("typedef uint64_t kiln_val;"));
    assert!(prelude.contains("KILN_VAL_IS_NUMBER"));
    assert!(prelude.contains("kiln_val_from_boundary"));
    assert!(prelude.contains("kiln_val_to_boundary"));
}

#[test]
fn fused_computed_read_and_write_emission() {
    let p = program(
        vec![
            Constant::Number(0f64.to_bits()),
            Constant::Number(42f64.to_bits()),
        ],
        vec![
            // x = a[i]; (LoadLocal(0), LoadLocal(1), GetComputed, StoreLocal(2))
            Instruction::LoadLocal(0),
            Instruction::LoadLocal(1),
            Instruction::GetComputed,
            Instruction::StoreLocal(2),
            // a[i] = x; (LoadLocal(0), LoadLocal(1), LoadLocal(2), SetComputed, Pop)
            Instruction::LoadLocal(0),
            Instruction::LoadLocal(1),
            Instruction::LoadLocal(2),
            Instruction::SetComputed,
            Instruction::Pop,
            // a[i] = 42; (LoadLocal(0), LoadLocal(1), Const(1), SetComputedStrict, Pop)
            Instruction::LoadLocal(0),
            Instruction::LoadLocal(1),
            Instruction::Const(1),
            Instruction::SetComputedStrict { strict: true },
            Instruction::Pop,
            Instruction::Return,
        ],
        3,
    );
    let unit = emit(&p).expect("fused computed program emits");
    let b = body(&unit);
    assert!(b.contains("kiln_c99_val_read_element(host, v"));
    assert!(b.contains("kiln_c99_val_write_element(host, v"));
    assert!(
        !b.contains("stack["),
        "a graph body has no operand stack:\n{b}"
    );
}

#[test]
fn method_inline_cache_prelude_surface() {
    let unit = emit(&minimal_program()).expect("minimal program emits");
    let (prelude, _) = split(&unit.source);
    assert!(prelude.contains("uint8_t cached_tag;"));
    assert!(prelude.contains("uint8_t has_cached_method;"));
    assert!(prelude.contains("uint64_t cached_method;"));
    assert!(prelude.contains("site->has_cached_method"));
    assert!(prelude.contains("kiln_c99_invoke_method(host, &call_result, method.bits, self_value"));
}
