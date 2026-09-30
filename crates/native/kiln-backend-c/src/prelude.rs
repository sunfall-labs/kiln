use super::*;

#[derive(Clone, Copy)]
pub(crate) struct MethodId {
    pub(crate) name: &'static str,
    pub(crate) value: u16,
    pub(crate) groups: u32,
    pub(crate) numeric_arguments: u32,
    pub(crate) textual_arguments: u32,
}

const METHOD_LENGTH_UNSPECIFIED: u32 = u32::MAX;

/// Native dispatch IDs whose public owner/key cannot be recovered from the table-driven builtin
/// receiver rows. The member's arity still comes from `kiln_builtin_surface`; this table only
/// connects the C dispatch ID to that public identity.
const NATIVE_METHOD_SURFACE_BINDINGS: &[(&str, &str, &str)] = &[
    ("KILN_C99_METHOD_STRING_INDEX_OF", "String.prototype", "indexOf"),
    ("KILN_C99_METHOD_NODE_FS_READ_FILE_SYNC", "node:fs", "readFileSync"),
];

const fn bytes_contain(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.len() == 0 || needle.len() > haystack.len() {
        return false;
    }
    let mut start = 0;
    while start + needle.len() <= haystack.len() {
        let end = start + needle.len();
        let before_is_identifier = start > 0 &&
            ((haystack[start - 1] >= b'a' && haystack[start - 1] <= b'z') ||
             (haystack[start - 1] >= b'A' && haystack[start - 1] <= b'Z') ||
             (haystack[start - 1] >= b'0' && haystack[start - 1] <= b'9') ||
             haystack[start - 1] == b'_');
        let after_is_identifier = end < haystack.len() &&
            ((haystack[end] >= b'a' && haystack[end] <= b'z') ||
             (haystack[end] >= b'A' && haystack[end] <= b'Z') ||
             (haystack[end] >= b'0' && haystack[end] <= b'9') ||
             haystack[end] == b'_');
        if !before_is_identifier && !after_is_identifier &&
            bytes_match_at(haystack, start, needle) {
            return true;
        }
        start += 1;
    }
    false
}

fn method_declared_length(method: &MethodId) -> u32 {
    for (native_id, owner, member) in NATIVE_METHOD_SURFACE_BINDINGS {
        if *native_id == method.name {
            if let Some(length) = kiln_builtin_surface::native_function_length(owner, member) {
                return length as u32;
            }
        }
    }
    let mut length = None;
    let mut index = 0;
    while index < BUILTIN_SURFACE.len() {
        let member = BUILTIN_SURFACE[index];
        if bytes_contain(member.answers.as_bytes(), method.name.as_bytes()) {
            if let Some(candidate) = kiln_builtin_surface::native_function_length(
                member.owner,
                member.name,
            ) {
                if let Some(previous) = length {
                    if previous != candidate {
                        return METHOD_LENGTH_UNSPECIFIED;
                    }
                } else {
                    length = Some(candidate);
                }
            }
        }
        index += 1;
    }
    match length {
        Some(length) => length as u32,
        None => METHOD_LENGTH_UNSPECIFIED,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum NativePrototypeSurface {
    EventEmitter,
    Readable,
    Writable,
    ServerResponse,
}

#[derive(Clone, Copy)]
struct NativeConstructor {
    method: u16,
    parent: Option<u16>,
    surface: NativePrototypeSurface,
}

const fn validate_native_constructors(constructors: &[NativeConstructor], methods: &[MethodId]) {
    let mut index = 0;
    while index < constructors.len() {
        let constructor = constructors[index];
        let mut method_found = false;
        let mut method_index = 0;
        while method_index < methods.len() {
            if methods[method_index].value == constructor.method {
                method_found = true;
                break;
            }
            method_index += 1;
        }
        assert!(
            method_found,
            "native constructor is not a registered method"
        );

        let mut other = index + 1;
        while other < constructors.len() {
            assert!(
                constructors[other].method != constructor.method,
                "duplicate native constructor"
            );
            other += 1;
        }

        if let Some(parent) = constructor.parent {
            let mut parent_index = 0;
            let mut parent_found = false;
            while parent_index < index {
                if constructors[parent_index].method == parent {
                    parent_found = true;
                    break;
                }
                parent_index += 1;
            }
            assert!(
                parent_found,
                "native constructor parent must be declared before its child"
            );
        }
        index += 1;
    }
}

pub(crate) fn render_native_constructor_table() -> String {
    let mut table =
        String::from("static const kiln_native_constructor kiln_c99_native_constructors[] = {\n");
    for constructor in NATIVE_CONSTRUCTORS {
        let parent = constructor.parent.unwrap_or(0);
        let surface = match constructor.surface {
            NativePrototypeSurface::EventEmitter => "KILN_C99_NATIVE_EVENT_EMITTER",
            NativePrototypeSurface::Readable => "KILN_C99_NATIVE_READABLE",
            NativePrototypeSurface::Writable => "KILN_C99_NATIVE_WRITABLE",
            NativePrototypeSurface::ServerResponse => "KILN_C99_NATIVE_SERVER_RESPONSE",
        };
        use std::fmt::Write as _;
        writeln!(
            table,
            "    {{ UINT64_C({}), UINT64_C({parent}), {surface} }},",
            constructor.method
        )
        .expect("writing to String cannot fail");
    }
    table.push_str("};\n");
    table
}

fn render_native_method_length_rows() -> String {
    static ROWS: std::sync::LazyLock<String> =
        std::sync::LazyLock::new(compute_native_method_length_rows);
    ROWS.clone()
}

fn compute_native_method_length_rows() -> String {
    use std::fmt::Write as _;
    let mut rows = String::new();
    for method in METHOD_IDS {
        let length = method_declared_length(method);
        if length != METHOD_LENGTH_UNSPECIFIED {
            writeln!(
                rows,
                "    case UINT64_C({}): return UINT32_C({});",
                method.value,
                length
            )
            .expect("writing to a String cannot fail");
        }
    }
    rows
}

macro_rules! native_constructor {
    ($method:ident, $parent:expr, $surface:ident) => {
        NativeConstructor {
            method: method_value!($method),
            parent: $parent,
            surface: NativePrototypeSurface::$surface,
        }
    };
}

macro_rules! method_group_bit {
    (Direct) => {
        1u32 << 0
    };
    (ArrayLookup) => {
        1u32 << 1
    };
    (StringDispatch) => {
        1u32 << 2
    };
    (Generic) => {
        1u32 << 3
    };
    (Temporal) => {
        1u32 << 4
    };
    (TemporalFrom) => {
        1u32 << 5
    };
    (TemporalCompare) => {
        1u32 << 6
    };
    (TemporalNowRead) => {
        1u32 << 7
    };
    (TemporalOp) => {
        1u32 << 8
    };
    (DateSetter) => {
        1u32 << 9
    };
    (NodeEvent) => {
        1u32 << 10
    };
    (NodeNet) => {
        1u32 << 11
    };
    (NodeServer) => {
        1u32 << 12
    };
    (NodeHttp) => {
        1u32 << 13
    };
    (NodeResponse) => {
        1u32 << 14
    };
    (Buffer) => {
        1u32 << 15
    };
    (NodeUtil) => {
        1u32 << 16
    };
    (NodeDiagnostics) => {
        1u32 << 17
    };
    (NodeStream) => {
        1u32 << 18
    };
    (NodePath) => {
        1u32 << 19
    };
    (MathNumber) => {
        1u32 << 20
    };
}

macro_rules! method_group_bits {
    ($first:ident $(, $rest:ident)*) => {
        method_group_bit!($first) $(| method_group_bit!($rest))*
    };
}

macro_rules! method_group_c_bit {
    (Direct) => {
        "UINT32_C(1)"
    };
    (ArrayLookup) => {
        "UINT32_C(2)"
    };
    (StringDispatch) => {
        "UINT32_C(4)"
    };
    (Generic) => {
        "UINT32_C(8)"
    };
    (Temporal) => {
        "UINT32_C(16)"
    };
    (TemporalFrom) => {
        "UINT32_C(32)"
    };
    (TemporalCompare) => {
        "UINT32_C(64)"
    };
    (TemporalNowRead) => {
        "UINT32_C(128)"
    };
    (TemporalOp) => {
        "UINT32_C(256)"
    };
    (DateSetter) => {
        "UINT32_C(512)"
    };
    (NodeEvent) => {
        "UINT32_C(1024)"
    };
    (NodeNet) => {
        "UINT32_C(2048)"
    };
    (NodeServer) => {
        "UINT32_C(4096)"
    };
    (NodeHttp) => {
        "UINT32_C(8192)"
    };
    (NodeResponse) => {
        "UINT32_C(16384)"
    };
    (Buffer) => {
        "UINT32_C(32768)"
    };
    (NodeUtil) => {
        "UINT32_C(65536)"
    };
    (NodeDiagnostics) => {
        "UINT32_C(131072)"
    };
    (NodeStream) => {
        "UINT32_C(262144)"
    };
    (NodePath) => {
        "UINT32_C(524288)"
    };
    (MathNumber) => {
        "UINT32_C(1048576)"
    };
}

macro_rules! method_group_c_bits {
    ($first:ident $(, $rest:ident)*) => {
        concat!(method_group_c_bit!($first) $(, " | ", method_group_c_bit!($rest))* )
    };
}

/* Argument coercion is part of the method registry, alongside dispatch groups.  Keeping the
 * masks here lets the C prelude consume one generated descriptor instead of maintaining three
 * independent switches (groups, numeric arguments, and textual arguments).  The interpreter has
 * a richer Native-level table; these masks deliberately describe only the coercions implemented
 * by this C backend. */
macro_rules! method_metadata_case {
    /* The endpoint is an alias used only by the Rust range assertions.  Emitting it would create
     * a duplicate C switch case for Instant's final operation. */
    (KILN_C99_METHOD_TEMPORAL_LAST, $($group:ident),* ; $numeric:literal, $textual:literal) => { "" };
    ($name:ident, $first_group:ident $(, $group:ident)* ; $numeric:literal, $textual:literal) => {
        concat!(
            "    case ", stringify!($name), ": return (kiln_c99_method_metadata){",
            method_group_c_bits!($first_group $(, $group)*), ", ",
            "UINT32_C(", stringify!($numeric), "), UINT32_C(", stringify!($textual), ")};\n"
        )
    };
}

macro_rules! method_ids {
    ($(($name:ident, $value:literal, $first_group:ident $(, $group:ident)* ; $numeric:literal, $textual:literal)),* $(,)?) => {
        macro_rules! emit_method_define {
            $(($name) => {
                concat!("#define ", stringify!($name), " ", stringify!($value), "\n")
            };)*
        }

        macro_rules! method_value {
            $(($name) => { $value };)*
        }

        pub(crate) const METHOD_IDS: &[MethodId] = &[
            $(MethodId {
                name: stringify!($name),
                value: $value,
                groups: method_group_bits!($first_group $(, $group)*),
                numeric_arguments: $numeric,
                textual_arguments: $textual,
            }),*
        ];

        macro_rules! emit_method_metadata_cases {
            () => {
                concat!($(method_metadata_case!($name, $first_group $(, $group)* ; $numeric, $textual)),*)
            };
        }

        macro_rules! emit_method_code_list {
            () => { concat!($(stringify!($name), ", "),*) };
        }
    };
}

const fn method_name_is(name: &str, expected: &str) -> bool {
    let name = name.as_bytes();
    let expected = expected.as_bytes();
    if name.len() != expected.len() {
        return false;
    }
    let mut index = 0;
    while index < name.len() {
        if name[index] != expected[index] {
            return false;
        }
        index += 1;
    }
    true
}

const fn method_name_starts_with(name: &str, prefix: &str) -> bool {
    let name = name.as_bytes();
    let prefix = prefix.as_bytes();
    if name.len() < prefix.len() {
        return false;
    }
    let mut index = 0;
    while index < prefix.len() {
        if name[index] != prefix[index] {
            return false;
        }
        index += 1;
    }
    true
}

const fn method_name_is_temporal_last(name: &str) -> bool {
    method_name_is(name, "KILN_C99_METHOD_TEMPORAL_LAST")
}

const fn method_value(ids: &[MethodId], name: &str) -> u16 {
    let mut index = 0;
    while index < ids.len() {
        if method_name_is(ids[index].name, name) {
            return ids[index].value;
        }
        index += 1;
    }
    panic!("method ID is missing from the registry");
}

const fn assert_method_immediately_after(ids: &[MethodId], previous: &str, next: &str) {
    let previous = method_value(ids, previous);
    assert!(
        previous < u16::MAX && previous + 1 == method_value(ids, next),
        "Temporal method blocks must be ordered and adjacent"
    );
}

const fn validate_method_ids(ids: &[MethodId]) {
    // `kiln_c99_invoke_method` returns immediately from each of these adapter handlers. A method
    // in two such groups is therefore routed by source order rather than receiver shape, which is
    // never a meaningful contract. Receiver-specific secondary routing (for example raw-socket
    // `setEncoding`) must remain an explicit check in the generated dispatcher.
    let terminal_adapter_groups = method_group_bit!(NodeEvent)
        | method_group_bit!(NodeNet)
        | method_group_bit!(NodeHttp)
        | method_group_bit!(NodeResponse)
        | method_group_bit!(Buffer)
        | method_group_bit!(NodeUtil)
        | method_group_bit!(NodeDiagnostics)
        | method_group_bit!(NodeStream)
        | method_group_bit!(NodePath);
    let mut index = 0;
    while index < ids.len() {
        assert!(ids[index].value > 0, "method IDs must be positive");
        assert!(
            ids[index].groups != 0,
            "every method must have a dispatch category"
        );
        assert!(
            ids[index].numeric_arguments & ids[index].textual_arguments == 0,
            "numeric and textual coercion masks must not overlap"
        );
        let terminal_groups = ids[index].groups & terminal_adapter_groups;
        let receiver_routed_set_encoding = method_name_is(
            ids[index].name,
            "KILN_C99_METHOD_NODE_READABLE_SET_ENCODING",
        ) && terminal_groups
            == method_group_bit!(NodeNet)
                | method_group_bit!(NodeHttp)
                | method_group_bit!(NodeStream);
        assert!(
            terminal_groups == 0
                || terminal_groups & (terminal_groups - 1) == 0
                || receiver_routed_set_encoding,
            "only explicitly receiver-routed setEncoding may span terminal adapter groups"
        );
        let mut other = index + 1;
        while other < ids.len() {
            assert!(
                !method_name_is(ids[index].name, ids[other].name),
                "method names must be unique"
            );
            if ids[index].value == ids[other].value {
                let alias = if method_name_is_temporal_last(ids[index].name) {
                    method_name_is(
                        ids[other].name,
                        "KILN_C99_METHOD_TEMPORAL_INSTANT_FROM_EPOCH_NANOSECONDS",
                    )
                } else if method_name_is_temporal_last(ids[other].name) {
                    method_name_is(
                        ids[index].name,
                        "KILN_C99_METHOD_TEMPORAL_INSTANT_FROM_EPOCH_NANOSECONDS",
                    )
                } else {
                    false
                };
                assert!(
                    alias
                        && method_name_starts_with(ids[index].name, "KILN_C99_METHOD_TEMPORAL_")
                        && method_name_starts_with(ids[other].name, "KILN_C99_METHOD_TEMPORAL_"),
                    "method IDs must be unique except for the Instant nanoseconds alias"
                );
            }
            other += 1;
        }
        index += 1;
    }

    // These are range-dispatched, so their endpoint constants must retain the width used by the
    // generated call path. The individual operation IDs between those endpoints are intentionally
    // not declared: the runtime derives their kind from the code itself.
    assert!(
        method_value(ids, "KILN_C99_METHOD_TEMPORAL_FROM_LAST")
            - method_value(ids, "KILN_C99_METHOD_TEMPORAL_FROM")
            == 7,
        "Temporal.from range must contain eight kinds"
    );
    assert!(
        method_value(ids, "KILN_C99_METHOD_TEMPORAL_COMPARE_LAST")
            - method_value(ids, "KILN_C99_METHOD_TEMPORAL_COMPARE")
            == 7,
        "Temporal.compare range must contain eight kinds"
    );
    assert!(
        method_value(ids, "KILN_C99_METHOD_TEMPORAL_NOW_READ_LAST")
            - method_value(ids, "KILN_C99_METHOD_TEMPORAL_NOW_READ")
            == 7,
        "Temporal.Now range must contain eight kinds"
    );
    assert!(
        method_value(ids, "KILN_C99_METHOD_TEMPORAL_OP_LAST")
            - method_value(ids, "KILN_C99_METHOD_TEMPORAL_OP")
            == 22,
        "Temporal operation range must contain 23 operations"
    );
    assert_method_immediately_after(
        ids,
        "KILN_C99_METHOD_TEMPORAL_FROM_LAST",
        "KILN_C99_METHOD_TEMPORAL_COMPARE",
    );
    assert_method_immediately_after(
        ids,
        "KILN_C99_METHOD_TEMPORAL_COMPARE_LAST",
        "KILN_C99_METHOD_TEMPORAL_TO_STRING",
    );
    assert_method_immediately_after(
        ids,
        "KILN_C99_METHOD_TEMPORAL_TO_STRING",
        "KILN_C99_METHOD_TEMPORAL_TO_JSON",
    );
    assert_method_immediately_after(
        ids,
        "KILN_C99_METHOD_TEMPORAL_TO_JSON",
        "KILN_C99_METHOD_TEMPORAL_VALUE_OF",
    );
    assert_method_immediately_after(
        ids,
        "KILN_C99_METHOD_TEMPORAL_VALUE_OF",
        "KILN_C99_METHOD_TEMPORAL_NOW_TIME_ZONE_ID",
    );
    assert_method_immediately_after(
        ids,
        "KILN_C99_METHOD_TEMPORAL_NOW_TIME_ZONE_ID",
        "KILN_C99_METHOD_TEMPORAL_NOW_READ",
    );
    assert_method_immediately_after(
        ids,
        "KILN_C99_METHOD_TEMPORAL_NOW_READ_LAST",
        "KILN_C99_METHOD_TEMPORAL_OP",
    );
    assert_method_immediately_after(
        ids,
        "KILN_C99_METHOD_TEMPORAL_OP_LAST",
        "KILN_C99_METHOD_TEMPORAL_INSTANT_FROM_EPOCH_MILLISECONDS",
    );
    assert_method_immediately_after(
        ids,
        "KILN_C99_METHOD_TEMPORAL_INSTANT_FROM_EPOCH_MILLISECONDS",
        "KILN_C99_METHOD_TEMPORAL_INSTANT_FROM_EPOCH_NANOSECONDS",
    );

    let mut temporal_low = u16::MAX;
    let mut temporal_high = 0;
    let mut index = 0;
    while index < ids.len() {
        if method_name_starts_with(ids[index].name, "KILN_C99_METHOD_TEMPORAL_")
            && !method_name_is_temporal_last(ids[index].name)
        {
            if ids[index].value < temporal_low {
                temporal_low = ids[index].value;
            }
            if ids[index].value > temporal_high {
                temporal_high = ids[index].value;
            }
        }
        index += 1;
    }
    assert!(temporal_low < temporal_high, "Temporal methods must exist");
    assert!(
        temporal_low == method_value(ids, "KILN_C99_METHOD_TEMPORAL_FROM")
            && temporal_high
                == method_value(
                    ids,
                    "KILN_C99_METHOD_TEMPORAL_INSTANT_FROM_EPOCH_NANOSECONDS",
                ),
        "Temporal range must begin at FROM and end at the final Instant constructor"
    );
    assert!(
        method_value(ids, "KILN_C99_METHOD_TEMPORAL_LAST") == temporal_high,
        "TEMPORAL_LAST must alias the highest Temporal method"
    );
    index = 0;
    while index < ids.len() {
        if !method_name_starts_with(ids[index].name, "KILN_C99_METHOD_TEMPORAL_") {
            assert!(
                ids[index].value < temporal_low || ids[index].value > temporal_high,
                "non-Temporal method lies inside a Temporal dispatch range"
            );
        }
        index += 1;
    }

    // IDs at and above the first Temporal.Instant constructor are append-only. This catches a
    // merge that shifts the equality-dispatched tail into a hole or reuses an old value.
    const APPEND_ONLY_BASE: u16 = 336;
    let mut tail_high = 0;
    index = 0;
    while index < ids.len() {
        if ids[index].value >= APPEND_ONLY_BASE && ids[index].value > tail_high {
            tail_high = ids[index].value;
        }
        index += 1;
    }
    assert!(
        tail_high >= APPEND_ONLY_BASE,
        "append-only method tail must exist"
    );
    // Numbers handed out in advance, and therefore allowed to be absent for as long as the
    // branch holding them is open.
    //
    // Append-at-max is the right rule while one person at a time is adding codes. It stops being
    // possible the moment several branches are open at once: the numbers have to be reserved up
    // front instead, and whoever holds the *lower* reservation then lands into a hole rather
    // than onto the end. Refusing the hole would mean the branches could only land in one order,
    // which is not a property anybody can arrange.
    //
    // 442..=449 is such a reservation, held by a branch that had not landed when the `DataView`
    // block took 450..=469. The row goes when it lands -- a reserved ID that has since been
    // declared is simply an ID, and nothing below objects to one.
    const RESERVED_METHOD_IDS: [u16; 8] = [442, 443, 444, 445, 446, 447, 448, 449];
    let mut value = APPEND_ONLY_BASE;
    while value <= tail_high {
        let mut found = false;
        index = 0;
        while index < ids.len() {
            if ids[index].value == value {
                found = true;
                break;
            }
            index += 1;
        }
        let mut reserved = 0;
        while !found && reserved < RESERVED_METHOD_IDS.len() {
            if RESERVED_METHOD_IDS[reserved] == value {
                found = true;
            }
            reserved += 1;
        }
        assert!(
            found,
            "append-only method IDs must have no gaps, and this one is not a reserved range"
        );
        value += 1;
    }
}

method_ids! {
    (KILN_C99_METHOD_ARRAY_AT, 200, ArrayLookup; 0x1, 0),
    (KILN_C99_METHOD_ARRAY_FIND, 201, ArrayLookup; 0, 0),
    (KILN_C99_METHOD_ARRAY_FIND_INDEX, 202, ArrayLookup; 0, 0),
    (KILN_C99_METHOD_ARRAY_SOME, 203, ArrayLookup; 0, 0),
    (KILN_C99_METHOD_ARRAY_EVERY, 204, ArrayLookup; 0, 0),
    (KILN_C99_METHOD_ARRAY_SORT, 205, ArrayLookup; 0, 0),
    (KILN_C99_METHOD_ARRAY_FLAT, 206, ArrayLookup; 0, 0),
    (KILN_C99_METHOD_ARRAY_ENTRIES, 207, ArrayLookup; 0, 0),
    (KILN_C99_METHOD_ARRAY_KEYS, 208, ArrayLookup; 0, 0),
    (KILN_C99_METHOD_ARRAY_REVERSE, 209, ArrayLookup; 0, 0),
    (KILN_C99_METHOD_ARRAY_SHIFT, 210, ArrayLookup; 0, 0),
    (KILN_C99_METHOD_ARRAY_UNSHIFT, 211, ArrayLookup; 0, 0),
    (KILN_C99_METHOD_ARRAY_FILL, 212, ArrayLookup; 0x6, 0),
    (KILN_C99_METHOD_ARRAY_LAST_INDEX_OF, 213, ArrayLookup; 0x2, 0),
    (KILN_C99_METHOD_ARRAY_SPLICE, 214, ArrayLookup; 0x3, 0),
    (KILN_C99_METHOD_ARRAY_COPY_WITHIN, 215, ArrayLookup; 0x7, 0),
    (KILN_C99_METHOD_ARRAY_TO_STRING, 216, ArrayLookup; 0, 0),
    (KILN_C99_METHOD_ARRAY_FLAT_MAP, 217, ArrayLookup; 0, 0),
    (KILN_C99_METHOD_ARRAY_FIND_LAST, 218, ArrayLookup; 0, 0),
    (KILN_C99_METHOD_ARRAY_FIND_LAST_INDEX, 219, ArrayLookup; 0, 0),
    (KILN_C99_METHOD_ARRAY_REDUCE_RIGHT, 220, ArrayLookup; 0, 0),
    /* The four ES2023 change-by-copy methods, each the counterpart of one that mutates: `with`
     * to an index assignment, `toReversed` to `reverse`, `toSorted` to `sort`, `toSpliced` to
     * `splice`. They sit beside the array block they belong to, but their codes come off the
     * append-only tail rather than continuing 220: the tail is the one place a change can add to
     * without knowing what else is landing beside it, which is what it is for. Group membership
     * is what puts a code on an array, not numeric adjacency -- and that is the property that
     * makes appending anywhere cheap. */
    (KILN_C99_METHOD_ARRAY_TO_SORTED, 443, ArrayLookup; 0, 0),
    (KILN_C99_METHOD_ARRAY_TO_REVERSED, 444, ArrayLookup; 0, 0),
    (KILN_C99_METHOD_ARRAY_TO_SPLICED, 445, ArrayLookup; 0x3, 0),
    (KILN_C99_METHOD_ARRAY_WITH, 446, ArrayLookup; 0x1, 0),
    (KILN_C99_METHOD_STRING_CODE_POINT_AT, 240, StringDispatch; 0x1, 0),
    (KILN_C99_METHOD_STRING_SUBSTR, 241, StringDispatch; 0x3, 0),
    (KILN_C99_METHOD_STRING_LOCALE_COMPARE, 242, StringDispatch; 0, 0),
    (KILN_C99_METHOD_STRING_NORMALIZE, 243, StringDispatch; 0, 0),
    (KILN_C99_METHOD_STRING_TO_STRING, 244, StringDispatch; 0, 0),
    (KILN_C99_METHOD_STRING_VALUE_OF, 245, StringDispatch; 0, 0),
    (KILN_C99_METHOD_STRING_MATCH, 246, StringDispatch; 0, 0),
    (KILN_C99_METHOD_STRING_MATCH_ALL, 247, StringDispatch; 0, 0),
    (KILN_C99_METHOD_STRING_SEARCH, 248, StringDispatch; 0, 0),
    (KILN_C99_METHOD_ERROR_TO_STRING, 249, StringDispatch; 0, 0),
    (KILN_C99_METHOD_PUSH, 1, ArrayLookup; 0, 0),
    (KILN_C99_METHOD_POP, 2, ArrayLookup; 0, 0),
    (KILN_C99_METHOD_JOIN, 3, ArrayLookup; 0, 0),
    (KILN_C99_METHOD_INDEX_OF, 4, ArrayLookup; 0x2, 0),
    (KILN_C99_METHOD_INCLUDES, 5, ArrayLookup; 0x2, 0),
    (KILN_C99_METHOD_SLICE, 6, ArrayLookup; 0x3, 0),
    (KILN_C99_METHOD_CONCAT, 7, ArrayLookup; 0, 0),
    (KILN_C99_METHOD_MAP, 8, ArrayLookup; 0, 0),
    (KILN_C99_METHOD_FILTER, 9, ArrayLookup; 0, 0),
    (KILN_C99_METHOD_FOR_EACH, 10, ArrayLookup; 0, 0),
    (KILN_C99_METHOD_REDUCE, 11, ArrayLookup; 0, 0),
    (KILN_C99_METHOD_STRING_SLICE, 12, StringDispatch; 0x3, 0),
    (KILN_C99_METHOD_SUBSTRING, 13, StringDispatch; 0x3, 0),
    (KILN_C99_METHOD_STRING_INDEX_OF, 14, StringDispatch; 0x2, 0),
    (KILN_C99_METHOD_LAST_INDEX_OF, 15, StringDispatch; 0x2, 0),
    (KILN_C99_METHOD_STRING_INCLUDES, 16, StringDispatch; 0x2, 0),
    (KILN_C99_METHOD_STARTS_WITH, 17, StringDispatch; 0x2, 0),
    (KILN_C99_METHOD_ENDS_WITH, 18, StringDispatch; 0x2, 0),
    (KILN_C99_METHOD_CHAR_AT, 19, StringDispatch; 0x1, 0),
    (KILN_C99_METHOD_CHAR_CODE_AT, 20, StringDispatch; 0x1, 0),
    (KILN_C99_METHOD_STRING_AT, 21, StringDispatch; 0x1, 0),
    (KILN_C99_METHOD_TRIM, 22, StringDispatch; 0, 0),
    (KILN_C99_METHOD_TRIM_START, 23, StringDispatch; 0, 0),
    (KILN_C99_METHOD_TRIM_END, 24, StringDispatch; 0, 0),
    (KILN_C99_METHOD_SPLIT, 25, StringDispatch; 0, 0),
    (KILN_C99_METHOD_REPEAT, 26, StringDispatch; 0x1, 0),
    (KILN_C99_METHOD_PAD_START, 27, StringDispatch; 0x1, 0),
    (KILN_C99_METHOD_PAD_END, 28, StringDispatch; 0x1, 0),
    (KILN_C99_METHOD_TO_UPPER_CASE, 29, StringDispatch; 0, 0),
    (KILN_C99_METHOD_TO_LOWER_CASE, 30, StringDispatch; 0, 0),
    (KILN_C99_METHOD_REPLACE, 31, StringDispatch; 0, 0),
    (KILN_C99_METHOD_REPLACE_ALL, 32, StringDispatch; 0, 0),
    (KILN_C99_METHOD_STRING_CONCAT, 33, StringDispatch; 0, 0),
    (KILN_C99_METHOD_MATH_FLOOR, 34, Generic, MathNumber; 0, 0),
    (KILN_C99_METHOD_MATH_CEIL, 35, Generic, MathNumber; 0, 0),
    (KILN_C99_METHOD_MATH_ROUND, 36, Generic, MathNumber; 0, 0),
    (KILN_C99_METHOD_MATH_TRUNC, 37, Generic, MathNumber; 0, 0),
    (KILN_C99_METHOD_MATH_ABS, 38, Generic, MathNumber; 0, 0),
    (KILN_C99_METHOD_MATH_SIGN, 39, Generic, MathNumber; 0, 0),
    (KILN_C99_METHOD_MATH_SQRT, 40, Generic, MathNumber; 0, 0),
    (KILN_C99_METHOD_MATH_CBRT, 41, Generic, MathNumber; 0, 0),
    (KILN_C99_METHOD_MATH_POW, 42, Generic, MathNumber; 0, 0),
    (KILN_C99_METHOD_MATH_MIN, 43, Generic; 0, 0),
    (KILN_C99_METHOD_MATH_MAX, 44, Generic; 0, 0),
    (KILN_C99_METHOD_MATH_LOG, 45, Generic, MathNumber; 0, 0),
    (KILN_C99_METHOD_MATH_LOG2, 46, Generic, MathNumber; 0, 0),
    (KILN_C99_METHOD_MATH_LOG10, 47, Generic, MathNumber; 0, 0),
    (KILN_C99_METHOD_MATH_EXP, 48, Generic, MathNumber; 0, 0),
    (KILN_C99_METHOD_MATH_HYPOT, 49, Generic; 0, 0),
    (KILN_C99_METHOD_MATH_ATAN2, 50, Generic, MathNumber; 0, 0),
    (KILN_C99_METHOD_MATH_SIN, 51, Generic, MathNumber; 0, 0),
    (KILN_C99_METHOD_MATH_COS, 52, Generic, MathNumber; 0, 0),
    (KILN_C99_METHOD_MATH_TAN, 53, Generic, MathNumber; 0, 0),
    (KILN_C99_METHOD_MATH_ASIN, 54, Generic, MathNumber; 0, 0),
    (KILN_C99_METHOD_MATH_ACOS, 55, Generic, MathNumber; 0, 0),
    (KILN_C99_METHOD_MATH_ATAN, 56, Generic, MathNumber; 0, 0),
    (KILN_C99_METHOD_IS_INTEGER, 57, Generic; 0, 0),
    (KILN_C99_METHOD_IS_SAFE_INTEGER, 58, Generic; 0, 0),
    (KILN_C99_METHOD_IS_NAN, 59, Generic; 0, 0),
    (KILN_C99_METHOD_IS_FINITE, 60, Generic; 0, 0),
    (KILN_C99_METHOD_OBJECT_KEYS, 61, Generic; 0, 0),
    (KILN_C99_METHOD_OBJECT_VALUES, 62, Generic; 0, 0),
    (KILN_C99_METHOD_OBJECT_ENTRIES, 63, Generic; 0, 0),
    (KILN_C99_METHOD_OBJECT_ASSIGN, 64, Generic; 0, 0),
    (KILN_C99_METHOD_HAS_OWN, 65, Generic; 0, 0),
    (KILN_C99_METHOD_IS_ARRAY, 66, Generic; 0, 0),
    (KILN_C99_METHOD_ARRAY_OF, 67, Generic; 0, 0),
    (KILN_C99_METHOD_OBJECT_CREATE, 68, Generic; 0, 0),
    (KILN_C99_METHOD_JSON_STRINGIFY, 69, Generic; 0, 0),
    (KILN_C99_METHOD_JSON_PARSE, 70, Generic; 0, 0),
    (KILN_C99_METHOD_GENERATOR_NEXT, 71, Generic; 0, 0),
    (KILN_C99_METHOD_GENERATOR_RETURN, 72, Generic; 0, 0),
    (KILN_C99_METHOD_GENERATOR_SELF, 73, Generic; 0, 0),
    (KILN_C99_METHOD_GET_TIME, 74, Generic; 0, 0),
    (KILN_C99_METHOD_DATE_NOW, 87, Generic; 0, 0),
    (KILN_C99_METHOD_DATE_UTC, 88, Generic; 0, 0),
    (KILN_C99_METHOD_DATE_PARSE, 89, Generic; 0, 0),
    (KILN_C99_METHOD_HAS_OWN_PROPERTY, 90, Generic; 0, 0),
    (KILN_C99_METHOD_OBJECT_TO_STRING, 91, Generic; 0, 0),
    (KILN_C99_METHOD_OBJECT_VALUE_OF, 92, Generic; 0, 0),
    (KILN_C99_METHOD_COLLECTION_GET, 93, Generic; 0, 0),
    (KILN_C99_METHOD_COLLECTION_SET, 94, Generic; 0, 0),
    (KILN_C99_METHOD_COLLECTION_ADD, 95, Generic; 0, 0),
    (KILN_C99_METHOD_COLLECTION_HAS, 96, Generic; 0, 0),
    (KILN_C99_METHOD_COLLECTION_DELETE, 97, Generic; 0, 0),
    (KILN_C99_METHOD_COLLECTION_CLEAR, 98, Generic; 0, 0),
    (KILN_C99_METHOD_COLLECTION_FOR_EACH, 99, Generic; 0, 0),
    (KILN_C99_METHOD_COLLECTION_KEYS, 100, Generic; 0, 0),
    (KILN_C99_METHOD_COLLECTION_VALUES, 101, Generic; 0, 0),
    (KILN_C99_METHOD_COLLECTION_ENTRIES, 102, Generic; 0, 0),
    (KILN_C99_METHOD_COLLECTION_SELF, 103, Generic; 0, 0),
    (KILN_C99_METHOD_ITERATOR_NEXT, 104, Generic; 0, 0),
    (KILN_C99_METHOD_ITERATOR_SELF, 105, Generic; 0, 0),
    (KILN_C99_METHOD_ARRAY_VALUES, 106, Generic; 0, 0),
    (KILN_C99_METHOD_THEN, 107, Generic; 0, 0),
    (KILN_C99_METHOD_CATCH, 108, Generic; 0, 0),
    (KILN_C99_METHOD_FINALLY, 109, Generic; 0, 0),
    (KILN_C99_METHOD_PROMISE_RESOLVE, 110, Generic; 0, 0),
    (KILN_C99_METHOD_PROMISE_REJECT, 111, Generic; 0, 0),
    (KILN_C99_METHOD_PROMISE_ALL, 112, Generic; 0, 0),
    (KILN_C99_METHOD_PROMISE_RACE, 113, Generic; 0, 0),
    (KILN_C99_METHOD_ASYNC_RESUME, 114, Generic; 0, 0),
    (KILN_C99_METHOD_ASYNC_THROW, 115, Generic; 0, 0),
    (KILN_C99_METHOD_ALL_STEP, 116, Generic; 0, 0),
    (KILN_C99_METHOD_PARSE_INT, 117, Generic; 0, 0),
    (KILN_C99_METHOD_PARSE_FLOAT, 118, Generic; 0, 0),
    (KILN_C99_METHOD_GLOBAL_IS_NAN, 119, Generic; 0, 0),
    (KILN_C99_METHOD_GLOBAL_IS_FINITE, 120, Generic; 0, 0),
    (KILN_C99_METHOD_NUMBER_TO_STRING, 121, Generic; 0x1, 0),
    (KILN_C99_METHOD_NUMBER_TO_FIXED, 122, Generic; 0x1, 0),
    (KILN_C99_METHOD_NUMBER_TO_EXPONENTIAL, 197, Generic; 0x1, 0),
    (KILN_C99_METHOD_NUMBER_TO_PRECISION, 198, Generic; 0x1, 0),
    (KILN_C99_METHOD_NUMBER_VALUE_OF, 123, Generic; 0, 0),
    (KILN_C99_METHOD_ESCAPE, 124, Generic; 0, 0x1),
    (KILN_C99_METHOD_UNESCAPE, 125, Generic; 0, 0x1),
    (KILN_C99_METHOD_ENCODE_URI, 126, Generic; 0, 0),
    (KILN_C99_METHOD_ENCODE_URI_COMPONENT, 127, Generic; 0, 0),
    (KILN_C99_METHOD_DECODE_URI, 128, Generic; 0, 0),
    (KILN_C99_METHOD_DECODE_URI_COMPONENT, 129, Generic; 0, 0),
    (KILN_C99_METHOD_QUEUE_MICROTASK, 130, Generic; 0, 0),
    (KILN_C99_METHOD_SET_TIMEOUT, 131, Generic; 0, 0),
    (KILN_C99_METHOD_SET_INTERVAL, 132, Generic; 0, 0),
    (KILN_C99_METHOD_CLEAR_TIMEOUT, 133, Generic; 0, 0),
    (KILN_C99_METHOD_CLEAR_INTERVAL, 134, Generic; 0, 0),
    (KILN_C99_METHOD_PROMISE_ALL_SETTLED, 135, Generic; 0, 0),
    (KILN_C99_METHOD_PROMISE_ANY, 136, Generic; 0, 0),
    (KILN_C99_METHOD_PROMISE_WITH_RESOLVERS, 411, Generic; 0, 0),
    (KILN_C99_METHOD_NODE_TTY_ISATTY, 412, Direct; 0, 0),
    (KILN_C99_METHOD_TYPED_TO_STRING_TAG, 413, Direct; 0, 0),
    (KILN_C99_METHOD_REJECT_STEP, 137, Generic; 0, 0),
    (KILN_C99_METHOD_FINALLY_PASS, 138, Generic; 0, 0),
    (KILN_C99_METHOD_FINALLY_RAISE, 139, Generic; 0, 0),
    (KILN_C99_METHOD_FINALLY_RESTORE, 140, Generic; 0, 0),
    (KILN_C99_METHOD_FINALLY_RERAISE, 141, Generic; 0, 0),
    (KILN_C99_METHOD_ADOPT_STEP, 142, Generic; 0, 0),
    (KILN_C99_METHOD_OBJECT_FREEZE, 143, Generic; 0, 0),
    (KILN_C99_METHOD_OBJECT_IS_FROZEN, 144, Generic; 0, 0),
    (KILN_C99_METHOD_OBJECT_PREVENT_EXTENSIONS, 436, Generic; 0, 0),
    (KILN_C99_METHOD_OBJECT_IS_EXTENSIBLE, 437, Generic; 0, 0),
    (KILN_C99_METHOD_FUNCTION_CALL, 145, Generic; 0, 0),
    (KILN_C99_METHOD_FUNCTION_APPLY, 146, Generic; 0, 0),
    (KILN_C99_METHOD_FUNCTION_BIND, 147, Generic; 0, 0),
    (KILN_C99_METHOD_OBJECT_GET_PROTOTYPE_OF, 148, Generic; 0, 0),
    (KILN_C99_METHOD_OBJECT_SET_PROTOTYPE_OF, 149, Generic; 0, 0),
    (KILN_C99_METHOD_OBJECT_GET_OWN_PROPERTY_NAMES, 150, Generic; 0, 0),
    (KILN_C99_METHOD_OBJECT_FROM_ENTRIES, 151, Generic; 0, 0),
    (KILN_C99_METHOD_ARRAY_FROM, 152, Generic; 0, 0),
    (KILN_C99_METHOD_IS_PROTOTYPE_OF, 153, Generic; 0, 0),
    (KILN_C99_METHOD_PROPERTY_IS_ENUMERABLE, 154, Generic; 0, 0),
    (KILN_C99_METHOD_STRING_RAW, 155, Generic; 0, 0),
    (KILN_C99_METHOD_STRING_FROM_CHAR_CODE, 442, Generic; 0, 0),
    (KILN_C99_METHOD_OBJECT_DEFINE_PROPERTY, 156, Generic; 0, 0),
    (KILN_C99_METHOD_OBJECT_DEFINE_PROPERTIES, 157, Generic; 0, 0),
    (KILN_C99_METHOD_OBJECT_GET_OWN_PROPERTY_DESCRIPTOR, 158, Generic; 0, 0),
    (KILN_C99_METHOD_OBJECT_GET_OWN_PROPERTY_DESCRIPTORS, 159, Generic; 0, 0),
    (KILN_C99_METHOD_OBJECT_IS, 185, Generic; 0, 0),
    (KILN_C99_METHOD_OBJECT_TO_LOCALE_STRING, 196, Generic; 0, 0),
    (KILN_C99_METHOD_ERROR_CAPTURE_STACK_TRACE, 186, Generic; 0, 0),
    (KILN_C99_METHOD_REFLECT_OWN_KEYS, 187, Generic; 0, 0),
    (KILN_C99_METHOD_REFLECT_GET, 188, Generic; 0, 0),
    (KILN_C99_METHOD_REFLECT_SET, 189, Generic; 0, 0),
    (KILN_C99_METHOD_REFLECT_HAS, 190, Generic; 0, 0),
    (KILN_C99_METHOD_REFLECT_DELETE_PROPERTY, 191, Generic; 0, 0),
    (KILN_C99_METHOD_REFLECT_APPLY, 192, Generic; 0, 0),
    (KILN_C99_METHOD_REFLECT_CONSTRUCT, 193, Generic; 0, 0),
    (KILN_C99_METHOD_PROXY_REVOCABLE, 194, Generic; 0, 0),
    (KILN_C99_METHOD_PROXY_REVOKE, 195, Generic; 0, 0),
    (KILN_C99_METHOD_MATH_RANDOM, 160, Generic; 0, 0),
    (KILN_C99_METHOD_MATH_SINH, 161, Generic, MathNumber; 0, 0),
    (KILN_C99_METHOD_MATH_COSH, 162, Generic, MathNumber; 0, 0),
    (KILN_C99_METHOD_MATH_TANH, 163, Generic, MathNumber; 0, 0),
    (KILN_C99_METHOD_MATH_ASINH, 164, Generic, MathNumber; 0, 0),
    (KILN_C99_METHOD_MATH_ACOSH, 165, Generic, MathNumber; 0, 0),
    (KILN_C99_METHOD_MATH_ATANH, 166, Generic, MathNumber; 0, 0),
    (KILN_C99_METHOD_MATH_LOG1P, 167, Generic, MathNumber; 0, 0),
    (KILN_C99_METHOD_MATH_EXPM1, 168, Generic, MathNumber; 0, 0),
    (KILN_C99_METHOD_MATH_FROUND, 169, Generic, MathNumber; 0, 0),
    (KILN_C99_METHOD_MATH_CLZ32, 170, Generic, MathNumber; 0, 0),
    (KILN_C99_METHOD_MATH_IMUL, 171, Generic, MathNumber; 0, 0),
    (KILN_C99_METHOD_REGEXP_TEST, 172, Generic; 0, 0),
    (KILN_C99_METHOD_REGEXP_EXEC, 173, Generic; 0, 0),
    (KILN_C99_METHOD_REGEXP_TO_STRING, 174, Generic; 0, 0),
    (KILN_C99_METHOD_COLLECTION_ITERATOR_NEXT, 175, Generic; 0, 0),
    (KILN_C99_METHOD_PRIMITIVE_TO_STRING, 176, Generic; 0, 0),
    (KILN_C99_METHOD_TYPED_OF, 233, Direct; 0, 0),
    (KILN_C99_METHOD_TYPED_FROM, 234, Direct; 0, 0),
    (KILN_C99_METHOD_ARRAY_GENERIC_MAP, 235, Direct; 0, 0),
    (KILN_C99_METHOD_ARRAY_GENERIC_FILTER, 236, Direct; 0, 0),
    (KILN_C99_METHOD_NODE_EVENT_ON, 260, NodeEvent; 0, 0),
    (KILN_C99_METHOD_NODE_EVENT_ONCE, 261, NodeEvent; 0, 0),
    (KILN_C99_METHOD_NODE_EVENT_EMIT, 262, NodeEvent; 0, 0),
    (KILN_C99_METHOD_NODE_EVENT_LISTENERS, 263, NodeEvent; 0, 0),
    (KILN_C99_METHOD_NODE_EVENT_EVENT_NAMES, 529, NodeEvent; 0, 0),
    (KILN_C99_METHOD_NODE_EVENT_REMOVE_LISTENER, 264, NodeEvent; 0, 0),
    (KILN_C99_METHOD_NODE_EVENT_REMOVE_ALL_LISTENERS, 265, NodeEvent; 0, 0),
    (KILN_C99_METHOD_NODE_EVENT_SET_MAX_LISTENERS, 266, NodeEvent; 0, 0),
    (KILN_C99_METHOD_NODE_EVENT_EMITTER, 267, NodeEvent; 0, 0),
    (KILN_C99_METHOD_NODE_NET_CREATE_SERVER, 268, NodeNet; 0, 0),
    (KILN_C99_METHOD_NODE_SERVER_LISTEN, 269, NodeNet, NodeServer; 0, 0),
    (KILN_C99_METHOD_NODE_SERVER_CLOSE, 270, NodeNet, NodeServer; 0, 0),
    (KILN_C99_METHOD_NODE_SERVER_ADDRESS, 271, NodeNet, NodeServer; 0, 0),
    (KILN_C99_METHOD_NODE_SOCKET_WRITE, 272, NodeNet; 0, 0),
    (KILN_C99_METHOD_NODE_SOCKET_END, 273, NodeNet; 0, 0),
    (KILN_C99_METHOD_NODE_SOCKET_DESTROY, 274, NodeNet; 0, 0),
    (KILN_C99_METHOD_NODE_HTTP_CREATE_SERVER, 275, NodeHttp; 0, 0),
    (KILN_C99_METHOD_NODE_RESPONSE_SET_HEADER, 276, NodeHttp; 0, 0),
    (KILN_C99_METHOD_NODE_RESPONSE_WRITE_HEAD, 277, NodeHttp; 0, 0),
    (KILN_C99_METHOD_NODE_RESPONSE_WRITE, 278, NodeHttp; 0, 0),
    (KILN_C99_METHOD_NODE_RESPONSE_END, 279, NodeHttp; 0, 0),
    (KILN_C99_METHOD_NODE_HTTP_GET, 280, NodeHttp; 0, 0),
    (KILN_C99_METHOD_NODE_HTTP_REQUEST, 281, NodeHttp; 0, 0),
    /* The private host half of `serve()` (web.ts), reached only through
     * `process.__kilnNativeModules.http`: a server whose listener receives each request whole as
     * strings and bytes, and the one call that answers it. Plumbing for a TypeScript built-in,
     * not a built-in -- neither is a member of any public object. */
    (KILN_C99_METHOD_NODE_HTTP_CREATE_FETCH_SERVER, 530, NodeHttp; 0, 0),
    (KILN_C99_METHOD_NODE_HTTP_FETCH_RESPOND, 531, NodeHttp; 0, 0),
    (KILN_C99_METHOD_NODE_CLIENT_REQUEST_WRITE, 282, NodeHttp; 0, 0),
    (KILN_C99_METHOD_NODE_CLIENT_REQUEST_END, 283, NodeHttp; 0, 0),
    /* Three groups, and no precedence between them: `kiln_c99_invoke_method` routes this
     * code by receiver before it consults any group. See the comment there. */
    (KILN_C99_METHOD_NODE_READABLE_SET_ENCODING, 284, NodeHttp, NodeNet, NodeStream; 0, 0),
    (KILN_C99_METHOD_NODE_NET_CONNECT, 338, Direct, NodeNet; 0, 0),
    (KILN_C99_METHOD_URL_SEARCH_PARAMS_GET, 339, Direct; 0, 0),
    (KILN_C99_METHOD_BUFFER_FROM, 340, Buffer; 0, 0),
    (KILN_C99_METHOD_BUFFER_ALLOC, 341, Buffer; 0, 0),
    (KILN_C99_METHOD_BUFFER_IS_BUFFER, 342, Buffer; 0, 0),
    (KILN_C99_METHOD_BUFFER_BYTE_LENGTH, 343, Buffer; 0, 0),
    (KILN_C99_METHOD_BUFFER_CONCAT, 344, Buffer; 0, 0),
    (KILN_C99_METHOD_BUFFER_TO_STRING, 345, Buffer; 0, 0),
    // The same decoder under the name the shared Buffer bootstrap captures it by
    // (`__kilnBufferToString`), so `Buffer.prototype.toString.name` stays "toString".
    (KILN_C99_METHOD_BUFFER_TO_STRING_RAW, 528, Buffer; 0, 0),
    (KILN_C99_METHOD_NODE_UTIL_DEBUGLOG, 346, NodeUtil; 0, 0),
    (KILN_C99_METHOD_NODE_UTIL_NOOP, 347, NodeUtil; 0, 0),
    (KILN_C99_METHOD_NODE_UTIL_DEPRECATE, 348, NodeUtil; 0, 0),
    (KILN_C99_METHOD_NODE_UTIL_INSPECT, 349, NodeUtil; 0, 0),
    (KILN_C99_METHOD_NODE_UTIL_INHERITS, 350, NodeUtil; 0, 0),
    (KILN_C99_METHOD_NODE_UTIL_FORMAT, 351, NodeUtil; 0, 0),
    (KILN_C99_METHOD_NODE_DIAGNOSTICS_CHANNEL, 352, NodeDiagnostics; 0, 0),
    (KILN_C99_METHOD_NODE_DIAGNOSTICS_TRACING_CHANNEL, 353, NodeDiagnostics; 0, 0),
    (KILN_C99_METHOD_NODE_DIAGNOSTICS_PUBLISH, 354, NodeDiagnostics; 0, 0),
    (KILN_C99_METHOD_NODE_DIAGNOSTICS_RUN_STORES, 355, NodeDiagnostics; 0, 0),
    (KILN_C99_METHOD_NODE_HTTPS_CREATE_SERVER, 356, Direct; 0, 0),
    (KILN_C99_METHOD_NODE_HTTP2_CREATE_SERVER, 357, Direct; 0, 0),
    (KILN_C99_METHOD_NODE_HTTP2_CREATE_SECURE_SERVER, 358, Direct; 0, 0),
    (KILN_C99_METHOD_NODE_DNS_LOOKUP, 359, Direct; 0, 0),
    (KILN_C99_METHOD_NODE_OS_HOSTNAME, 360, Direct; 0, 0),
    (KILN_C99_METHOD_NODE_OS_NETWORK_INTERFACES, 361, Direct; 0, 0),
    (KILN_C99_METHOD_NODE_STREAM, 362, NodeStream; 0, 0),
    (KILN_C99_METHOD_NODE_READABLE, 363, NodeStream; 0, 0),
    (KILN_C99_METHOD_NODE_WRITABLE, 364, NodeStream; 0, 0),
    (KILN_C99_METHOD_NODE_STREAM_ADD_ABORT_SIGNAL, 365, NodeStream; 0, 0),
    (KILN_C99_METHOD_NODE_STREAM_FINISHED, 366, NodeStream; 0, 0),
    (KILN_C99_METHOD_NODE_READABLE_PUSH, 367, NodeStream; 0, 0),
    (KILN_C99_METHOD_NODE_READABLE_READ, 368, NodeStream; 0, 0),
    (KILN_C99_METHOD_NODE_READABLE_DESTROY, 369, NodeStream; 0, 0),
    (KILN_C99_METHOD_NODE_READABLE_PAUSE, 370, NodeStream; 0, 0),
    (KILN_C99_METHOD_NODE_READABLE_RESUME, 371, NodeStream; 0, 0),
    (KILN_C99_METHOD_NODE_READABLE_PIPE, 372, NodeStream; 0, 0),
    (KILN_C99_METHOD_NODE_READABLE_UNPIPE, 373, NodeStream; 0, 0),
    (KILN_C99_METHOD_NODE_WRITABLE_WRITE, 374, NodeStream; 0, 0),
    (KILN_C99_METHOD_NODE_WRITABLE_END, 375, NodeStream; 0, 0),
    (KILN_C99_METHOD_NODE_WRITABLE_DESTROY, 376, NodeStream; 0, 0),
    (KILN_C99_METHOD_PERFORMANCE_NOW, 377, Direct; 0, 0),
    (KILN_C99_METHOD_NODE_FS_EXISTS_SYNC, 378, Direct; 0, 0),
    (KILN_C99_METHOD_NODE_FS_READ_FILE, 379, Direct; 0, 0),
    (KILN_C99_METHOD_NODE_FS_STAT, 380, Direct; 0, 0),
    (KILN_C99_METHOD_NODE_FS_WRITE_FILE, 381, Direct; 0, 0),
    (KILN_C99_METHOD_NODE_PATH_IS_ABSOLUTE, 382, NodePath; 0, 0),
    (KILN_C99_METHOD_NODE_PATH_JOIN, 383, NodePath; 0, 0),
    (KILN_C99_METHOD_NODE_PATH_RESOLVE, 384, NodePath; 0, 0),
    (KILN_C99_METHOD_NODE_PATH_DIRNAME, 438, NodePath; 0, 0),
    (KILN_C99_METHOD_NODE_PATH_BASENAME, 439, NodePath; 0, 0),
    (KILN_C99_METHOD_NODE_PATH_EXTNAME, 440, NodePath; 0, 0),
    (KILN_C99_METHOD_NODE_PATH_RELATIVE, 441, NodePath; 0, 0),
    (KILN_C99_METHOD_NODE_ASSERT_OK, 385, Direct; 0, 0),
    (KILN_C99_METHOD_NODE_WORKER_CONSTRUCTOR, 386, Direct; 0, 0),
    (KILN_C99_METHOD_NODE_MODULE_CREATE_REQUIRE, 387, Direct; 0, 0),
    // `require("node:module")`, which node answers with the `Module` class rather than with a
    // namespace. It gets a code of its own rather than borrowing the unmaterialized placeholder
    // above: two values sharing a code are `===` in this runtime, and `require("node:module")`
    // comparing equal to every unbuilt member of every other builtin would be a wrong answer
    // bought to save a number. `node:stream` needs no code here because
    // `KILN_C99_METHOD_NODE_STREAM` already is its callable.
    (KILN_C99_METHOD_NODE_MODULE, 483, Direct; 0, 0),
    (KILN_C99_METHOD_NODE_FFI_DLOPEN, 495, Direct; 0, 0),
    (KILN_C99_METHOD_NODE_FFI_GET_RAW_POINTER, 496, Direct; 0, 0),
    (KILN_C99_METHOD_NODE_FFI_TO_ARRAY_BUFFER, 497, Direct; 0, 0),
    (KILN_C99_METHOD_NODE_FFI_REGISTER_CALLBACK, 498, Direct; 0, 0),
    (KILN_C99_METHOD_NODE_FFI_UNREGISTER_CALLBACK, 499, Direct; 0, 0),
    (KILN_C99_METHOD_NODE_FFI_CLOSE, 500, Direct; 0, 0),
    // Private bootstrap bridge. Compiled runtime source captures it and removes the global before
    // user code runs; the method itself exposes only ICU4X boundary records, not Intl policy.
    (KILN_C99_METHOD_INTL_SEGMENT_RAW, 501, Direct; 0, 0),
    (KILN_C99_METHOD_STRING_LOCALE_CASE_RAW, 547, Direct; 0, 0),
    (KILN_C99_METHOD_INTL_DATETIME_FORMAT_RAW, 548, Direct; 0, 0),
    (KILN_C99_METHOD_NODE_URL_FILE_URL_TO_PATH, 388, Direct; 0, 0),
    (KILN_C99_METHOD_NODE_URL_PARSE, 389, Direct; 0, 0),
    (KILN_C99_METHOD_NODE_URL_PATH_TO_FILE_URL, 390, Direct; 0, 0),
    (KILN_C99_METHOD_NODE_ASYNC_RESOURCE, 391, Direct; 0, 0),
    (KILN_C99_METHOD_NODE_ASYNC_RESOURCE_BIND, 392, Direct; 0, 0),
    (KILN_C99_METHOD_NODE_ASYNC_RESOURCE_DESTROY, 393, Direct; 0, 0),
    (KILN_C99_METHOD_NODE_CRYPTO_RANDOM_UUID, 394, Direct; 0, 0),
    (KILN_C99_METHOD_NODE_CRYPTO_CREATE_HASH, 395, Direct; 0, 0),
    (KILN_C99_METHOD_PROCESS_NEXT_TICK, 396, Direct; 0, 0),
    (KILN_C99_METHOD_PROCESS_START_RAW, 508, Direct; 0, 0),
    (KILN_C99_METHOD_PROCESS_CANCEL_RAW, 509, Direct; 0, 0),
    (KILN_C99_METHOD_PROCESS_HRTIME, 397, Direct; 0, 0),
    (KILN_C99_METHOD_PROCESS_HRTIME_BIGINT, 398, Direct; 0, 0),
    (KILN_C99_METHOD_PROCESS_CWD, 399, Direct; 0, 0),
    (KILN_C99_METHOD_ATOMICS_UNAVAILABLE, 400, Direct; 0, 0),
    (KILN_C99_METHOD_SET_IMMEDIATE, 401, Generic; 0, 0),
    (KILN_C99_METHOD_CLEAR_IMMEDIATE, 402, Generic; 0, 0),
    (KILN_C99_METHOD_NODE_HTTP_SERVER, 403, Direct; 0, 0),
    (KILN_C99_METHOD_NODE_SERVER_SET_TIMEOUT, 404, Direct, NodeServer, NodeNet; 0, 0),
    (KILN_C99_METHOD_NODE_RESPONSE_GET_HEADER, 405, Direct, NodeResponse; 0, 0),
    (KILN_C99_METHOD_NODE_RESPONSE_GET_HEADERS, 406, Direct, NodeResponse; 0, 0),
    (KILN_C99_METHOD_NODE_RESPONSE_HAS_HEADER, 407, Direct, NodeResponse; 0, 0),
    (KILN_C99_METHOD_NODE_RESPONSE_REMOVE_HEADER, 408, Direct, NodeResponse; 0, 0),
    (KILN_C99_METHOD_SYMBOL_FOR, 409, Direct; 0, 0),
    (KILN_C99_METHOD_SYMBOL_KEY_FOR, 410, Direct; 0, 0),
    (KILN_C99_METHOD_NODE_STAT_IS_FILE, 414, Direct; 0, 0),
    (KILN_C99_METHOD_NODE_STAT_IS_DIRECTORY, 415, Direct; 0, 0),
    (KILN_C99_METHOD_ABORT_CONTROLLER_ABORT, 416, Generic; 0, 0),
    (KILN_C99_METHOD_ABORT_SIGNAL_ABORT, 417, Generic; 0, 0),
    (KILN_C99_METHOD_ABORT_SIGNAL_LISTEN, 418, Generic; 0, 0),
    (KILN_C99_METHOD_ABORT_SIGNAL_UNLISTEN, 419, Generic; 0, 0),
    (KILN_C99_METHOD_ABORT_SIGNAL_THROW, 420, Generic; 0, 0),
    (KILN_C99_METHOD_ABORT_SIGNAL_TIMEOUT, 421, Generic; 0, 0),
    (KILN_C99_METHOD_ABORT_SIGNAL_ANY, 422, Generic; 0, 0),
    (KILN_C99_METHOD_ABORT_SIGNAL_SETTLE, 423, Generic; 0, 0),
    (KILN_C99_METHOD_ABORT_SIGNAL_FOLLOW, 424, Generic; 0, 0),
    (KILN_C99_METHOD_TEXT_ENCODER_ENCODE, 425, Generic; 0, 0),
    (KILN_C99_METHOD_NODE_HTTP_SERVER_RESPONSE, 426, Direct, NodeHttp; 0, 0),
    (KILN_C99_METHOD_NODE_QUERYSTRING_PARSE, 427, Direct; 0, 0),
    (KILN_C99_METHOD_FUNCTION_TO_STRING, 428, Generic; 0, 0),
    (KILN_C99_METHOD_ARRAY_BUFFER_IS_VIEW, 429, Generic; 0, 0),
    (KILN_C99_METHOD_THROW_TYPE_ERROR, 430, Generic; 0, 0),
    /* `console.log` reached as a *value* rather than through the `console.log(x)` shape the
     * compiler recognises. `Direct` and attached to the console object by name, exactly as the
     * `Atomics` methods are: nothing looks these up by name through a group range, so a `log`
     * on some other receiver cannot find them. */
    (KILN_C99_METHOD_CONSOLE_LOG, 431, Direct; 0, 0),
    (KILN_C99_METHOD_CONSOLE_ERROR, 432, Direct; 0, 0),
    (KILN_C99_METHOD_OBJECT_GET_OWN_PROPERTY_SYMBOLS, 433, Generic; 0, 0),
    /* The two halves of `req.headers`. They are a method pair rather than a stored object so
     * that the header table is built only if a program reads it -- see the accessor installed in
     * `kiln_c99_node_http_event`. */
    (KILN_C99_METHOD_NODE_REQUEST_HEADERS_GET, 434, Direct, NodeHttp; 0, 0),
    (KILN_C99_METHOD_NODE_REQUEST_HEADERS_SET, 435, Direct, NodeHttp; 0, 0),
    (KILN_C99_METHOD_TEMPORAL_FROM, 285, Temporal, TemporalFrom; 0, 0),
    (KILN_C99_METHOD_TEMPORAL_FROM_LAST, 292, Temporal, TemporalFrom; 0, 0),
    (KILN_C99_METHOD_TEMPORAL_COMPARE, 293, Temporal, TemporalCompare; 0, 0),
    (KILN_C99_METHOD_TEMPORAL_COMPARE_LAST, 300, Temporal, TemporalCompare; 0, 0),
    (KILN_C99_METHOD_TEMPORAL_TO_STRING, 301, Temporal; 0, 0),
    (KILN_C99_METHOD_TEMPORAL_TO_JSON, 302, Temporal; 0, 0),
    (KILN_C99_METHOD_TEMPORAL_VALUE_OF, 303, Temporal; 0, 0),
    (KILN_C99_METHOD_TEMPORAL_NOW_TIME_ZONE_ID, 304, Temporal; 0, 0),
    (KILN_C99_METHOD_TEMPORAL_NOW_READ, 305, Temporal, TemporalNowRead; 0, 0),
    (KILN_C99_METHOD_TEMPORAL_NOW_READ_LAST, 312, Temporal, TemporalNowRead; 0, 0),
    (KILN_C99_METHOD_TEMPORAL_OP, 313, Temporal, TemporalOp; 0, 0),
    (KILN_C99_METHOD_TEMPORAL_OP_LAST, 335, Temporal, TemporalOp; 0, 0),
    (KILN_C99_METHOD_TEMPORAL_INSTANT_FROM_EPOCH_MILLISECONDS, 336, Temporal; 0, 0),
    (KILN_C99_METHOD_TEMPORAL_INSTANT_FROM_EPOCH_NANOSECONDS, 337, Temporal; 0, 0),
    (KILN_C99_METHOD_TEMPORAL_LAST, 337, Temporal; 0, 0),
    (KILN_C99_METHOD_SET_TIME, 177, Generic, DateSetter; 0, 0),
    // `DataView.prototype`. `Direct` because the whole block is answered by one handler the
    // dispatcher reaches through a range test, which is what the group means here -- the same
    // arrangement `KILN_C99_METHOD_TYPED_OF` and its neighbours already use.
    (KILN_C99_METHOD_DATA_VIEW_GET_INT8, 447, Direct; 0, 0),
    (KILN_C99_METHOD_DATA_VIEW_GET_UINT8, 448, Direct; 0, 0),
    (KILN_C99_METHOD_DATA_VIEW_GET_INT16, 449, Direct; 0, 0),
    (KILN_C99_METHOD_DATA_VIEW_GET_UINT16, 450, Direct; 0, 0),
    (KILN_C99_METHOD_DATA_VIEW_GET_INT32, 451, Direct; 0, 0),
    (KILN_C99_METHOD_DATA_VIEW_GET_UINT32, 452, Direct; 0, 0),
    (KILN_C99_METHOD_DATA_VIEW_GET_FLOAT32, 453, Direct; 0, 0),
    (KILN_C99_METHOD_DATA_VIEW_GET_FLOAT64, 454, Direct; 0, 0),
    (KILN_C99_METHOD_DATA_VIEW_GET_BIG_INT64, 455, Direct; 0, 0),
    (KILN_C99_METHOD_DATA_VIEW_GET_BIG_UINT64, 456, Direct; 0, 0),
    (KILN_C99_METHOD_DATA_VIEW_SET_INT8, 457, Direct; 0, 0),
    (KILN_C99_METHOD_DATA_VIEW_SET_UINT8, 458, Direct; 0, 0),
    (KILN_C99_METHOD_DATA_VIEW_SET_INT16, 459, Direct; 0, 0),
    (KILN_C99_METHOD_DATA_VIEW_SET_UINT16, 460, Direct; 0, 0),
    (KILN_C99_METHOD_DATA_VIEW_SET_INT32, 461, Direct; 0, 0),
    (KILN_C99_METHOD_DATA_VIEW_SET_UINT32, 462, Direct; 0, 0),
    (KILN_C99_METHOD_DATA_VIEW_SET_FLOAT32, 463, Direct; 0, 0),
    (KILN_C99_METHOD_DATA_VIEW_SET_FLOAT64, 464, Direct; 0, 0),
    (KILN_C99_METHOD_DATA_VIEW_SET_BIG_INT64, 465, Direct; 0, 0),
    (KILN_C99_METHOD_DATA_VIEW_SET_BIG_UINT64, 466, Direct; 0, 0),

    /* The iterator helpers (ES2025). Contiguous on purpose, twice over: 470..481 is the range
     * `Iterator.prototype` is read through by name, and 470..484 is the range the dispatcher
     * routes to `kiln_c99_iterator_method`. Nothing else may sit inside either, and the three
     * codes past the name range -- the two methods a helper carries and `Iterator.from` -- are
     * deliberately outside it, because `next` and `return` are the *helper's* members and
     * `Iterator.prototype.next` is not a thing the language has. */
    (KILN_C99_METHOD_ITERATOR_MAP, 467, Generic; 0, 0),
    (KILN_C99_METHOD_ITERATOR_FILTER, 468, Generic; 0, 0),
    (KILN_C99_METHOD_ITERATOR_TAKE, 469, Generic; 0, 0),
    (KILN_C99_METHOD_ITERATOR_DROP, 470, Generic; 0, 0),
    (KILN_C99_METHOD_ITERATOR_FLAT_MAP, 471, Generic; 0, 0),
    (KILN_C99_METHOD_ITERATOR_REDUCE, 472, Generic; 0, 0),
    (KILN_C99_METHOD_ITERATOR_TO_ARRAY, 473, Generic; 0, 0),
    (KILN_C99_METHOD_ITERATOR_FOR_EACH, 474, Generic; 0, 0),
    (KILN_C99_METHOD_ITERATOR_SOME, 475, Generic; 0, 0),
    (KILN_C99_METHOD_ITERATOR_EVERY, 476, Generic; 0, 0),
    (KILN_C99_METHOD_ITERATOR_FIND, 477, Generic; 0, 0),
    (KILN_C99_METHOD_ITERATOR_INCLUDES, 478, Generic; 0, 0),
    (KILN_C99_METHOD_ITERATOR_HELPER_NEXT, 479, Generic; 0, 0),
    (KILN_C99_METHOD_ITERATOR_HELPER_RETURN, 480, Generic; 0, 0),
    (KILN_C99_METHOD_ITERATOR_FROM, 481, Generic; 0, 0),
    // A member a node builtin publishes and this backend has not built. One code for all of them,
    // because the refusal does not need to be told which member it is: the instruction that
    // called it already carries the name, so this answers `CallMethod(readFileSync)`.
    (KILN_C99_METHOD_NODE_BUILTIN_UNMATERIALIZED, 482, Direct; 0, 0),
    (KILN_C99_METHOD_REFLECT_DEFINE_PROPERTY, 512, Generic; 0, 0),
    (KILN_C99_METHOD_REFLECT_GET_PROTOTYPE_OF, 484, Generic; 0, 0),
    (KILN_C99_METHOD_GENERATOR_THROW, 485, Generic; 0, 0),
    (KILN_C99_METHOD_TEXT_DECODER_DECODE, 486, Generic; 0, 0),
    (KILN_C99_METHOD_NODE_ASYNC_LOCAL_STORAGE, 487, Direct; 0, 0),
    (KILN_C99_METHOD_NODE_ALS_GET_STORE, 488, Direct; 0, 0),
    (KILN_C99_METHOD_NODE_ALS_RUN, 489, Direct; 0, 0),
    (KILN_C99_METHOD_NODE_ALS_ENTER_WITH, 490, Direct; 0, 0),
    (KILN_C99_METHOD_NODE_ALS_EXIT, 491, Direct; 0, 0),
    (KILN_C99_METHOD_NODE_ALS_DISABLE, 492, Direct; 0, 0),
    (KILN_C99_METHOD_STRING_FROM_CODE_POINT, 493, Generic; 0, 0),
    /* `parallelMap` (ADR 0058, application 2). A member of `Array.prototype` on both engines,
     * so a *read* answers a callable and a feature test takes the same branch as on the
     * interpreter; called through a read value it is `map` under its own name, generic like
     * the one borrowed from `Array.prototype`. The pooled walk is a direct-call-site lowering
     * and never reaches this code. */
    (KILN_C99_METHOD_ARRAY_PARALLEL_MAP, 494, ArrayLookup; 0, 0),
    (KILN_C99_METHOD_NODE_EVENT_LISTENER_COUNT, 502, NodeEvent; 0, 0),
    (KILN_C99_METHOD_PROCESS_KILL, 503, Direct; 0, 0),
    (KILN_C99_METHOD_NODE_READABLE_SET_RAW_MODE, 504, NodeStream; 0, 0),
    (KILN_C99_METHOD_NODE_OS_HOMEDIR, 505, Direct; 0, 0),
    (KILN_C99_METHOD_NODE_OS_TMPDIR, 506, Direct; 0, 0),
    (KILN_C99_METHOD_NODE_FS_MKDIR, 507, Direct; 0, 0),
    // Private bootstrap bridges for Uint8Array codecs (ADR 0005).
    // Captured and deleted by the compiled builtin bootstrap before user code runs.
    (KILN_C99_METHOD_UINT8_TO_HEX_RAW, 510, Direct; 0, 0),
    (KILN_C99_METHOD_UINT8_TO_BASE64_RAW, 511, Direct; 0, 0),
    // Private bootstrap bridges for `URL` (ADR 0063 rule 4): the `url` crate's parse and set
    // through the provider, the request fast path, and the prototypes the fast path hands out.
    (KILN_C99_METHOD_URL_PARSE_RAW, 524, Direct; 0, 0),
    (KILN_C99_METHOD_URL_SET_RAW, 525, Direct; 0, 0),
    (KILN_C99_METHOD_URL_FAST_RAW, 526, Direct; 0, 0),
    (KILN_C99_METHOD_URL_ADOPT_RAW, 527, Direct; 0, 0),
    // The transfer primitive (ADR 0065, representation; ADR 0070 primitive C), captured and
    // deleted by `kiln-builtins/src/arraybuffer.ts`.
    (KILN_C99_METHOD_ARRAY_BUFFER_DETACH_RAW, 532, Direct; 0, 0),
    // The plain-data fast path of the structured clone (ADR 0065, measured), captured and
    // deleted by `kiln-builtins/src/structured_clone.ts`.
    (KILN_C99_METHOD_CLONE_DATA_RAW, 533, Direct; 0, 0),
    // Whether a microtask checkpoint would run anything now, for message delivery; captured and
    // deleted by `kiln-builtins/src/web_events_streams.ts`.
    (KILN_C99_METHOD_JOBS_PENDING_RAW, 534, Direct; 0, 0),
    // The realm's I/O lane, node's poll phase, where the same file queues a port's delivery;
    // the timer case of the generic dispatch, beside `setImmediate`.
    (KILN_C99_METHOD_QUEUE_IO_RAW, 535, Generic; 0, 0),
    // Node's unhandled-rejection tracking registers itself here; captured and deleted by
    // `kiln-builtins/src/rejections.ts`.
    (KILN_C99_METHOD_PROMISE_REJECTIONS_RAW, 536, Direct; 0, 0),
    // `%TypedArray%.prototype`'s element-storage primitives (ADR 0065, representation; parallel
    // issue 16), captured and deleted by `kiln-builtins/src/typedarray.ts`: a same-type element
    // copy, and the default-order sort through the provider's `kiln_typed_array_sort`.
    (KILN_C99_METHOD_TYPED_ARRAY_COPY_RAW, 537, Direct; 0, 0),
    (KILN_C99_METHOD_TYPED_ARRAY_SORT_RAW, 538, Direct; 0, 0),
    // The runtime's cross-realm ports and workers (realms issue 06), captured and deleted by
    // `kiln-builtins/src/web_events_streams.ts`: every synchronous operation, and the wait.
    (KILN_C99_METHOD_WORKER_OP_RAW, 539, Direct; 0, 0),
    (KILN_C99_METHOD_PORT_WAIT_RAW, 540, Direct; 0, 0),
    // `SharedArrayBuffer` stores and `Atomics` on their elements (realms issue 06), captured and
    // deleted by `kiln-builtins/src/arraybuffer.ts` and `atomics.ts`.
    (KILN_C99_METHOD_SHARED_BUFFER_RAW, 541, Direct; 0, 0),
    (KILN_C99_METHOD_ATOMICS_RAW, 542, Direct; 0, 0),
    // File descriptor I/O below node:fs's TypeScript checks and callback adapters.
    (KILN_C99_METHOD_FS_DESCRIPTOR_RAW, 543, Direct; 0, 0),
    (KILN_C99_METHOD_SHARE_RAW, 544, Direct; 0, 0),
    (KILN_C99_METHOD_SHARED_IS_RAW, 545, Direct; 0, 0),
    (KILN_C99_METHOD_SHARED_WEAK_RAW, 546, Direct; 0, 0),
    /* Existing primitive-wrapper valueOf methods need identities distinct from Object's. */
    (KILN_C99_METHOD_BOOLEAN_VALUE_OF, 549, Generic; 0, 0),
    (KILN_C99_METHOD_SYMBOL_VALUE_OF, 550, Generic; 0, 0),
    (KILN_C99_METHOD_COLLECTION_SIZE_RAW, 551, Direct; 0, 0),
    (KILN_C99_METHOD_JSON_UNBOX_RAW, 552, Direct; 0, 0),
    (KILN_C99_METHOD_PROCESS_EXIT, 513, Direct; 0, 0),
    (KILN_C99_METHOD_NODE_FS_READ_FILE_SYNC, 514, Direct; 0, 0),
    (KILN_C99_METHOD_NODE_FS_WRITE_FILE_SYNC, 515, Direct; 0, 0),
    (KILN_C99_METHOD_NODE_FS_MKDIR_SYNC, 516, Direct; 0, 0),
    (KILN_C99_METHOD_NODE_FS_STAT_SYNC, 517, Direct; 0, 0),
    (KILN_C99_METHOD_NODE_FS_LSTAT_SYNC, 518, Direct; 0, 0),
    (KILN_C99_METHOD_NODE_FS_READDIR_SYNC, 519, Direct; 0, 0),
    (KILN_C99_METHOD_NODE_FS_REALPATH_SYNC, 520, Direct; 0, 0),
    (KILN_C99_METHOD_NODE_FS_UNLINK_SYNC, 521, Direct; 0, 0),
    (KILN_C99_METHOD_NODE_FS_RMDIR_SYNC, 522, Direct; 0, 0),
    (KILN_C99_METHOD_NODE_STAT_IS_SYMBOLIC_LINK, 523, Direct; 0, 0),
}

/// The hook a host installs when it lets a program make a second realm.
///
/// Spelled here rather than shared with `kiln-interpreter` because this backend deliberately
/// takes no dependency on that crate: what it needs is the *name* a program would reach for, and
/// a name is a fact about the program, not about the interpreter's tables.
pub(crate) const HOST_CREATE_REALM: &str = "$262CreateRealm";

/// The hook a host installs when it lets a program evaluate Script source at run time.
///
/// Refused for the same reason `DirectEval` is: this backend has no compiler to reach at run
/// time. Named separately from `eval` because it is a different name in the program.
pub(crate) const HOST_EVAL_SCRIPT: &str = "$262EvalScript";

/// The hooks a host installs when it lets a program start a second agent.
///
/// Refused for a sharper reason than the realm hook is. A second agent is a second thread with a
/// realm of its own, blocking `Atomics.wait`, and a scheduler that decides the interleaving; this
/// backend emits one C translation unit with one global object and no scheduler at all. A
/// compiled program that reached these would start nothing, wait on nobody, and answer every
/// `Atomics.wait` "timed-out" while the interpreter answered "ok" -- a silent divergence between
/// the two engines, which is the one failure `scripts/native.sh` exists to prevent. Refusing by
/// name says which capability is missing, and the differential harness counts it as refused
/// rather than as diverged.
pub(crate) const HOST_AGENT_HOOKS: [&str; 8] = [
    "$262AgentStart",
    "$262AgentBroadcast",
    "$262AgentGetReport",
    "$262AgentSleep",
    "$262AgentMonotonicNow",
    "$262AgentReceiveBroadcast",
    "$262AgentReport",
    "$262AgentLeaving",
];

/// Whether this name is a host hook this backend has no way to honour.
pub(crate) fn refused_host_hook(name: &str) -> bool {
    name == HOST_CREATE_REALM || name == HOST_EVAL_SCRIPT || HOST_AGENT_HOOKS.contains(&name)
}
/// The constructor a program reaches for when it wants a realm it can evaluate in.
///
/// Refused for the same reason the two above are, and it matters more here because
/// `ShadowRealm` is a *language* built-in rather than a host hook: a program can name it without
/// asking anyone's permission. This backend builds one global object, once, in C. A compiled
/// `new ShadowRealm()` would therefore hand back a realm whose intrinsics were secretly this
/// one's -- so `r.evaluate("Array") === Array` would be true, every wrapped function would be
/// the function it wrapped, and the boundary the whole feature exists to enforce would not
/// exist. Refusing by name says which construct is missing; answering would be a confident
/// wrong answer about object identity.
pub(crate) const SHADOW_REALM: &str = "ShadowRealm";

const NATIVE_CONSTRUCTORS: &[NativeConstructor] = &[
    native_constructor!(KILN_C99_METHOD_NODE_EVENT_EMITTER, None, EventEmitter),
    native_constructor!(
        KILN_C99_METHOD_NODE_READABLE,
        Some(method_value!(KILN_C99_METHOD_NODE_EVENT_EMITTER)),
        Readable
    ),
    native_constructor!(
        KILN_C99_METHOD_NODE_WRITABLE,
        Some(method_value!(KILN_C99_METHOD_NODE_EVENT_EMITTER)),
        Writable
    ),
    native_constructor!(
        KILN_C99_METHOD_NODE_HTTP_SERVER_RESPONSE,
        Some(method_value!(KILN_C99_METHOD_NODE_WRITABLE)),
        ServerResponse
    ),
];

const _: () = validate_method_ids(METHOD_IDS);
const _: () = validate_native_constructors(NATIVE_CONSTRUCTORS, METHOD_IDS);

/// One member of one built-in's *static* surface: what `Number.MAX_VALUE` and `Object.keys` name.
///
/// ONE TABLE, TWO EMITTERS (ADR 0043). Before [`BUILTIN_SURFACE`] the surface was written down
/// twice -- once in the interpreter's Rust, once as hand-written `kiln_c99_same_name` chains in
/// the emitted C -- and nothing compared the two. `Number` had fifteen statics on one side and
/// five on the other, and `Number.MAX_VALUE` is the first property a bundled `zod` reads, so its
/// absence refused the whole module. Those were not four bugs; they were one shape of bug with an
/// unbounded supply.
///
/// The rows below are now the only place the native surface is written down. The read path, the
/// call path and `Object.getOwnPropertyNames` are all generated from them, which is what makes a
/// member that is *implemented but unreachable* inexpressible: reachability is the row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuiltinMember {
    /// The global that owns it, spelled as a program spells it: `"Number"`, `"Math"`.
    pub owner: &'static str,
    /// The member's own name. A `@@` prefix means symbol-keyed, the same spelling the emitted C
    /// uses for a reserved store key.
    ///
    /// It is also the name the member *reports*: `Object.keys.name` is `"keys"`, and the emitted
    /// `kiln_c99_surface_method_name` reads this very string rather than keeping a second copy in
    /// a `case`. See `kiln_builtin_surface`'s "the key is the name".
    pub name: &'static str,
    /// Which row kind this is, spelled as the declaration spells it: `code`, `num`, `text`,
    /// `sym`, `ctor`.
    pub kind: &'static str,
    /// The declaration's argument, still parenthesised as `stringify!` on a token tree leaves it:
    /// `"(KILN_C99_METHOD_OBJECT_KEYS)"`. Carried so a guard can join a row against the C it
    /// generates without parsing the macro invocation back out of this file.
    pub answers: &'static str,
    /// Whether the row is one of kiln's own members rather than the language's (ADR 0068):
    /// installed by default, left out by `KILN_EXTENSIONS=0` or `kiln build
    /// --no-kiln-extensions`. Written on the row as a trailing `kiln_extension` and held equal to
    /// `kiln_builtin_surface::KILN_EXTENSIONS` by [`assert_surface_is_covered`], so a member
    /// cannot be marked on one side only.
    pub kiln_extension: bool,
}

/// `kiln_extension` on a [`builtin_surface!`] row, as the Rust flag and as the emitted C flag.
macro_rules! kiln_extension_flag {
    () => {
        false
    };
    (kiln_extension) => {
        true
    };
}

macro_rules! kiln_extension_c_flag {
    () => {
        "0,"
    };
    (kiln_extension) => {
        "1,"
    };
}

macro_rules! builtin_member_row {
    ($builtin:ident, $name:literal, accessor()) => {
        concat!(
            "    {",
            stringify!($builtin),
            ", KILN_C99_MEMBER_ACCESSOR, 0, 0.0, \"",
            $name,
            "\", NULL},\n"
        )
    };
    ($builtin:ident, $name:literal, code($code:ident)) => {
        concat!(
            "    {",
            stringify!($builtin),
            ", KILN_C99_MEMBER_METHOD, ",
            stringify!($code),
            ", 0.0, \"",
            $name,
            "\", NULL},\n"
        )
    };
    ($builtin:ident, $name:literal, num($value:literal)) => {
        concat!(
            "    {",
            stringify!($builtin),
            ", KILN_C99_MEMBER_NUMBER, 0, ",
            $value,
            ", \"",
            $name,
            "\", NULL},\n"
        )
    };
    ($builtin:ident, $name:literal, text($value:literal)) => {
        concat!(
            "    {",
            stringify!($builtin),
            ", KILN_C99_MEMBER_TEXT, 0, 0.0, \"",
            $name,
            "\", \"",
            $value,
            "\"},\n"
        )
    };
    ($builtin:ident, $name:literal, sym($value:literal)) => {
        concat!(
            "    {",
            stringify!($builtin),
            ", KILN_C99_MEMBER_SYMBOL, 0, 0.0, \"",
            $name,
            "\", \"",
            $value,
            "\"},\n"
        )
    };
    ($builtin:ident, $name:literal, ctor($ctor:ident)) => {
        concat!(
            "    {",
            stringify!($builtin),
            ", KILN_C99_MEMBER_BUILTIN, ",
            stringify!($ctor),
            ", 0.0, \"",
            $name,
            "\", NULL},\n"
        )
    };
    ($builtin:ident, $name:literal, unscopables()) => {
        concat!(
            "    {",
            stringify!($builtin),
            ", KILN_C99_MEMBER_UNSCOPABLES, 0, 0.0, \"",
            $name,
            "\", NULL},\n"
        )
    };
}

/// The single declaration of every table-driven built-in's static surface.
///
/// A row is `(C builtin constant, the global that names it, the member's name, what it answers)`.
/// `code(...)` is a method the program calls, `num(...)` and `text(...)` a value it reads,
/// `sym(...)` a well-known symbol, and `ctor(...)` a built-in handed back by name --
/// `Array[Symbol.species]` is `Array`.
///
/// Rows are declared in the interpreter's own order, because that order is what
/// `Object.getOwnPropertyNames` reports on both sides and agreeing costs nothing here.
///
/// A trailing `kiln_extension` marks one of kiln's own members (ADR 0068); see
/// [`BuiltinMember::kiln_extension`].
macro_rules! builtin_surface {
    ($(($builtin:ident, $owner:literal, $name:literal, $kind:tt $args:tt $(, $extension:ident)?)),* $(,)?) => {
        /// Every member the emitted C answers for, by owner. See [`BuiltinMember`].
        pub const BUILTIN_SURFACE: &[BuiltinMember] = &[
            $(BuiltinMember {
                owner: $owner,
                name: $name,
                kind: stringify!($kind),
                answers: stringify!($args),
                kiln_extension: kiln_extension_flag!($($extension)?),
            },)*
        ];

        macro_rules! emit_builtin_surface_rows {
            () => { concat!($(builtin_member_row!($builtin, $name, $kind $args)),*) };
        }

        macro_rules! emit_builtin_extension_flags {
            () => { concat!($(kiln_extension_c_flag!($($extension)?)),*) };
        }
    };
}

builtin_surface! {
    (KILN_C99_BUILTIN_OBJECT, "Object", "keys", code(KILN_C99_METHOD_OBJECT_KEYS)),
    (KILN_C99_BUILTIN_OBJECT, "Object", "getOwnPropertyNames",
     code(KILN_C99_METHOD_OBJECT_GET_OWN_PROPERTY_NAMES)),
    (KILN_C99_BUILTIN_OBJECT, "Object", "assign", code(KILN_C99_METHOD_OBJECT_ASSIGN)),
    (KILN_C99_BUILTIN_OBJECT, "Object", "preventExtensions",
     code(KILN_C99_METHOD_OBJECT_PREVENT_EXTENSIONS)),
    (KILN_C99_BUILTIN_OBJECT, "Object", "isExtensible",
     code(KILN_C99_METHOD_OBJECT_IS_EXTENSIBLE)),
    (KILN_C99_BUILTIN_OBJECT, "Object", "entries", code(KILN_C99_METHOD_OBJECT_ENTRIES)),
    (KILN_C99_BUILTIN_OBJECT, "Object", "values", code(KILN_C99_METHOD_OBJECT_VALUES)),
    (KILN_C99_BUILTIN_OBJECT, "Object", "freeze", code(KILN_C99_METHOD_OBJECT_FREEZE)),
    (KILN_C99_BUILTIN_OBJECT, "Object", "isFrozen", code(KILN_C99_METHOD_OBJECT_IS_FROZEN)),
    (KILN_C99_BUILTIN_OBJECT, "Object", "create", code(KILN_C99_METHOD_OBJECT_CREATE)),
    (KILN_C99_BUILTIN_OBJECT, "Object", "defineProperty",
     code(KILN_C99_METHOD_OBJECT_DEFINE_PROPERTY)),
    (KILN_C99_BUILTIN_OBJECT, "Object", "fromEntries", code(KILN_C99_METHOD_OBJECT_FROM_ENTRIES)),
    (KILN_C99_BUILTIN_OBJECT, "Object", "hasOwn", code(KILN_C99_METHOD_HAS_OWN)),
    (KILN_C99_BUILTIN_OBJECT, "Object", "is", code(KILN_C99_METHOD_OBJECT_IS)),
    (KILN_C99_BUILTIN_OBJECT, "Object", "getOwnPropertySymbols",
     code(KILN_C99_METHOD_OBJECT_GET_OWN_PROPERTY_SYMBOLS)),
    (KILN_C99_BUILTIN_OBJECT, "Object", "defineProperties",
     code(KILN_C99_METHOD_OBJECT_DEFINE_PROPERTIES)),
    (KILN_C99_BUILTIN_OBJECT, "Object", "getOwnPropertyDescriptor",
     code(KILN_C99_METHOD_OBJECT_GET_OWN_PROPERTY_DESCRIPTOR)),
    (KILN_C99_BUILTIN_OBJECT, "Object", "getOwnPropertyDescriptors",
     code(KILN_C99_METHOD_OBJECT_GET_OWN_PROPERTY_DESCRIPTORS)),
    (KILN_C99_BUILTIN_OBJECT, "Object", "setPrototypeOf",
     code(KILN_C99_METHOD_OBJECT_SET_PROTOTYPE_OF)),
    (KILN_C99_BUILTIN_OBJECT, "Object", "getPrototypeOf",
     code(KILN_C99_METHOD_OBJECT_GET_PROTOTYPE_OF)),

    (KILN_C99_BUILTIN_ARRAY, "Array", "isArray", code(KILN_C99_METHOD_IS_ARRAY)),
    (KILN_C99_BUILTIN_ARRAY, "Array", "from", code(KILN_C99_METHOD_ARRAY_FROM)),
    (KILN_C99_BUILTIN_ARRAY, "Array", "of", code(KILN_C99_METHOD_ARRAY_OF)),
    // `Array[Symbol.species]` is `Array`. It read `undefined` natively and a function in the
    // interpreter, with no refusal in between -- the silent half of the drift, and the exact
    // shape `typeof X === "undefined"` feature-detection walks into.
    (KILN_C99_BUILTIN_ARRAY, "Array", "@@species", ctor(KILN_C99_BUILTIN_ARRAY)),
    // The same accessor on every other constructor the interpreter defines it for
    // (`defines_species`). Until 2026-09-19 each of these refused natively and the refusal was
    // silenced into `undefined` one frame up, in the symbol catch-all `kiln_c99_get_key` held.
    (KILN_C99_BUILTIN_MAP, "Map", "@@species", ctor(KILN_C99_BUILTIN_MAP)),
    (KILN_C99_BUILTIN_PROMISE, "Promise", "@@species", ctor(KILN_C99_BUILTIN_PROMISE)),
    (KILN_C99_BUILTIN_REGEXP, "RegExp", "@@species", ctor(KILN_C99_BUILTIN_REGEXP)),
    (KILN_C99_BUILTIN_ARRAY_BUFFER, "ArrayBuffer", "@@species", ctor(KILN_C99_BUILTIN_ARRAY_BUFFER)),

    (KILN_C99_BUILTIN_STRING, "String", "raw", code(KILN_C99_METHOD_STRING_RAW)),
    (KILN_C99_BUILTIN_STRING, "String", "fromCharCode", code(KILN_C99_METHOD_STRING_FROM_CHAR_CODE)),
    (KILN_C99_BUILTIN_STRING, "String", "fromCodePoint", code(KILN_C99_METHOD_STRING_FROM_CODE_POINT)),

    (KILN_C99_BUILTIN_NUMBER, "Number", "isInteger", code(KILN_C99_METHOD_IS_INTEGER)),
    (KILN_C99_BUILTIN_NUMBER, "Number", "isSafeInteger", code(KILN_C99_METHOD_IS_SAFE_INTEGER)),
    (KILN_C99_BUILTIN_NUMBER, "Number", "isNaN", code(KILN_C99_METHOD_IS_NAN)),
    (KILN_C99_BUILTIN_NUMBER, "Number", "isFinite", code(KILN_C99_METHOD_IS_FINITE)),
    // `Number.parseInt` and the global `parseInt` are the same function in the language, so they
    // are the same code here.
    (KILN_C99_BUILTIN_NUMBER, "Number", "parseInt", code(KILN_C99_METHOD_PARSE_INT)),
    (KILN_C99_BUILTIN_NUMBER, "Number", "parseFloat", code(KILN_C99_METHOD_PARSE_FLOAT)),
    (KILN_C99_BUILTIN_NUMBER, "Number", "MAX_SAFE_INTEGER", num("9007199254740991.0")),
    (KILN_C99_BUILTIN_NUMBER, "Number", "MIN_SAFE_INTEGER", num("-9007199254740991.0")),
    (KILN_C99_BUILTIN_NUMBER, "Number", "MAX_VALUE", num("1.7976931348623157e308")),
    (KILN_C99_BUILTIN_NUMBER, "Number", "MIN_VALUE", num("5e-324")),
    (KILN_C99_BUILTIN_NUMBER, "Number", "EPSILON", num("2.220446049250313e-16")),
    (KILN_C99_BUILTIN_NUMBER, "Number", "POSITIVE_INFINITY", num("INFINITY")),
    (KILN_C99_BUILTIN_NUMBER, "Number", "NEGATIVE_INFINITY", num("-INFINITY")),
    (KILN_C99_BUILTIN_NUMBER, "Number", "NaN", num("NAN")),

    (KILN_C99_BUILTIN_DATE, "Date", "now", code(KILN_C99_METHOD_DATE_NOW)),
    (KILN_C99_BUILTIN_DATE, "Date", "UTC", code(KILN_C99_METHOD_DATE_UTC)),
    (KILN_C99_BUILTIN_DATE, "Date", "parse", code(KILN_C99_METHOD_DATE_PARSE)),

    (KILN_C99_BUILTIN_PROMISE, "Promise", "resolve", code(KILN_C99_METHOD_PROMISE_RESOLVE)),
    (KILN_C99_BUILTIN_PROMISE, "Promise", "reject", code(KILN_C99_METHOD_PROMISE_REJECT)),
    (KILN_C99_BUILTIN_PROMISE, "Promise", "all", code(KILN_C99_METHOD_PROMISE_ALL)),
    (KILN_C99_BUILTIN_PROMISE, "Promise", "allSettled", code(KILN_C99_METHOD_PROMISE_ALL_SETTLED)),
    (KILN_C99_BUILTIN_PROMISE, "Promise", "race", code(KILN_C99_METHOD_PROMISE_RACE)),
    (KILN_C99_BUILTIN_PROMISE, "Promise", "any", code(KILN_C99_METHOD_PROMISE_ANY)),
    (KILN_C99_BUILTIN_PROMISE, "Promise", "withResolvers",
     code(KILN_C99_METHOD_PROMISE_WITH_RESOLVERS)),

    (KILN_C99_BUILTIN_JSON, "JSON", "parse", code(KILN_C99_METHOD_JSON_PARSE)),
    (KILN_C99_BUILTIN_JSON, "JSON", "stringify", code(KILN_C99_METHOD_JSON_STRINGIFY)),
    (KILN_C99_BUILTIN_JSON, "JSON", "@@toStringTag", text("JSON")),

    (KILN_C99_BUILTIN_REFLECT, "Reflect", "defineProperty",
     code(KILN_C99_METHOD_REFLECT_DEFINE_PROPERTY)),
    (KILN_C99_BUILTIN_REFLECT, "Reflect", "deleteProperty",
     code(KILN_C99_METHOD_REFLECT_DELETE_PROPERTY)),
    (KILN_C99_BUILTIN_REFLECT, "Reflect", "apply", code(KILN_C99_METHOD_REFLECT_APPLY)),
    (KILN_C99_BUILTIN_REFLECT, "Reflect", "construct", code(KILN_C99_METHOD_REFLECT_CONSTRUCT)),
    (KILN_C99_BUILTIN_REFLECT, "Reflect", "get", code(KILN_C99_METHOD_REFLECT_GET)),
    // Really the same operation `Object` carries, so it is the same code; the rest differ from
    // their `Object` namesakes in what they answer and are their own.
    (KILN_C99_BUILTIN_REFLECT, "Reflect", "getOwnPropertyDescriptor",
     code(KILN_C99_METHOD_OBJECT_GET_OWN_PROPERTY_DESCRIPTOR)),
    (KILN_C99_BUILTIN_REFLECT, "Reflect", "getPrototypeOf",
     code(KILN_C99_METHOD_REFLECT_GET_PROTOTYPE_OF)),
    (KILN_C99_BUILTIN_REFLECT, "Reflect", "has", code(KILN_C99_METHOD_REFLECT_HAS)),
    (KILN_C99_BUILTIN_REFLECT, "Reflect", "ownKeys", code(KILN_C99_METHOD_REFLECT_OWN_KEYS)),
    (KILN_C99_BUILTIN_REFLECT, "Reflect", "set", code(KILN_C99_METHOD_REFLECT_SET)),
    (KILN_C99_BUILTIN_REFLECT, "Reflect", "@@toStringTag", text("Reflect")),

    (KILN_C99_BUILTIN_PROXY, "Proxy", "revocable", code(KILN_C99_METHOD_PROXY_REVOCABLE)),

    (KILN_C99_BUILTIN_ARRAY_BUFFER, "ArrayBuffer", "isView",
     code(KILN_C99_METHOD_ARRAY_BUFFER_IS_VIEW)),

    (KILN_C99_BUILTIN_PERFORMANCE, "performance", "now",
     code(KILN_C99_METHOD_PERFORMANCE_NOW)),

    // Every error constructor carries `captureStackTrace`, which `kiln_c99_namespace_method`
    // answers ahead of the table rather than repeating in a row per kind. The row is here so that
    // `Object.getOwnPropertyNames(Error)` and the drift test can both see it.
    (KILN_C99_BUILTIN_ERROR, "Error", "captureStackTrace",
     code(KILN_C99_METHOD_ERROR_CAPTURE_STACK_TRACE)),

    (KILN_C99_BUILTIN_SYMBOL, "Symbol", "for", code(KILN_C99_METHOD_SYMBOL_FOR)),
    (KILN_C99_BUILTIN_SYMBOL, "Symbol", "keyFor", code(KILN_C99_METHOD_SYMBOL_KEY_FOR)),
    (KILN_C99_BUILTIN_SYMBOL, "Symbol", "asyncIterator", sym("@@asyncIterator")),
    (KILN_C99_BUILTIN_SYMBOL, "Symbol", "hasInstance", sym("@@hasInstance")),
    (KILN_C99_BUILTIN_SYMBOL, "Symbol", "isConcatSpreadable", sym("@@isConcatSpreadable")),
    (KILN_C99_BUILTIN_SYMBOL, "Symbol", "iterator", sym("@@iterator")),
    (KILN_C99_BUILTIN_SYMBOL, "Symbol", "match", sym("@@match")),
    (KILN_C99_BUILTIN_SYMBOL, "Symbol", "matchAll", sym("@@matchAll")),
    (KILN_C99_BUILTIN_SYMBOL, "Symbol", "replace", sym("@@replace")),
    (KILN_C99_BUILTIN_SYMBOL, "Symbol", "search", sym("@@search")),
    (KILN_C99_BUILTIN_SYMBOL, "Symbol", "species", sym("@@species")),
    (KILN_C99_BUILTIN_SYMBOL, "Symbol", "split", sym("@@split")),
    (KILN_C99_BUILTIN_SYMBOL, "Symbol", "toPrimitive", sym("@@toPrimitive")),
    (KILN_C99_BUILTIN_SYMBOL, "Symbol", "toStringTag", sym("@@toStringTag")),
    (KILN_C99_BUILTIN_SYMBOL, "Symbol", "unscopables", sym("@@unscopables")),
    (KILN_C99_BUILTIN_SYMBOL, "Symbol", "dispose", sym("@@dispose")),
    (KILN_C99_BUILTIN_SYMBOL, "Symbol", "asyncDispose", sym("@@asyncDispose")),

    /* `Iterator.from` is the whole of the constructor's answered surface, and it is the member a
     * bundled `immer` reads. `concat`, `zip` and `zipKeyed` are still backlog below: each builds
     * a helper over *several* sources, which is a different shape from the one-source helpers
     * `Iterator.prototype` hands out. */
    (KILN_C99_BUILTIN_ITERATOR, "Iterator", "from", code(KILN_C99_METHOD_ITERATOR_FROM)),

    (KILN_C99_BUILTIN_BUFFER, "Buffer", "from", code(KILN_C99_METHOD_BUFFER_FROM)),
    (KILN_C99_BUILTIN_BUFFER, "Buffer", "alloc", code(KILN_C99_METHOD_BUFFER_ALLOC)),
    // Three names for one operation: nothing here hands back uninitialised memory, so the
    // "unsafe" spellings are the safe one under another name.
    (KILN_C99_BUILTIN_BUFFER, "Buffer", "allocUnsafe", code(KILN_C99_METHOD_BUFFER_ALLOC)),
    (KILN_C99_BUILTIN_BUFFER, "Buffer", "allocUnsafeSlow", code(KILN_C99_METHOD_BUFFER_ALLOC)),
    (KILN_C99_BUILTIN_BUFFER, "Buffer", "isBuffer", code(KILN_C99_METHOD_BUFFER_IS_BUFFER)),
    (KILN_C99_BUILTIN_BUFFER, "Buffer", "byteLength", code(KILN_C99_METHOD_BUFFER_BYTE_LENGTH)),
    (KILN_C99_BUILTIN_BUFFER, "Buffer", "concat", code(KILN_C99_METHOD_BUFFER_CONCAT)),

    // Not tabulated by the interpreter, so the drift test reports it as unchecked rather than
    // passing it silently. The rows still make it self-describing.
    (KILN_C99_BUILTIN_ABORT_SIGNAL, "AbortSignal", "abort",
     code(KILN_C99_METHOD_ABORT_SIGNAL_ABORT)),
    (KILN_C99_BUILTIN_ABORT_SIGNAL, "AbortSignal", "timeout",
     code(KILN_C99_METHOD_ABORT_SIGNAL_TIMEOUT)),
    (KILN_C99_BUILTIN_ABORT_SIGNAL, "AbortSignal", "any", code(KILN_C99_METHOD_ABORT_SIGNAL_ANY)),

    (KILN_C99_BUILTIN_MATH, "Math", "abs", code(KILN_C99_METHOD_MATH_ABS)),
    (KILN_C99_BUILTIN_MATH, "Math", "floor", code(KILN_C99_METHOD_MATH_FLOOR)),
    (KILN_C99_BUILTIN_MATH, "Math", "ceil", code(KILN_C99_METHOD_MATH_CEIL)),
    (KILN_C99_BUILTIN_MATH, "Math", "round", code(KILN_C99_METHOD_MATH_ROUND)),
    (KILN_C99_BUILTIN_MATH, "Math", "trunc", code(KILN_C99_METHOD_MATH_TRUNC)),
    (KILN_C99_BUILTIN_MATH, "Math", "sign", code(KILN_C99_METHOD_MATH_SIGN)),
    (KILN_C99_BUILTIN_MATH, "Math", "sqrt", code(KILN_C99_METHOD_MATH_SQRT)),
    (KILN_C99_BUILTIN_MATH, "Math", "cbrt", code(KILN_C99_METHOD_MATH_CBRT)),
    (KILN_C99_BUILTIN_MATH, "Math", "random", code(KILN_C99_METHOD_MATH_RANDOM)),
    (KILN_C99_BUILTIN_MATH, "Math", "sinh", code(KILN_C99_METHOD_MATH_SINH)),
    (KILN_C99_BUILTIN_MATH, "Math", "cosh", code(KILN_C99_METHOD_MATH_COSH)),
    (KILN_C99_BUILTIN_MATH, "Math", "tanh", code(KILN_C99_METHOD_MATH_TANH)),
    (KILN_C99_BUILTIN_MATH, "Math", "asinh", code(KILN_C99_METHOD_MATH_ASINH)),
    (KILN_C99_BUILTIN_MATH, "Math", "acosh", code(KILN_C99_METHOD_MATH_ACOSH)),
    (KILN_C99_BUILTIN_MATH, "Math", "atanh", code(KILN_C99_METHOD_MATH_ATANH)),
    (KILN_C99_BUILTIN_MATH, "Math", "log1p", code(KILN_C99_METHOD_MATH_LOG1P)),
    (KILN_C99_BUILTIN_MATH, "Math", "expm1", code(KILN_C99_METHOD_MATH_EXPM1)),
    (KILN_C99_BUILTIN_MATH, "Math", "fround", code(KILN_C99_METHOD_MATH_FROUND)),
    (KILN_C99_BUILTIN_MATH, "Math", "clz32", code(KILN_C99_METHOD_MATH_CLZ32)),
    (KILN_C99_BUILTIN_MATH, "Math", "imul", code(KILN_C99_METHOD_MATH_IMUL)),
    (KILN_C99_BUILTIN_MATH, "Math", "hypot", code(KILN_C99_METHOD_MATH_HYPOT)),
    (KILN_C99_BUILTIN_MATH, "Math", "atan2", code(KILN_C99_METHOD_MATH_ATAN2)),
    (KILN_C99_BUILTIN_MATH, "Math", "sin", code(KILN_C99_METHOD_MATH_SIN)),
    (KILN_C99_BUILTIN_MATH, "Math", "cos", code(KILN_C99_METHOD_MATH_COS)),
    (KILN_C99_BUILTIN_MATH, "Math", "tan", code(KILN_C99_METHOD_MATH_TAN)),
    (KILN_C99_BUILTIN_MATH, "Math", "asin", code(KILN_C99_METHOD_MATH_ASIN)),
    (KILN_C99_BUILTIN_MATH, "Math", "acos", code(KILN_C99_METHOD_MATH_ACOS)),
    (KILN_C99_BUILTIN_MATH, "Math", "atan", code(KILN_C99_METHOD_MATH_ATAN)),
    (KILN_C99_BUILTIN_MATH, "Math", "pow", code(KILN_C99_METHOD_MATH_POW)),
    (KILN_C99_BUILTIN_MATH, "Math", "min", code(KILN_C99_METHOD_MATH_MIN)),
    (KILN_C99_BUILTIN_MATH, "Math", "max", code(KILN_C99_METHOD_MATH_MAX)),
    (KILN_C99_BUILTIN_MATH, "Math", "log", code(KILN_C99_METHOD_MATH_LOG)),
    (KILN_C99_BUILTIN_MATH, "Math", "log2", code(KILN_C99_METHOD_MATH_LOG2)),
    (KILN_C99_BUILTIN_MATH, "Math", "log10", code(KILN_C99_METHOD_MATH_LOG10)),
    (KILN_C99_BUILTIN_MATH, "Math", "exp", code(KILN_C99_METHOD_MATH_EXP)),
    (KILN_C99_BUILTIN_MATH, "Math", "PI", num("3.14159265358979311600")),
    (KILN_C99_BUILTIN_MATH, "Math", "E", num("2.71828182845904509080")),
    (KILN_C99_BUILTIN_MATH, "Math", "LN2", num("0.69314718055994530942")),
    (KILN_C99_BUILTIN_MATH, "Math", "LN10", num("2.30258509299404568402")),
    (KILN_C99_BUILTIN_MATH, "Math", "SQRT2", num("1.41421356237309504880")),
    (KILN_C99_BUILTIN_MATH, "Math", "SQRT1_2", num("0.70710678118654752440")),
    (KILN_C99_BUILTIN_MATH, "Math", "LOG2E", num("1.44269504088896340736")),
    (KILN_C99_BUILTIN_MATH, "Math", "LOG10E", num("0.43429448190325182765")),
    (KILN_C99_BUILTIN_MATH, "Math", "@@toStringTag", text("Math")),

    (KILN_C99_BUILTIN_SYMBOL_PROTOTYPE, "Symbol.prototype", "toString",
     code(KILN_C99_METHOD_PRIMITIVE_TO_STRING)),
    (KILN_C99_BUILTIN_SYMBOL_PROTOTYPE, "Symbol.prototype", "valueOf",
     code(KILN_C99_METHOD_SYMBOL_VALUE_OF)),
    (KILN_C99_BUILTIN_SYMBOL_PROTOTYPE, "Symbol.prototype", "constructor",
     ctor(KILN_C99_BUILTIN_SYMBOL)),
    (KILN_C99_BUILTIN_SYMBOL_PROTOTYPE, "Symbol.prototype", "@@toStringTag",
     text("Symbol")),

    (KILN_C99_BUILTIN_ARRAY_PROTOTYPE, "Array.prototype", "length", num("0.0")),
    (KILN_C99_BUILTIN_ARRAY_PROTOTYPE, "Array.prototype", "constructor", ctor(KILN_C99_BUILTIN_ARRAY)),
    (KILN_C99_BUILTIN_ARRAY_PROTOTYPE, "Array.prototype", "map", code(KILN_C99_METHOD_ARRAY_GENERIC_MAP)),
    (KILN_C99_BUILTIN_ARRAY_PROTOTYPE, "Array.prototype", "parallelMap", code(KILN_C99_METHOD_ARRAY_PARALLEL_MAP),
     kiln_extension),
    (KILN_C99_BUILTIN_ARRAY_PROTOTYPE, "Array.prototype", "forEach", code(KILN_C99_METHOD_FOR_EACH)),
    (KILN_C99_BUILTIN_ARRAY_PROTOTYPE, "Array.prototype", "filter", code(KILN_C99_METHOD_ARRAY_GENERIC_FILTER)),
    (KILN_C99_BUILTIN_ARRAY_PROTOTYPE, "Array.prototype", "reduce", code(KILN_C99_METHOD_REDUCE)),
    (KILN_C99_BUILTIN_ARRAY_PROTOTYPE, "Array.prototype", "sort", code(KILN_C99_METHOD_ARRAY_SORT)),
    (KILN_C99_BUILTIN_ARRAY_PROTOTYPE, "Array.prototype", "findIndex", code(KILN_C99_METHOD_ARRAY_FIND_INDEX)),
    (KILN_C99_BUILTIN_ARRAY_PROTOTYPE, "Array.prototype", "find", code(KILN_C99_METHOD_ARRAY_FIND)),
    (KILN_C99_BUILTIN_ARRAY_PROTOTYPE, "Array.prototype", "some", code(KILN_C99_METHOD_ARRAY_SOME)),
    (KILN_C99_BUILTIN_ARRAY_PROTOTYPE, "Array.prototype", "every", code(KILN_C99_METHOD_ARRAY_EVERY)),
    (KILN_C99_BUILTIN_ARRAY_PROTOTYPE, "Array.prototype", "flatMap", code(KILN_C99_METHOD_ARRAY_FLAT_MAP)),
    (KILN_C99_BUILTIN_ARRAY_PROTOTYPE, "Array.prototype", "findLast", code(KILN_C99_METHOD_ARRAY_FIND_LAST)),
    (KILN_C99_BUILTIN_ARRAY_PROTOTYPE, "Array.prototype", "findLastIndex", code(KILN_C99_METHOD_ARRAY_FIND_LAST_INDEX)),
    (KILN_C99_BUILTIN_ARRAY_PROTOTYPE, "Array.prototype", "reduceRight", code(KILN_C99_METHOD_ARRAY_REDUCE_RIGHT)),
    (KILN_C99_BUILTIN_ARRAY_PROTOTYPE, "Array.prototype", "join", code(KILN_C99_METHOD_JOIN)),
    (KILN_C99_BUILTIN_ARRAY_PROTOTYPE, "Array.prototype", "push", code(KILN_C99_METHOD_PUSH)),
    (KILN_C99_BUILTIN_ARRAY_PROTOTYPE, "Array.prototype", "pop", code(KILN_C99_METHOD_POP)),
    (KILN_C99_BUILTIN_ARRAY_PROTOTYPE, "Array.prototype", "indexOf", code(KILN_C99_METHOD_INDEX_OF)),
    (KILN_C99_BUILTIN_ARRAY_PROTOTYPE, "Array.prototype", "includes", code(KILN_C99_METHOD_INCLUDES)),
    (KILN_C99_BUILTIN_ARRAY_PROTOTYPE, "Array.prototype", "slice", code(KILN_C99_METHOD_SLICE)),
    (KILN_C99_BUILTIN_ARRAY_PROTOTYPE, "Array.prototype", "at", code(KILN_C99_METHOD_ARRAY_AT)),
    (KILN_C99_BUILTIN_ARRAY_PROTOTYPE, "Array.prototype", "flat", code(KILN_C99_METHOD_ARRAY_FLAT)),
    (KILN_C99_BUILTIN_ARRAY_PROTOTYPE, "Array.prototype", "entries", code(KILN_C99_METHOD_ARRAY_ENTRIES)),
    (KILN_C99_BUILTIN_ARRAY_PROTOTYPE, "Array.prototype", "keys", code(KILN_C99_METHOD_ARRAY_KEYS)),
    (KILN_C99_BUILTIN_ARRAY_PROTOTYPE, "Array.prototype", "concat", code(KILN_C99_METHOD_CONCAT)),
    (KILN_C99_BUILTIN_ARRAY_PROTOTYPE, "Array.prototype", "reverse", code(KILN_C99_METHOD_ARRAY_REVERSE)),
    (KILN_C99_BUILTIN_ARRAY_PROTOTYPE, "Array.prototype", "shift", code(KILN_C99_METHOD_ARRAY_SHIFT)),
    (KILN_C99_BUILTIN_ARRAY_PROTOTYPE, "Array.prototype", "unshift", code(KILN_C99_METHOD_ARRAY_UNSHIFT)),
    (KILN_C99_BUILTIN_ARRAY_PROTOTYPE, "Array.prototype", "lastIndexOf", code(KILN_C99_METHOD_ARRAY_LAST_INDEX_OF)),
    (KILN_C99_BUILTIN_ARRAY_PROTOTYPE, "Array.prototype", "fill", code(KILN_C99_METHOD_ARRAY_FILL)),
    (KILN_C99_BUILTIN_ARRAY_PROTOTYPE, "Array.prototype", "splice", code(KILN_C99_METHOD_ARRAY_SPLICE)),
    (KILN_C99_BUILTIN_ARRAY_PROTOTYPE, "Array.prototype", "copyWithin", code(KILN_C99_METHOD_ARRAY_COPY_WITHIN)),
    (KILN_C99_BUILTIN_ARRAY_PROTOTYPE, "Array.prototype", "toString", code(KILN_C99_METHOD_ARRAY_TO_STRING)),
    (KILN_C99_BUILTIN_ARRAY_PROTOTYPE, "Array.prototype", "toLocaleString", code(KILN_C99_METHOD_OBJECT_TO_LOCALE_STRING)),
    (KILN_C99_BUILTIN_ARRAY_PROTOTYPE, "Array.prototype", "values", code(KILN_C99_METHOD_ARRAY_VALUES)),
    (KILN_C99_BUILTIN_ARRAY_PROTOTYPE, "Array.prototype", "with", code(KILN_C99_METHOD_ARRAY_WITH)),
    (KILN_C99_BUILTIN_ARRAY_PROTOTYPE, "Array.prototype", "toReversed", code(KILN_C99_METHOD_ARRAY_TO_REVERSED)),
    (KILN_C99_BUILTIN_ARRAY_PROTOTYPE, "Array.prototype", "toSorted", code(KILN_C99_METHOD_ARRAY_TO_SORTED)),
    (KILN_C99_BUILTIN_ARRAY_PROTOTYPE, "Array.prototype", "toSpliced", code(KILN_C99_METHOD_ARRAY_TO_SPLICED)),
    (KILN_C99_BUILTIN_ARRAY_PROTOTYPE, "Array.prototype", "@@iterator", code(KILN_C99_METHOD_ARRAY_VALUES)),
    (KILN_C99_BUILTIN_ARRAY_PROTOTYPE, "Array.prototype", "@@unscopables", unscopables()),

    (KILN_C99_BUILTIN_NUMBER_PROTOTYPE, "Number.prototype", "constructor", ctor(KILN_C99_BUILTIN_NUMBER)),
    (KILN_C99_BUILTIN_NUMBER_PROTOTYPE, "Number.prototype", "toExponential", code(KILN_C99_METHOD_NUMBER_TO_EXPONENTIAL)),
    (KILN_C99_BUILTIN_NUMBER_PROTOTYPE, "Number.prototype", "toFixed", code(KILN_C99_METHOD_NUMBER_TO_FIXED)),
    (KILN_C99_BUILTIN_NUMBER_PROTOTYPE, "Number.prototype", "toPrecision", code(KILN_C99_METHOD_NUMBER_TO_PRECISION)),
    (KILN_C99_BUILTIN_NUMBER_PROTOTYPE, "Number.prototype", "toString", code(KILN_C99_METHOD_NUMBER_TO_STRING)),
    (KILN_C99_BUILTIN_NUMBER_PROTOTYPE, "Number.prototype", "valueOf", code(KILN_C99_METHOD_NUMBER_VALUE_OF)),
    (KILN_C99_BUILTIN_NUMBER_PROTOTYPE, "Number.prototype", "toLocaleString", code(KILN_C99_METHOD_OBJECT_TO_LOCALE_STRING)),

    (KILN_C99_BUILTIN_BOOLEAN_PROTOTYPE, "Boolean.prototype", "constructor", ctor(KILN_C99_BUILTIN_BOOLEAN)),
    (KILN_C99_BUILTIN_BOOLEAN_PROTOTYPE, "Boolean.prototype", "toString", code(KILN_C99_METHOD_PRIMITIVE_TO_STRING)),
    (KILN_C99_BUILTIN_BOOLEAN_PROTOTYPE, "Boolean.prototype", "valueOf", code(KILN_C99_METHOD_BOOLEAN_VALUE_OF)),

    (KILN_C99_BUILTIN_BIGINT_PROTOTYPE, "BigInt.prototype", "constructor", ctor(KILN_C99_BUILTIN_BIGINT)),
    (KILN_C99_BUILTIN_BIGINT_PROTOTYPE, "BigInt.prototype", "toString", code(KILN_C99_METHOD_NUMBER_TO_STRING)),
    (KILN_C99_BUILTIN_BIGINT_PROTOTYPE, "BigInt.prototype", "toLocaleString", code(KILN_C99_METHOD_OBJECT_TO_LOCALE_STRING)),
    (KILN_C99_BUILTIN_BIGINT_PROTOTYPE, "BigInt.prototype", "valueOf", code(KILN_C99_METHOD_NUMBER_VALUE_OF)),
    (KILN_C99_BUILTIN_BIGINT_PROTOTYPE, "BigInt.prototype", "@@toStringTag", text("BigInt")),

    (KILN_C99_BUILTIN_PROMISE_PROTOTYPE, "Promise.prototype", "constructor", ctor(KILN_C99_BUILTIN_PROMISE)),
    (KILN_C99_BUILTIN_PROMISE_PROTOTYPE, "Promise.prototype", "then", code(KILN_C99_METHOD_THEN)),
    (KILN_C99_BUILTIN_PROMISE_PROTOTYPE, "Promise.prototype", "catch", code(KILN_C99_METHOD_CATCH)),
    (KILN_C99_BUILTIN_PROMISE_PROTOTYPE, "Promise.prototype", "finally", code(KILN_C99_METHOD_FINALLY)),
    (KILN_C99_BUILTIN_PROMISE_PROTOTYPE, "Promise.prototype", "@@toStringTag", text("Promise")),

    (KILN_C99_BUILTIN_OBJECT_PROTOTYPE, "Object.prototype", "constructor", ctor(KILN_C99_BUILTIN_OBJECT)),
    (KILN_C99_BUILTIN_OBJECT_PROTOTYPE, "Object.prototype", "hasOwnProperty", code(KILN_C99_METHOD_HAS_OWN_PROPERTY)),
    (KILN_C99_BUILTIN_OBJECT_PROTOTYPE, "Object.prototype", "isPrototypeOf", code(KILN_C99_METHOD_IS_PROTOTYPE_OF)),
    (KILN_C99_BUILTIN_OBJECT_PROTOTYPE, "Object.prototype", "propertyIsEnumerable", code(KILN_C99_METHOD_PROPERTY_IS_ENUMERABLE)),
    (KILN_C99_BUILTIN_OBJECT_PROTOTYPE, "Object.prototype", "toLocaleString", code(KILN_C99_METHOD_OBJECT_TO_LOCALE_STRING)),
    (KILN_C99_BUILTIN_OBJECT_PROTOTYPE, "Object.prototype", "toString", code(KILN_C99_METHOD_OBJECT_TO_STRING)),
    (KILN_C99_BUILTIN_OBJECT_PROTOTYPE, "Object.prototype", "valueOf", code(KILN_C99_METHOD_OBJECT_VALUE_OF)),

    (KILN_C99_BUILTIN_FUNCTION_PROTOTYPE, "Function.prototype", "constructor", ctor(KILN_C99_BUILTIN_FUNCTION)),
    (KILN_C99_BUILTIN_FUNCTION_PROTOTYPE, "Function.prototype", "apply", code(KILN_C99_METHOD_FUNCTION_APPLY)),
    (KILN_C99_BUILTIN_FUNCTION_PROTOTYPE, "Function.prototype", "bind", code(KILN_C99_METHOD_FUNCTION_BIND)),
    (KILN_C99_BUILTIN_FUNCTION_PROTOTYPE, "Function.prototype", "call", code(KILN_C99_METHOD_FUNCTION_CALL)),
    (KILN_C99_BUILTIN_FUNCTION_PROTOTYPE, "Function.prototype", "toString", code(KILN_C99_METHOD_FUNCTION_TO_STRING)),

    (KILN_C99_BUILTIN_MAP_PROTOTYPE, "Map.prototype", "constructor", ctor(KILN_C99_BUILTIN_MAP)),
    (KILN_C99_BUILTIN_MAP_PROTOTYPE, "Map.prototype", "clear", code(KILN_C99_METHOD_COLLECTION_CLEAR)),
    (KILN_C99_BUILTIN_MAP_PROTOTYPE, "Map.prototype", "delete", code(KILN_C99_METHOD_COLLECTION_DELETE)),
    (KILN_C99_BUILTIN_MAP_PROTOTYPE, "Map.prototype", "entries", code(KILN_C99_METHOD_COLLECTION_ENTRIES)),
    (KILN_C99_BUILTIN_MAP_PROTOTYPE, "Map.prototype", "forEach", code(KILN_C99_METHOD_COLLECTION_FOR_EACH)),
    (KILN_C99_BUILTIN_MAP_PROTOTYPE, "Map.prototype", "get", code(KILN_C99_METHOD_COLLECTION_GET)),
    (KILN_C99_BUILTIN_MAP_PROTOTYPE, "Map.prototype", "has", code(KILN_C99_METHOD_COLLECTION_HAS)),
    (KILN_C99_BUILTIN_MAP_PROTOTYPE, "Map.prototype", "keys", code(KILN_C99_METHOD_COLLECTION_KEYS)),
    (KILN_C99_BUILTIN_MAP_PROTOTYPE, "Map.prototype", "set", code(KILN_C99_METHOD_COLLECTION_SET)),
    (KILN_C99_BUILTIN_MAP_PROTOTYPE, "Map.prototype", "size", accessor()),
    (KILN_C99_BUILTIN_MAP_PROTOTYPE, "Map.prototype", "values", code(KILN_C99_METHOD_COLLECTION_VALUES)),
    (KILN_C99_BUILTIN_MAP_PROTOTYPE, "Map.prototype", "@@iterator", code(KILN_C99_METHOD_COLLECTION_SELF)),
    (KILN_C99_BUILTIN_MAP_PROTOTYPE, "Map.prototype", "@@toStringTag", text("Map")),

    (KILN_C99_BUILTIN_SET_PROTOTYPE, "Set.prototype", "constructor", ctor(KILN_C99_BUILTIN_SET)),
    (KILN_C99_BUILTIN_SET_PROTOTYPE, "Set.prototype", "add", code(KILN_C99_METHOD_COLLECTION_ADD)),
    (KILN_C99_BUILTIN_SET_PROTOTYPE, "Set.prototype", "clear", code(KILN_C99_METHOD_COLLECTION_CLEAR)),
    (KILN_C99_BUILTIN_SET_PROTOTYPE, "Set.prototype", "delete", code(KILN_C99_METHOD_COLLECTION_DELETE)),
    (KILN_C99_BUILTIN_SET_PROTOTYPE, "Set.prototype", "entries", code(KILN_C99_METHOD_COLLECTION_ENTRIES)),
    (KILN_C99_BUILTIN_SET_PROTOTYPE, "Set.prototype", "forEach", code(KILN_C99_METHOD_COLLECTION_FOR_EACH)),
    (KILN_C99_BUILTIN_SET_PROTOTYPE, "Set.prototype", "has", code(KILN_C99_METHOD_COLLECTION_HAS)),
    (KILN_C99_BUILTIN_SET_PROTOTYPE, "Set.prototype", "keys", code(KILN_C99_METHOD_COLLECTION_KEYS)),
    (KILN_C99_BUILTIN_SET_PROTOTYPE, "Set.prototype", "size", accessor()),
    (KILN_C99_BUILTIN_SET_PROTOTYPE, "Set.prototype", "values", code(KILN_C99_METHOD_COLLECTION_VALUES)),
    (KILN_C99_BUILTIN_SET_PROTOTYPE, "Set.prototype", "@@iterator", code(KILN_C99_METHOD_COLLECTION_SELF)),
    (KILN_C99_BUILTIN_SET_PROTOTYPE, "Set.prototype", "@@toStringTag", text("Set")),

    (KILN_C99_BUILTIN_WEAK_MAP_PROTOTYPE, "WeakMap.prototype", "constructor", ctor(KILN_C99_BUILTIN_WEAK_MAP)),
    (KILN_C99_BUILTIN_WEAK_MAP_PROTOTYPE, "WeakMap.prototype", "delete", code(KILN_C99_METHOD_COLLECTION_DELETE)),
    (KILN_C99_BUILTIN_WEAK_MAP_PROTOTYPE, "WeakMap.prototype", "get", code(KILN_C99_METHOD_COLLECTION_GET)),
    (KILN_C99_BUILTIN_WEAK_MAP_PROTOTYPE, "WeakMap.prototype", "has", code(KILN_C99_METHOD_COLLECTION_HAS)),
    (KILN_C99_BUILTIN_WEAK_MAP_PROTOTYPE, "WeakMap.prototype", "set", code(KILN_C99_METHOD_COLLECTION_SET)),
    (KILN_C99_BUILTIN_WEAK_MAP_PROTOTYPE, "WeakMap.prototype", "@@toStringTag", text("WeakMap")),

    (KILN_C99_BUILTIN_WEAK_SET_PROTOTYPE, "WeakSet.prototype", "constructor", ctor(KILN_C99_BUILTIN_WEAK_SET)),
    (KILN_C99_BUILTIN_WEAK_SET_PROTOTYPE, "WeakSet.prototype", "add", code(KILN_C99_METHOD_COLLECTION_ADD)),
    (KILN_C99_BUILTIN_WEAK_SET_PROTOTYPE, "WeakSet.prototype", "delete", code(KILN_C99_METHOD_COLLECTION_DELETE)),
    (KILN_C99_BUILTIN_WEAK_SET_PROTOTYPE, "WeakSet.prototype", "has", code(KILN_C99_METHOD_COLLECTION_HAS)),
    (KILN_C99_BUILTIN_WEAK_SET_PROTOTYPE, "WeakSet.prototype", "@@toStringTag", text("WeakSet")),
}

/// A member the shared surface declares and this backend does not answer, with the reason.
///
/// The backlog is a list rather than a count on purpose. A gate that says "twenty-three gaps"
/// teaches people to ignore it; one that names them is a work queue, and each row is a sentence
/// somebody can act on or argue with. `builtin_surface_drift` in `kiln-native` prints them.
pub struct BuiltinGap {
    /// The global, spelled as [`kiln_builtin_surface::SURFACE`] spells it.
    pub global: &'static str,
    /// The member this backend does not answer.
    pub member: &'static str,
    /// Why it is still open. Never empty: a gap without a reason cannot be told from an oversight.
    pub why: &'static str,
}

/// Every member of the shared surface this backend does not implement.
///
/// Adding a member to `kiln_builtin_surface::SURFACE` is a compile error here until it appears
/// either in [`BUILTIN_SURFACE`] or in this list -- see [`assert_surface_is_covered`]. That is the
/// whole point: the decision to leave a member unimplemented becomes something somebody writes
/// down, rather than something a customer's bundle discovers.
/// The members this backend knows it does not answer, and why.
///
/// Declared through a macro for the same reason the surface is: the Rust list and the C table are
/// generated from one place, so they cannot disagree. The C side needs it because a built-in's
/// read path has to tell two things apart that look identical from inside the table -- a name the
/// language never had, which is `undefined`, and a member this backend has not built, which must
/// refuse by name. Without the backlog in C, answering `undefined` for an absent name would also
/// have answered it for `Object.groupBy`, and a silent wrong answer is worse than the refusal it
/// replaced.
macro_rules! builtin_backlog {
    ($(($global:literal, $member:literal, $why:literal)),* $(,)?) => {
        pub const BUILTIN_SURFACE_BACKLOG: &[BuiltinGap] = &[
            $(BuiltinGap { global: $global, member: $member, why: $why }),*
        ];
        macro_rules! emit_builtin_backlog_rows {
            () => { concat!($(concat!("    {\"", $global, "\", \"", $member, "\"},\n")),*) };
        }
    };
}

builtin_backlog! {
    ("Object.prototype", "__defineGetter__",
     "answered by the runtime bootstrap in TypeScript, from the prototype's store; no row"),
    ("Object.prototype", "__defineSetter__",
     "answered by the runtime bootstrap in TypeScript, from the prototype's store; no row"),
    ("Object.prototype", "__lookupGetter__",
     "answered by the runtime bootstrap in TypeScript, from the prototype's store; no row"),
    ("Object.prototype", "__lookupSetter__",
     "answered by the runtime bootstrap in TypeScript, from the prototype's store; no row"),
    ("Object.prototype", "__proto__",
     "legacy Annex B proto accessor handled via runtime link rather than static table"),
    ("Function.prototype", "@@hasInstance",
     "not implemented; ordinary instanceof check handled via runtime rather than method call"),
    ("Function.prototype", "caller",
     "legacy Annex B restricted accessor property throwing TypeError not modeled in native C backend"),
    ("Function.prototype", "arguments",
     "legacy Annex B restricted accessor property throwing TypeError not modeled in native C backend"),
    ("Map.prototype", "getOrInsert",
     "answered by the runtime bootstrap in TypeScript, from the prototype's store; no row"),
    ("Map.prototype", "getOrInsertComputed",
     "answered by the runtime bootstrap in TypeScript, from the prototype's store; no row"),
    ("Set.prototype", "union",
     "answered by the runtime bootstrap in TypeScript, from the prototype's store; no row"),
    ("Set.prototype", "intersection",
     "answered by the runtime bootstrap in TypeScript, from the prototype's store; no row"),
    ("Set.prototype", "difference",
     "answered by the runtime bootstrap in TypeScript, from the prototype's store; no row"),
    ("Set.prototype", "symmetricDifference",
     "answered by the runtime bootstrap in TypeScript, from the prototype's store; no row"),
    ("Set.prototype", "isSubsetOf",
     "answered by the runtime bootstrap in TypeScript, from the prototype's store; no row"),
    ("Set.prototype", "isSupersetOf",
     "answered by the runtime bootstrap in TypeScript, from the prototype's store; no row"),
    ("Set.prototype", "isDisjointFrom",
     "answered by the runtime bootstrap in TypeScript, from the prototype's store; no row"),
    ("WeakMap.prototype", "getOrInsert",
     "answered by the runtime bootstrap in TypeScript, from the prototype's store; no row"),
    ("WeakMap.prototype", "getOrInsertComputed",
     "answered by the runtime bootstrap in TypeScript, from the prototype's store; no row"),
    ("Object", "groupBy",
     "answered by the runtime bootstrap in TypeScript, from the constructor's store; no row"),
    ("Object", "seal",
     "answered by the runtime bootstrap in TypeScript, from the constructor's store; no row"),
    ("Object", "isSealed",
     "answered by the runtime bootstrap in TypeScript, from the constructor's store; no row"),
    ("Array", "fromAsync",
     "answered by the runtime bootstrap in TypeScript, from the constructor's store; no row"),
    ("Promise", "try",
     "answered by the runtime bootstrap in TypeScript, from the constructor's store; no row"),
    ("JSON", "rawJSON",
     "not implemented; the raw-JSON marker has no representation in this value kernel"),
    ("JSON", "isRawJSON",
     "not implemented; the raw-JSON marker has no representation in this value kernel"),
    ("Reflect", "setPrototypeOf",
     "answered by the runtime bootstrap in TypeScript, from the namespace's store; no row"),
    ("Reflect", "isExtensible",
     "answered by the runtime bootstrap in TypeScript, from the namespace's store; no row"),
    ("Reflect", "preventExtensions",
     "answered by the runtime bootstrap in TypeScript, from the namespace's store; no row"),
    ("RegExp", "escape",
     "answered by the runtime bootstrap in TypeScript, from the constructor's store; no row"),
    ("Error", "isError",
     "not implemented"),
    ("BigInt", "asIntN",
     "not implemented; needs a width-truncating call the BigInt provider does not export"),
    ("BigInt", "asUintN",
     "not implemented; needs a width-truncating call the BigInt provider does not export"),
    ("Map", "groupBy",
     "answered by the runtime bootstrap in TypeScript, from the constructor's store; no row"),
    ("Iterator", "concat",
     "answered by the runtime bootstrap in TypeScript, from the constructor's store; no row"),
    ("Iterator", "zip",
     "answered by the runtime bootstrap in TypeScript, from the constructor's store; no row"),
    ("Iterator", "zipKeyed",
     "answered by the runtime bootstrap in TypeScript, from the constructor's store; no row"),
    ("Buffer", "constants",
     "not implemented; the constants namespace object has no representation here"),
    ("Math", "f16round",
     "not implemented; needs a half-precision round the number provider does not export"),
    ("%TypedArray%", "@@species",
     "not implemented; the intrinsic is not a value this backend hands back"),
    ("Promise", "allKeyed",
     "answered by the runtime bootstrap in TypeScript, from the constructor's store; no row"),
    ("Promise", "allSettledKeyed",
     "answered by the runtime bootstrap in TypeScript, from the constructor's store; no row"),
    ("%TypedArrayConstructor%", "of",
     "answered from the receiver's element kind, not from a row"),
    ("%TypedArrayConstructor%", "from",
     "answered from the receiver's element kind, not from a row"),
    ("%TypedArrayConstructor%", "BYTES_PER_ELEMENT",
     "answered from the receiver's element kind, not from a row"),
    ("Uint8Array", "of",
     "answered from the receiver's element kind, not from a row"),
    ("Uint8Array", "from",
     "answered from the receiver's element kind, not from a row"),
    ("Uint8Array", "BYTES_PER_ELEMENT",
     "answered from the receiver's element kind, not from a row"),
    ("Uint8Array", "fromBase64",
     "answered by the shared typedarray.ts bootstrap, not from a row"),
    ("Uint8Array", "fromHex",
     "answered by the shared typedarray.ts bootstrap, not from a row"),
    ("Atomics", "add",
     "refused by name: shared memory has no representation here"),
    ("Atomics", "and",
     "refused by name: shared memory has no representation here"),
    ("Atomics", "compareExchange",
     "refused by name: shared memory has no representation here"),
    ("Atomics", "exchange",
     "refused by name: shared memory has no representation here"),
    ("Atomics", "isLockFree",
     "refused by name: shared memory has no representation here"),
    ("Atomics", "load",
     "refused by name: shared memory has no representation here"),
    ("Atomics", "notify",
     "refused by name: shared memory has no representation here"),
    ("Atomics", "or",
     "refused by name: shared memory has no representation here"),
    ("Atomics", "pause",
     "refused by name: shared memory has no representation here"),
    ("Atomics", "store",
     "refused by name: shared memory has no representation here"),
    ("Atomics", "sub",
     "refused by name: shared memory has no representation here"),
    ("Atomics", "wait",
     "refused by name: shared memory has no representation here"),
    ("Atomics", "waitAsync",
     "refused by name: shared memory has no representation here"),
    ("Atomics", "xor",
     "refused by name: shared memory has no representation here"),
    ("Atomics", "@@toStringTag",
     "refused by name: shared memory has no representation here"),
    ("Symbol.prototype", "description",
     "not implemented; needs an accessor method on native Symbol.prototype"),
    ("Symbol.prototype", "@@toPrimitive",
     "not implemented; needs @@toPrimitive method code"),
}

const fn surface_answers(surface: &[BuiltinMember], global: &str, member: &str) -> bool {
    let mut index = 0;
    while index < surface.len() {
        if method_name_is(surface[index].owner, global)
            && method_name_is(surface[index].name, member)
        {
            return true;
        }
        index += 1;
    }
    false
}

const fn backlog_names(backlog: &[BuiltinGap], global: &str, member: &str) -> bool {
    let mut index = 0;
    while index < backlog.len() {
        if method_name_is(backlog[index].global, global)
            && method_name_is(backlog[index].member, member)
        {
            return true;
        }
        index += 1;
    }
    false
}

/// Every member of the shared surface is either implemented here or written down as backlog, and
/// nothing here claims a member the shared surface does not declare.
///
/// THIS IS THE GATE, AND IT IS A COMPILE ERROR RATHER THAN A TEST FAILURE. A member added to
/// `kiln_builtin_surface::SURFACE` -- which is also where the interpreter reads its own lists --
/// does not build here until somebody decides what this backend does about it. That is the
/// difference between drift being structurally impossible and merely untested, which is the whole
/// of ADR 0043.
///
/// A `const` panic carries a literal and nothing else, so this says only *that* the two disagree.
/// `builtin_surface_drift` in `kiln-native` reads the same three tables and says which members,
/// with each backlog row's reason. The compiler is the gate; the test is the explanation.
const fn assert_surface_is_covered(surface: &[BuiltinMember], backlog: &[BuiltinGap]) {
    let shared = kiln_builtin_surface::SURFACE;
    let mut index = 0;
    while index < shared.len() {
        let members = shared[index].members;
        let mut member = 0;
        while member < members.len() {
            assert!(
                surface_answers(surface, shared[index].global, members[member])
                    || backlog_names(backlog, shared[index].global, members[member]),
                "a member of kiln_builtin_surface::SURFACE is neither in BUILTIN_SURFACE nor in \
                 BUILTIN_SURFACE_BACKLOG; run `mbx test -p kiln-native --test \
                 builtin_surface_drift` to be told which one"
            );
            member += 1;
        }
        index += 1;
    }

    // Check prototype surface coverage as well (ADR 0059)
    let proto_shared = kiln_builtin_surface::PROTOTYPE_SURFACE;
    let mut p_idx = 0;
    while p_idx < proto_shared.len() {
        let proto = proto_shared[p_idx].prototype;
        let member_key = proto_shared[p_idx].key.property_key();
        assert!(
            surface_answers(surface, proto.name(), member_key)
                || backlog_names(backlog, proto.name(), member_key),
            "a prototype member of kiln_builtin_surface::PROTOTYPE_SURFACE is neither in BUILTIN_SURFACE nor in BUILTIN_SURFACE_BACKLOG"
        );
        p_idx += 1;
    }

    // The other direction. A member only one engine has is drift whichever engine has it, and
    // this backend inventing one is the harder half to notice: nothing refuses, and the two
    // engines simply answer differently.
    index = 0;
    while index < surface.len() {
        let is_proto = kiln_builtin_surface::PrototypeId::from_name(surface[index].owner);
        if let Some(proto) = is_proto {
            assert!(
                kiln_builtin_surface::prototype_has(proto, surface[index].name),
                "BUILTIN_SURFACE answers a prototype member kiln_builtin_surface::PROTOTYPE_SURFACE does not declare"
            );
        } else {
            assert!(
                !kiln_builtin_surface::is_declared(surface[index].owner)
                    || kiln_builtin_surface::has(surface[index].owner, surface[index].name),
                "BUILTIN_SURFACE answers a member kiln_builtin_surface::SURFACE does not declare; \
                 either declare it there so both engines have it, or drop the row"
            );
        }
        index += 1;
    }

    // A kiln extension is marked on its row exactly when the shared description declares it one
    // (ADR 0068): the switch that removes them reads the row's mark, so a mark missing here would
    // leave the member installed under `KILN_EXTENSIONS=0`, and a stray one would remove a member
    // of the language.
    index = 0;
    while index < surface.len() {
        assert!(
            surface[index].kiln_extension
                == kiln_builtin_surface::is_kiln_extension(
                    surface[index].owner,
                    surface[index].name
                ),
            "a BUILTIN_SURFACE row's `kiln_extension` mark disagrees with \
             kiln_builtin_surface::KILN_EXTENSIONS"
        );
        index += 1;
    }

    // A backlog row that is no longer true is worse than no backlog: it describes a past that has
    // been fixed, and the next reader believes it.
    index = 0;
    while index < backlog.len() {
        assert!(
            !backlog[index].why.is_empty(),
            "a BUILTIN_SURFACE_BACKLOG row has no reason, which cannot be told from an oversight"
        );
        let is_proto = kiln_builtin_surface::PrototypeId::from_name(backlog[index].global);
        if let Some(proto) = is_proto {
            assert!(
                kiln_builtin_surface::prototype_has(proto, backlog[index].member),
                "a BUILTIN_SURFACE_BACKLOG row names a prototype member the shared surface does not declare"
            );
        } else {
            assert!(
                kiln_builtin_surface::is_declared(backlog[index].global)
                    && kiln_builtin_surface::has(backlog[index].global, backlog[index].member),
                "a BUILTIN_SURFACE_BACKLOG row names a member the shared surface does not declare"
            );
        }
        assert!(
            !surface_answers(surface, backlog[index].global, backlog[index].member),
            "a BUILTIN_SURFACE_BACKLOG row names a member BUILTIN_SURFACE now answers; delete the \
             backlog row"
        );
        index += 1;
    }
}

const _: () = assert_surface_is_covered(BUILTIN_SURFACE, BUILTIN_SURFACE_BACKLOG);

/// A prelude helper whose `KILN_C99_CONTROL_ERROR` is an *internal status* rather than a refusal.
///
/// The status is produced, handed to the caller named here, and converted there into something the
/// program sees. Nothing is ever reported as a refusal, so there is nothing for a detail to name.
pub(crate) struct InternalRefusal {
    /// The prelude function holding the sites.
    pub(crate) function: &'static str,
    /// How many `KILN_C99_REFUSE_INTERNAL` sites it holds.
    pub(crate) sites: usize,
    /// Which caller converts the status, and into what.
    pub(crate) converted_by: &'static str,
}

/// Every `KILN_C99_REFUSE_INTERNAL` in the emitted prelude, by the function that holds it.
///
/// This is the classification the inventory turns on, and it is deliberately small. `187 refusal
/// sites` overstates the backlog, because `KILN_C99_CONTROL_ERROR` does double duty -- but only
/// for the JSON reader, where it is measured rather than assumed: `JSON.parse` on `{bad`, `[1,`,
/// `nope`, `{"a":}` and `""` throws a `SyntaxError` in both engines, so not one of the reader's
/// exits is a gap a program can arrive at. Everything else in the prelude refuses to a program,
/// including the sites that only a broken invariant reaches -- a detail costs nothing there and is
/// exactly what is wanted on the day one fires.
///
/// A new one of these does not build until a row here says which caller converts it, which is the
/// same ceremony `BUILTIN_SURFACE_BACKLOG` imposes and for the same reason: an exemption nobody
/// has to justify is how a gate stops meaning anything.
pub(crate) const REFUSAL_INTERNAL_STATUS: &[InternalRefusal] = &[
    InternalRefusal {
        function: "kiln_c99_module_completion",
        sites: 1,
        converted_by: "the dynamic importer, which propagates its control status",
    },
    InternalRefusal {
        function: "kiln_c99_json_build",
        sites: 5,
        converted_by: "JSON.parse, which turns it into a SyntaxError the program can catch",
    },
    InternalRefusal {
        function: "kiln_c99_json_document",
        sites: 1,
        converted_by: "JSON.parse's SyntaxError, or a data module's materialization",
    },
];

const fn internal_refusal_sites(rows: &[InternalRefusal]) -> usize {
    let mut total = 0;
    let mut index = 0;
    while index < rows.len() {
        assert!(
            !rows[index].converted_by.is_empty() && !rows[index].function.is_empty(),
            "a REFUSAL_INTERNAL_STATUS row does not say which caller converts the status, which \
             cannot be told from a refusal somebody forgot to name"
        );
        total += rows[index].sites;
        index += 1;
    }
    total
}

const fn bytes_match_at(bytes: &[u8], at: usize, needle: &[u8]) -> bool {
    if at + needle.len() > bytes.len() {
        return false;
    }
    let mut index = 0;
    while index < needle.len() {
        if bytes[at + index] != needle[index] {
            return false;
        }
        index += 1;
    }
    true
}

/// How far the scan may jump when a nine-byte window is not `KILN_C99_`.
///
/// Boyer-Moore-Horspool, and not for speed's sake: this runs in `const` evaluation over most of a
/// megabyte of C, where a byte-at-a-time loop is not a scan but a compile-time budget. rustc's
/// `long_running_const_eval` refuses one, and is right to. The table is the distance from the last
/// window byte back to that byte's previous position in `KILN_C99_`, or the whole window when the
/// needle does not contain it at all -- which is most of C.
const fn refusal_scan_shift(window_end: u8) -> usize {
    match window_end {
        b'9' => 1,
        b'C' => 3,
        b'_' => 4,
        b'N' => 5,
        b'L' => 6,
        b'I' => 7,
        b'K' => 8,
        _ => 9,
    }
}

/// A refusal in the emitted prelude names what it lacked, or says which caller converts it.
///
/// THIS IS THE GATE, AND IT IS A COMPILE ERROR RATHER THAN A TEST FAILURE, for the reason ADR 0043
/// gives about the builtin surface: a rule somebody has to remember to check is a rule that holds
/// until the day it matters. A refusal that names nothing is the one thing a refusal must never
/// be, because naming what it lacked is the entire reason this backend refuses instead of
/// guessing -- and an unnamed one does worse than say nothing, because the enclosing instruction
/// then invents a reason. `Object.getOwnPropertyDescriptor(f, "prototype")` was reported as
/// `operation=Call detail=Call callee is unsupported by the native callable model`, a false
/// statement about a supported callee, produced by a helper three frames down that refused
/// silently.
///
/// So the status has exactly three producers, all of them in the prelude's diagnostic preamble,
/// and the prelude C may only ever *test* it. Anything else fails here.
///
/// A `const` panic carries a literal and nothing else, and a crate that does not compile cannot
/// run the test that would explain -- so the message carries the `grep` that names the line
/// instead. `refusal_sites_name_themselves` is the standing check, and it sees the half a `const`
/// cannot: that the internal-status sites are in the functions `REFUSAL_INTERNAL_STATUS` claims.
/// One prelude function's `KILN_C99_ABSENT` sites, and what proves the name really is absent.
///
/// The twin of [`InternalRefusal`], and it exists for the same reason: an answer a gate cannot
/// tell from an oversight is an answer nobody has to justify.
pub struct AbsentAnswer {
    /// The prelude function holding the sites.
    pub function: &'static str,
    /// How many `KILN_C99_ABSENT` sites it holds.
    pub sites: usize,
    /// What makes `undefined` the right answer there rather than a silent wrong one.
    pub proved_by: &'static str,
}

/// Every place the emitted prelude answers a read with `undefined`, by the function that holds it.
///
/// THE CLASS THIS EXISTS TO STOP. The native backend refuses what it has not built, which is the
/// right instinct -- but a read that answers `undefined` refuses nothing, and `undefined` is what
/// `typeof x.member === "function"` walks past. A bundled `postcss` died on `path.dirname` for
/// weeks that way, and a sweep on 2026-09-02 found thirty more: `Buffer.alloc` off
/// `require("node:buffer")`, `ArrayBuffer.prototype`, `Symbol.prototype`,
/// `String.prototype.values`, `Array.length`. Every one was a
/// place where the *membership predicate* behind an `undefined` answer was short, and no build and
/// no test could see it, because the wrong branch and the right branch look identical from inside.
///
/// So each row names the predicate that decides. Adding a site without a row fails the build; so
/// does deleting a predicate's last site and leaving the row. `absent_answers_are_where_they_say`
/// checks the half a `const` cannot -- that the sites are in the functions claimed here.
pub const ABSENT_ANSWER_SITES: &[AbsentAnswer] = &[
    AbsentAnswer {
        function: "kiln_c99_prototype_store_read_with_receiver",
        sites: 1,
        proved_by: "an accessor with no getter returns undefined by the language rule",
    },
    AbsentAnswer {
        function: "kiln_c99_function_species",
        sites: 2,
        proved_by: "the checked prototype chain ended at null, or an own accessor has no getter",
    },
    AbsentAnswer {
        function: "kiln_c99_callable_store_lookup",
        sites: 5,
        proved_by: "a callable with no property store has no own member at all, so there is no \
                    list to be short of",
    },
    AbsentAnswer {
        function: "kiln_c99_get_with_receiver",
        sites: 1,
        proved_by: "an accessor with no getter: the language defines that read as undefined",
    },
    AbsentAnswer {
        function: "kiln_c99_store_get_key",
        sites: 1,
        proved_by: "an accessor with no getter: the language defines that read as undefined. \
                    The site moved here with the headed store walk; kiln_c99_store_get in keys.c \
                    remains a Latin1 adapter for fixed C names and holds no site",
    },
    AbsentAnswer {
        function: "kiln_c99_shared_field_value",
        sites: 1,
        proved_by: "the sealed data field has the explicit undefined value tag",
    },
    AbsentAnswer {
        function: "kiln_c99_get_own_kind",
        sites: 35,
        proved_by: "one membership predicate per receiver kind -- kiln_c99_is_array_member, \
                    a shared facade's complete sealed field list and its reading realm's \
                    Object/Array prototype tables, \
                    kiln_c99_is_string_member, kiln_c99_is_date_member, \
                    kiln_c99_is_collection_member and the kiln_c99_collection_kind_member \
                    beneath it, which answers for a collection *prototype* the same way \
                    (a name belonging to a sibling kind -- `Map.prototype.add` -- is absent \
                    rather than unbuilt), kiln_c99_is_array_buffer_member, \
                    kiln_c99_is_typed_array_member, RegExp instance slots followed by the complete \
                    RegExp.prototype and Object.prototype stores/tables, and native callable own \
                    slots, the callable stores and prototype tables, Set constructor string statics (name, \
                    length and prototype), BigInt constructor string statics (asIntN and asUintN are \
                    handled above), then BUILTIN_SURFACE plus \
                    BUILTIN_SURFACE_BACKLOG for the table-driven built-ins. A bound function's \
                    a synchronous generator has no `@@asyncIterator` member (only async \
                    generators define it); and a bound function's `prototype` is decided by \
                    BoundFunctionCreate, which defines none, so it is \
                    absent when its [[Prototype]] is null or a `Function.prototype` whose store \
                    lacks it. These are the lists \
                    the two engines can disagree about, and \
                    scripts/native-surface-inventory.py --gate is what measures them",
    },
    AbsentAnswer {
        function: "kiln_c99_get_key",
        sites: 2,
        proved_by: "a hidden runtime slot no program can name, and a symbol the program made \
                    (`@@symbol:` and `@@symbol-for:` keys, kiln_c99_key_is_program_symbol) that \
                    reached a refusal: a built-in or exotic value carries such a symbol only \
                    through the property store every read consults first, so a refusal from the \
                    tables below it is absence. Well-known symbols are not in this row; a \
                    refusal on one stands",
    },
    AbsentAnswer {
        function: "kiln_c99_probe_symbol",
        sites: 1,
        proved_by: "the caller is the machine, not the program: `kiln_c99_to_primitive` asking \
                    for `@@toPrimitive`, iteration for `@@iterator`, `Object.prototype.toString` \
                    for `@@toStringTag`. A receiver that carries the symbol unwired refuses, and \
                    a probe the program did not write must not stop it on that. Until \
                    2026-09-19 this rule sat in kiln_c99_get_key and so covered every read, \
                    program reads included -- the inventory counted 35 silent `undefined`s \
                    behind it, `Map[Symbol.species]` and `RegExp.prototype[Symbol.replace]` \
                    among them. A program's read now refuses by name like any other member; \
                    the seven probe sites are the only callers of this function",
    },
    AbsentAnswer {
        function: "kiln_c99_get_computed",
        sites: 4,
        proved_by: "an index past the end and reserved slots nobody set: absence by position \
                    rather than by name, so no list decides it",
    },
    AbsentAnswer {
        function: "kiln_c99_iso_string",
        sites: 1,
        proved_by: "not a member read at all: an instant with no ISO spelling, whose caller \
                    decides between a throw and a null",
    },
];

pub const fn absent_answer_sites(rows: &[AbsentAnswer]) -> usize {
    let mut total = 0;
    let mut index = 0;
    while index < rows.len() {
        assert!(
            !rows[index].proved_by.is_empty() && !rows[index].function.is_empty(),
            "an ABSENT_ANSWER_SITES row does not say what proves the name absent, which cannot be \
             told from a silent wrong answer somebody did not notice"
        );
        total += rows[index].sites;
        index += 1;
    }
    total
}

/// An entry in the table of speculation invariants.
///
/// Invariant from `fastest-native-plan.md` and ADR 0056:
/// "A guard names its invariant. Every speculation is a row in a const table naming the
/// language rule it relies on, the way ABSENT_ANSWER_SITES works; an unlisted guard does not
/// compile."
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpeculationInvariant {
    pub name: &'static str,
    pub language_rule: &'static str,
}

pub const SPECULATION_INVARIANTS: &[SpeculationInvariant] = &[
    SpeculationInvariant {
        name: "parameter_number",
        language_rule: "ECMA-262 §7.1.3 ToNumber: If argument is a Number, no conversion is performed and operations can proceed unboxed",
    },
    SpeculationInvariant {
        name: "loop_speculation_exit",
        language_rule: "ECMA-262 §14.7.5: Loop header state deoptimizes cleanly to generic locals",
    },
    SpeculationInvariant {
        name: "upvalue_array",
        language_rule: "ECMA-262 §9.1.1.1 declarative environment records: a binding stored once holds that value at every later read, so an Array exotic object read from it at entry is the object read in the body",
    },
    SpeculationInvariant {
        name: "parameter_array",
        language_rule: "ECMA-262 §9.2.1 ordinary positional parameter binding: a simple, unmapped parameter retains its entry argument until an assignment; an Array-kind guard therefore authorizes the existing indexed-array fast tier",
    },
    SpeculationInvariant {
        name: "property_number",
        language_rule: "ECMA-262 §13.15.3 ApplyStringOrNumericBinaryOperator: an operand that is a Number takes the numeric branch; a property that answered anything else leaves for the generic clone at the read that answered it",
    },
    SpeculationInvariant {
        name: "spread_source_shaped",
        language_rule: "ECMA-262 §7.3.25 CopyDataProperties: a source whose own properties are all shaped data slots under string keys contributes exactly those slots' values; any other source leaves for the generic clone before the spread, with the literal rebuilt",
    },
    SpeculationInvariant {
        name: "spread_field_number",
        language_rule: "ECMA-262 §13.15.3 ApplyStringOrNumericBinaryOperator: a field of a sunk literal that holds a Number takes the numeric branch; one that does not leaves for the generic clone at the read, with the literal rebuilt",
    },
    SpeculationInvariant {
        name: "method_call_number",
        language_rule: "ECMA-262 §13.15.3 ApplyStringOrNumericBinaryOperator: an operand that is a Number takes the numeric branch; one that is not leaves for the generic clone after the call that answered it",
    },
];

pub const fn assert_speculation_invariants(rows: &[SpeculationInvariant]) -> usize {
    let mut index = 0;
    while index < rows.len() {
        assert!(
            !rows[index].name.is_empty() && !rows[index].language_rule.is_empty(),
            "a SPECULATION_INVARIANTS row does not say what language rule it relies on"
        );
        index += 1;
    }
    index
}

pub const fn lookup_speculation_invariant(name: &str) -> &'static SpeculationInvariant {
    let mut i = 0;
    while i < SPECULATION_INVARIANTS.len() {
        let a = SPECULATION_INVARIANTS[i].name.as_bytes();
        let b = name.as_bytes();
        if a.len() == b.len() {
            let mut j = 0;
            let mut eq = true;
            while j < a.len() {
                if a[j] != b[j] {
                    eq = false;
                    break;
                }
                j += 1;
            }
            if eq {
                return &SPECULATION_INVARIANTS[i];
            }
        }
        i += 1;
    }
    panic!("an unlisted guard does not compile: invariant name not in SPECULATION_INVARIANTS");
}

const _: usize = assert_speculation_invariants(SPECULATION_INVARIANTS);
const _: &SpeculationInvariant = lookup_speculation_invariant("parameter_number");
const _: &SpeculationInvariant = lookup_speculation_invariant("loop_speculation_exit");
const _: &SpeculationInvariant = lookup_speculation_invariant("upvalue_array");
const _: &SpeculationInvariant = lookup_speculation_invariant("parameter_array");
const _: &SpeculationInvariant = lookup_speculation_invariant("method_call_number");

/// A refusal in the emitted prelude names what it lacked, and an `undefined` answer names what
/// proves the name absent.
///
/// TWO HALVES OF ONE RULE, WHICH IS WHY THEY SHARE A SCAN. A read either says "the language puts
/// a member here and this backend has not built it", which stops the program at the cause, or "the
/// language puts nothing here", whose answer is `undefined` and stops nothing. Getting the second
/// one when the first was true is the expensive failure: `typeof x.member === "function"` takes
/// the other branch and the program dies three frames later naming nothing. Thirty of those were
/// measured on 2026-09-02, so `KILN_C99_ABSENT` and `ABSENT_ANSWER_SITES` now impose on the
/// `undefined` side the ceremony `KILN_C99_REFUSE` already imposed on the refusing side.
///
/// THIS IS THE GATE, AND IT IS A COMPILE ERROR RATHER THAN A TEST FAILURE, for the reason ADR 0043
/// gives about the builtin surface: a rule somebody has to remember to check is a rule that holds
/// until the day it matters. A refusal that names nothing is the one thing a refusal must never
/// be, because naming what it lacked is the entire reason this backend refuses instead of
/// guessing -- and an unnamed one does worse than say nothing, because the enclosing instruction
/// then invents a reason. `Object.getOwnPropertyDescriptor(f, "prototype")` was reported as
/// `operation=Call detail=Call callee is unsupported by the native callable model`, a false
/// statement about a supported callee, produced by a helper three frames down that refused
/// silently.
///
/// So the status has exactly three producers, all of them in the prelude's diagnostic preamble,
/// and the prelude C may only ever *test* it. Anything else fails here.
///
/// A `const` panic carries a literal and nothing else, and a crate that does not compile cannot
/// run the test that would explain -- so the message carries the `grep` that names the line
/// instead. `refusal_sites_name_themselves` is the standing check, and it sees the half a `const`
/// cannot: that the internal-status sites are in the functions `REFUSAL_INTERNAL_STATUS` claims.
const fn assert_refusals_name_themselves(source: &str, internal_sites: usize, absent_sites: usize) {
    let bytes = source.as_bytes();
    let mut at = 0;
    let mut internal = 0;
    let mut absent = 0;
    while at + 9 <= bytes.len() {
        if bytes[at] != b'K' || !bytes_match_at(bytes, at, b"KILN_C99_") {
            at += refusal_scan_shift(bytes[at + 8]);
        } else {
            if bytes_match_at(bytes, at, b"KILN_C99_CONTROL_ERROR") {
                assert!(
                    at >= 3
                        && (bytes_match_at(bytes, at - 3, b"== ")
                            || bytes_match_at(bytes, at - 3, b"!= ")),
                    "the prelude C produces KILN_C99_CONTROL_ERROR by hand instead of through \
                     KILN_C99_REFUSE, KILN_C99_REFUSE_NAMED or KILN_C99_REFUSE_INTERNAL, so a \
                     refusal there would name nothing. This crate no longer compiles, so the \
                     test that explains cannot run: `grep -n KILN_C99_CONTROL_ERROR \
                     crates/native/kiln-backend-c/src/runtime/*.c | grep -v '[=!]= '` names the line"
                );
            }
            if bytes_match_at(bytes, at, b"KILN_C99_REFUSE(") {
                assert!(
                    at + 17 < bytes.len() && bytes[at + 16] == b'"' && bytes[at + 17] != b'"',
                    "a KILN_C99_REFUSE site has no reason literal; say what the site lacked"
                );
            }
            if bytes_match_at(bytes, at, b"KILN_C99_REFUSE_NAMED(") {
                assert!(
                    at + 23 < bytes.len() && bytes[at + 22] == b'"' && bytes[at + 23] != b'"',
                    "a KILN_C99_REFUSE_NAMED site has no reason literal; say what the site lacked"
                );
            }
            /* An `undefined` answer must come through KILN_C99_ABSENT, which carries the reason
             * the name really is absent. Written by hand it carries nothing, and a short
             * membership list then reads as `undefined` where the interpreter has a value -- the
             * one wrong answer this backend is not allowed to give (ADR 0042). The check runs off
             * `return KILN_C99_OK;`, which the scan already stops at, and looks backwards. */
            if bytes_match_at(bytes, at, b"KILN_C99_OK;")
                && at >= 7
                && bytes_match_at(bytes, at - 7, b"return ")
            {
                let mut back = at - 7;
                while back > 0
                    && (bytes[back - 1] == b' '
                        || bytes[back - 1] == b'\n'
                        || bytes[back - 1] == b'\r'
                        || bytes[back - 1] == b'\t')
                {
                    back -= 1;
                }
                let answer = b"*out = kiln_c99_undefined();";
                assert!(
                    back < answer.len() || !bytes_match_at(bytes, back - answer.len(), answer),
                    "the prelude C answers a read with `undefined` by hand instead of through \
                     KILN_C99_ABSENT, so nothing there says why the name is absent rather than \
                     unimplemented -- and a short membership list would read as a silent wrong \
                     answer. This crate no longer compiles, so the test that explains cannot run: \
                     `grep -n -A1 '\\*out = kiln_c99_undefined();' \
                     crates/native/kiln-backend-c/src/runtime/*.c | grep -B1 'return KILN_C99_OK'` names \
                     the line"
                );
            }
            if bytes_match_at(bytes, at, b"KILN_C99_ABSENT(") {
                assert!(
                    at + 17 < bytes.len() && bytes[at + 16] == b'"' && bytes[at + 17] != b'"',
                    "a KILN_C99_ABSENT site has no reason literal; say what proves the name absent"
                );
                absent += 1;
            }
            if bytes_match_at(bytes, at, b"KILN_C99_REFUSE_INTERNAL(") {
                assert!(
                    at + 26 < bytes.len() && bytes[at + 25] == b'"' && bytes[at + 26] != b'"',
                    "a KILN_C99_REFUSE_INTERNAL site does not name the caller that converts it"
                );
                internal += 1;
            }
            at += 9;
        }
    }
    assert!(
        internal == internal_sites,
        "the prelude C holds a different number of KILN_C99_REFUSE_INTERNAL sites than \
         REFUSAL_INTERNAL_STATUS declares; add the row that says which caller converts the new \
         one, or drop the row that no longer describes anything. `grep -c \
         KILN_C99_REFUSE_INTERNAL crates/native/kiln-backend-c/src/runtime/*.c` is the count"
    );
    assert!(
        absent == absent_sites,
        "the prelude C holds a different number of KILN_C99_ABSENT sites than \
         ABSENT_ANSWER_SITES declares. A new one is a new place a read answers `undefined`: add \
         the row that says which membership predicate proves the name absent, or drop the row \
         that no longer describes anything. `grep -c KILN_C99_ABSENT \
         crates/native/kiln-backend-c/src/runtime/*.c` is the count"
    );
}

/// `prelude_runtime.c`, split into domain files under `runtime/` (see `runtime/README.md`); the
/// concatenation in this order is byte-identical to the original single file, and order is
/// load-bearing because C needs a declaration before its use.
macro_rules! runtime_part_manifest {
    ($callback:ident) => {
        $callback!(
            "runtime/foundation.c",
            "runtime/heap.c",
            "runtime/graph_capsule.c",
            "runtime/shapes.c",
            "runtime/platform.c",
            "runtime/node_support.c",
            "runtime/arrays.c",
            "runtime/properties.c",
            "runtime/generators.c",
            "runtime/methods.c",
            "runtime/web.c",
            "runtime/ryu_tables.c",
            "runtime/numbers.c",
            "runtime/builtins.c",
            "runtime/globals.c",
            "runtime/bigint.c",
            "runtime/temporal_values.c",
            "runtime/regexp.c",
            "runtime/symbols.c",
        )
    };
}

macro_rules! core_part_manifest {
    ($callback:ident) => {
        $callback!(
            "runtime/surface.c",
            "runtime/values.c",
            "runtime/text.c",
            "runtime/stores.c",
            "runtime/proxies.c",
            "runtime/keys.c",
            "runtime/read.c",
            "runtime/write.c",
            "runtime/inspect.c",
            "runtime/strings.c",
            "runtime/node.c",
            "runtime/temporal.c",
            "runtime/dispatch_part_a.c",
            "runtime/dispatch_part_b.c",
            "runtime/enumeration.c",
            "runtime/spread.c",
            "runtime/definitions.c",
            "runtime/json.c",
            "runtime/dates.c",
            "runtime/collections.c",
            "runtime/iterators.c",
            "runtime/errors.c",
            "runtime/promises.c",
            "runtime/constructors.c",
            "runtime/dynamic.c",
            "runtime/calls.c",
            "runtime/operators.c",
            "runtime/boxed.c",
        )
    };
}

macro_rules! concat_parts {
    ($($name:literal),* $(,)?) => { concat!($(include_str!($name)),*) };
}
#[cfg(test)]
macro_rules! array_parts {
    ($($name:literal),* $(,)?) => { &[$(($name, include_str!($name))),*] };
}
macro_rules! runtime_parts {
    () => {
        runtime_part_manifest!(concat_parts)
    };
}
macro_rules! core_parts {
    () => {
        core_part_manifest!(concat_parts)
    };
}

macro_rules! method_metadata_part {
    () => {
        concat!(
            r#"
/* One registry-generated lookup answers dispatch routing and argument coercion together.  The
 * zero default is intentional: unknown and reserved codes remain refused by the normal dispatch
 * fallthrough, rather than acquiring a handler merely because they are in a numeric interval. */
static kiln_c99_method_metadata kiln_c99_method_metadata_for(uint64_t code) {
    /* Temporal families intentionally use interior codes that are not individual registry rows;
     * the method value carries the kind/operation offset. Keep these range rules in the same
     * descriptor authority so dispatch does not lose every member except each endpoint. */
    if (code >= KILN_C99_METHOD_TEMPORAL_FROM && code <= KILN_C99_METHOD_TEMPORAL_FROM_LAST)
        return (kiln_c99_method_metadata){KILN_C99_GROUP_TEMPORAL | KILN_C99_GROUP_TEMPORAL_FROM,
                                         UINT32_C(0), UINT32_C(0)};
    if (code >= KILN_C99_METHOD_TEMPORAL_COMPARE && code <= KILN_C99_METHOD_TEMPORAL_COMPARE_LAST)
        return (kiln_c99_method_metadata){KILN_C99_GROUP_TEMPORAL | KILN_C99_GROUP_TEMPORAL_COMPARE,
                                         UINT32_C(0), UINT32_C(0)};
    if (code >= KILN_C99_METHOD_TEMPORAL_NOW_READ &&
        code <= KILN_C99_METHOD_TEMPORAL_NOW_READ_LAST)
        return (kiln_c99_method_metadata){KILN_C99_GROUP_TEMPORAL | KILN_C99_GROUP_TEMPORAL_NOW_READ,
                                         UINT32_C(0), UINT32_C(0)};
    if (code >= KILN_C99_METHOD_TEMPORAL_OP && code <= KILN_C99_METHOD_TEMPORAL_OP_LAST)
        return (kiln_c99_method_metadata){KILN_C99_GROUP_TEMPORAL | KILN_C99_GROUP_TEMPORAL_OP,
                                         UINT32_C(0), UINT32_C(0)};
    switch (code) {
"#,
            emit_method_metadata_cases!(),
            r#"    default:
        return (kiln_c99_method_metadata){UINT32_C(0), UINT32_C(0), UINT32_C(0)};
    }
}
static uint32_t kiln_c99_method_declared_length(uint64_t code) {
    switch (code) {
/* KILN_NATIVE_METHOD_LENGTH_ROWS */
    default: return UINT32_MAX;
    }
}
"#
        )
    };
}

macro_rules! method_metadata_decl {
    () => {
        r#"typedef struct kiln_c99_method_metadata {
    uint32_t groups;
    uint32_t numeric_arguments;
    uint32_t textual_arguments;
} kiln_c99_method_metadata;
static kiln_c99_method_metadata kiln_c99_method_metadata_for(uint64_t code);
static uint32_t kiln_c99_method_declared_length(uint64_t code);
"#
    };
}

#[cfg(test)]
pub(crate) fn prelude_source_parts() -> Vec<(&'static str, &'static str)> {
    let mut parts = Vec::new();
    parts.extend_from_slice(core_part_manifest!(array_parts));
    parts.extend_from_slice(runtime_part_manifest!(array_parts));
    parts
}

#[allow(long_running_const_eval)]
const _: () = assert_refusals_name_themselves(
    concat!(core_parts!(), runtime_parts!()),
    internal_refusal_sites(REFUSAL_INTERNAL_STATUS),
    absent_answer_sites(ABSENT_ANSWER_SITES),
);

/// Every `strlen(` in the prelude C measures a genuine C string -- a builtin's name out of a
/// table, an error message, an environment value, a header name, a Temporal field -- and never
/// a property key. A key is interned behind a `kiln_string` header and may contain U+0000
/// (`JSON.parse('{"a\\u0000b":1}')`, a registry symbol's binary id), so its length is
/// `kiln_c99_key_len(ikey)`, one load; `strlen` on a key stops at the NUL and the property
/// answers under the wrong name with nothing failing (`.scratch/json-tape/issues/01`). The count
/// is the guard, the way `ABSENT_ANSWER_SITES` is: a new `strlen(` does not compile until its
/// argument has been shown to be a C string and this number raised, or it is rewritten as
/// `kiln_c99_key_len`. The pass that set it classified all ninety sites on 2026-09-07: fourteen
/// took a shape or property key and became `kiln_c99_key_len`, one became the property's stored
/// `key_len`, and these are the rest. Site 79 is `kiln_c99_set_function_name`'s
/// `prefix` (`"get"`/`"set"`/NULL at every caller in values.c): a genuine NUL-terminated
/// C string literal, previously measured inside `kiln_c99_text_append_c`, now spelled
/// out at the `kiln_c99_static_text(prefix, strlen(prefix))` call in definitions.c.
/// Seven left on 2026-09-26 with the `node:http` lazy-header probes (six header-buffer scans,
/// now `memchr`) and the request-method table (lengths now stored beside the names).
/// One added the same day by `kiln_c99_register_builtin_keys` (surface.c), which measures a
/// surface row's `name`: a C string literal out of the generated table, as its other readers do.
/// One added 2026-09-27 by `kiln_c99_realm_tag_method_pristine` (stores.c, parallel issue 30),
/// which measures `kiln_c99_method_name`'s answer: a C string literal out of the method table.
/// One added for the fixed Atomics name table's load-time registration (surface.c, realms 08).
/// Native callable descriptors measure names from the literal method-name registry.
/// Stack snapshots measure the emitted source name, a NUL-terminated C literal.
pub const C_STRING_LENGTH_SITES: usize = 77;

/// How far the `strlen(` scan may skip after reading the last byte of its seven-byte window:
/// the Horspool shift, as `refusal_scan_shift` is for the refusal scan, so the const evaluator
/// touches a fraction of the prelude's bytes rather than every one.
const fn strlen_scan_shift(last: u8) -> usize {
    match last {
        b's' => 6,
        b't' => 5,
        b'r' => 4,
        b'l' => 3,
        b'e' => 2,
        b'n' => 1,
        _ => 7,
    }
}

/// The number of `strlen(` spellings in the prelude C, for the assertion below.
pub const fn c_string_length_sites(prelude: &str) -> usize {
    let bytes = prelude.as_bytes();
    let mut at = 0;
    let mut count = 0;
    while at + 7 <= bytes.len() {
        if bytes[at] == b's' && bytes_match_at(bytes, at, b"strlen(") {
            count += 1;
            at += 7;
        } else {
            at += strlen_scan_shift(bytes[at + 6]);
        }
    }
    count
}

#[allow(long_running_const_eval)]
const _: () = assert!(
    c_string_length_sites(concat!(core_parts!(), runtime_parts!())) == C_STRING_LENGTH_SITES,
    "the prelude C holds a different number of `strlen(` sites than C_STRING_LENGTH_SITES \
     declares. If the new one measures a property or shape key, it truncates at an embedded NUL: \
     use `kiln_c99_key_len(ikey)` (or the property's stored `key_len`) instead. If it measures a \
     genuine C string, raise the count. `grep -n 'strlen(' crates/native/kiln-backend-c/src/runtime/*.c` \
     lists the sites"
);

/// The globals [`BUILTIN_SURFACE`] speaks for, in declaration order and without repeats.
pub fn builtin_surface_owners() -> Vec<&'static str> {
    let mut owners: Vec<&'static str> = Vec::new();
    for member in BUILTIN_SURFACE {
        if !owners.contains(&member.owner) {
            owners.push(member.owner);
        }
    }
    owners
}

/// Every member name `global` answers to natively, in declaration order.
pub fn builtin_surface_members(global: &str) -> Vec<&'static str> {
    BUILTIN_SURFACE
        .iter()
        .filter(|member| member.owner == global)
        .map(|member| member.name)
        .collect()
}

macro_rules! header_prefix {
    () => {
        concat!(
            r#"/* Kiln C99 backend: deterministic scalar unit. */
/* `sigaction`, `SA_RESTART` and `write` are POSIX, and `-std=c99` puts glibc in strict-ANSI mode
 * where none of them is declared -- which shows up as `storage size of 'action' isn't known`, not
 * as a missing header. A feature-test macro has to be asked for before the first include, so this
 * is the first line of the unit. Darwin declares them without it and dislikes being narrowed. */
#if !defined(_WIN32) && !defined(__APPLE__) && !defined(_POSIX_C_SOURCE)
#define _POSIX_C_SOURCE 200809L
#endif
#include <ctype.h>
#include <errno.h>
#include <math.h>
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
/* The SIGUSR1 introspection dump. `signal.h` is C89 everywhere, but `sigaction`, `SIGUSR1` and
 * `write` are POSIX, so a non-POSIX target compiles the same prelude with the dump inert. */
#if defined(__unix__) || defined(__APPLE__)
#include <fcntl.h>
#include <signal.h>
#include <unistd.h>
#endif

/* C ABI version 1; HostOps ABI version 1. */
#define KILN_NUMBER_FORMAT_VERSION 1
#define KILN_NUMBER_PARSE_VERSION 1
#define KILN_C99_OK 0
#define KILN_C99_INVALID_ARGUMENT (-1)
#define KILN_C99_CONTROL_ERROR (-2)
#define KILN_C99_HOST_ERROR (-3)
#define KILN_C99_OUT_OF_MEMORY (-4)
#define KILN_C99_CALL_DEPTH_ERROR (-5)
#define KILN_C99_UNINITIALIZED_ERROR (-6)
/* A value the program threw, travelling outward until a handler takes it.  Distinct from every
 * other status because those mean "this cannot be done" and this one means "the program said
 * so": a `catch` takes this and must not take the others. */
#define KILN_C99_THROWN (-7)
/* Internal host dispatch used by the graph launcher for importer-bound CommonJS requires.
 * This is an operation number, not a field in kiln_host, so the two-word host ABI remains
 * stable across the standalone and generated runtimes. */
#define KILN_C99_HOST_MODULE_CREATE_REQUIRE UINT16_C(0x7FF0)

#if defined(__GNUC__) || defined(__clang__)
#define KILN_LIKELY(x) __builtin_expect(!!(x), 1)
#define KILN_UNLIKELY(x) __builtin_expect(!!(x), 0)
#define KILN_COLD __attribute__((cold))
#define KILN_NOINLINE __attribute__((noinline))
#else
#define KILN_LIKELY(x) (x)
#define KILN_UNLIKELY(x) (x)
#define KILN_COLD
#define KILN_NOINLINE
#endif

/* Demand-driven prelude: one flag per prunable method dispatch group. A
 * native build passes `-DKILN_RETAIN_GROUP_<name>=0` for groups its program
 * cannot reach (see `retained_method_groups`). Every other compilation --
 * including test translation units that embed the runtime parts without the
 * declarations header -- keeps the default of 1 and links exactly what it
 * always has. Flags are read only by `#if` guards on dispatch blocks,
 * constructor fast paths, header materialization, and global installs, never
 * by hot-path expressions, so a fully retained build compiles to identical
 * code. This block lives in the prefix (not the declarations header) so that
 * every translation unit assembling the runtime parts sees the defaults. */
#ifndef KILN_RETAIN_GROUP_NODE_EVENT
#define KILN_RETAIN_GROUP_NODE_EVENT 1
#endif
#ifndef KILN_RETAIN_GROUP_NODE_NET
#define KILN_RETAIN_GROUP_NODE_NET 1
#endif
#ifndef KILN_RETAIN_GROUP_NODE_SERVER
#define KILN_RETAIN_GROUP_NODE_SERVER 1
#endif
#ifndef KILN_RETAIN_GROUP_NODE_HTTP
#define KILN_RETAIN_GROUP_NODE_HTTP 1
#endif
#ifndef KILN_RETAIN_GROUP_NODE_RESPONSE
#define KILN_RETAIN_GROUP_NODE_RESPONSE 1
#endif
#ifndef KILN_RETAIN_GROUP_BUFFER
#define KILN_RETAIN_GROUP_BUFFER 1
#endif
#ifndef KILN_RETAIN_GROUP_NODE_UTIL
#define KILN_RETAIN_GROUP_NODE_UTIL 1
#endif
#ifndef KILN_RETAIN_GROUP_NODE_DIAGNOSTICS
#define KILN_RETAIN_GROUP_NODE_DIAGNOSTICS 1
#endif
#ifndef KILN_RETAIN_GROUP_NODE_STREAM
#define KILN_RETAIN_GROUP_NODE_STREAM 1
#endif
#ifndef KILN_RETAIN_GROUP_NODE_PATH
#define KILN_RETAIN_GROUP_NODE_PATH 1
#endif
#ifndef KILN_RETAIN_GROUP_URL
#define KILN_RETAIN_GROUP_URL 1
#endif
#ifndef KILN_RETAIN_GROUP_TEMPORAL
#define KILN_RETAIN_GROUP_TEMPORAL 1
#endif
#ifndef KILN_RETAIN_GROUP_TEMPORAL_FROM
#define KILN_RETAIN_GROUP_TEMPORAL_FROM 1
#endif
#ifndef KILN_RETAIN_GROUP_TEMPORAL_COMPARE
#define KILN_RETAIN_GROUP_TEMPORAL_COMPARE 1
#endif
#ifndef KILN_RETAIN_GROUP_TEMPORAL_NOW_READ
#define KILN_RETAIN_GROUP_TEMPORAL_NOW_READ 1
#endif
#ifndef KILN_RETAIN_GROUP_TEMPORAL_OP
#define KILN_RETAIN_GROUP_TEMPORAL_OP 1
#endif
#ifndef KILN_RETAIN_GROUP_DATE_SETTER
#define KILN_RETAIN_GROUP_DATE_SETTER 1
#endif
#ifndef KILN_RETAIN_GROUP_MATH_NUMBER
#define KILN_RETAIN_GROUP_MATH_NUMBER 1
#endif

/* The 16-byte string header is load-bearing ABI shared by constants, GC storage and
 * boundary values, so it is declared in the prefix: the static diagnostics below already
 * hold a `const kiln_string *`, and both the single-TU header and the separate-prelude
 * header start here. It is declared exactly once; the method-metadata section below
 * only adds `kiln_value` and `prelude_types.h`. */
"#,
            kiln_native_runtime::kiln_string_abi_declaration!(),
        )
    };
}

/// The refusal diagnostic's fields and their names. One per thread (ADR 0070, realms issue 04 step
/// 5): the diagnostic describes the failure the running call is unwinding, which the same thread's
/// launcher reads the moment its realm hands the failure back -- after the realm has exited, so it
/// cannot be the realm's -- and a realm publishes what it keeps to its own record
/// (`kiln_realm_diagnostic_note`). Each name below is a field of `kiln_c99_diagnostic`, so the code
/// that reads and writes a diagnostic did not change.
macro_rules! diagnostic_fields {
    () => {
        r#"typedef struct kiln_refusal_diagnostic {
    const char *site;
    const char *operation;
    size_t instruction;
    size_t depth;
    uint64_t method;
    uint64_t receiver_tag;
    int has_method;
    const char *detail;
    const char *function;
    size_t function_len;
    /* Generated and inferred function names remain canonical units until the diagnostic reaches
     * its UTF-8 host boundary. Fixed runtime names may still use the legacy ASCII slot above. */
    const kiln_string *function_key;
    /* Composed text for a diagnostic that wants more than one name. `GetComputed` is the only
     * operation whose key is a value rather than a constant, so its refusal could not say which
     * member was read -- `operation=GetComputed` and nothing else. Written at the moment of
     * failure, when nothing else is using it. */
    char compose[128];
} kiln_refusal_diagnostic;
#define kiln_c99_diagnostic_site (kiln_c99_diagnostic.site)
#define kiln_c99_diagnostic_operation (kiln_c99_diagnostic.operation)
#define kiln_c99_diagnostic_instruction (kiln_c99_diagnostic.instruction)
#define kiln_c99_diagnostic_depth (kiln_c99_diagnostic.depth)
#define kiln_c99_diagnostic_method (kiln_c99_diagnostic.method)
#define kiln_c99_diagnostic_receiver_tag (kiln_c99_diagnostic.receiver_tag)
#define kiln_c99_diagnostic_has_method (kiln_c99_diagnostic.has_method)
#define kiln_c99_diagnostic_detail (kiln_c99_diagnostic.detail)
#define kiln_c99_diagnostic_function (kiln_c99_diagnostic.function)
#define kiln_c99_diagnostic_function_len (kiln_c99_diagnostic.function_len)
#define kiln_c99_diagnostic_function_key (kiln_c99_diagnostic.function_key)
#define kiln_c99_diagnostic_compose (kiln_c99_diagnostic.compose)
"#
    };
}

macro_rules! header_static_diagnostics {
    () => {
        concat!(
        r#"/* Refusals are runtime/compiler diagnostics, not JavaScript exceptions.  Keep the most specific
 * detail at the failing helper and attach the generated module + bytecode site as control
 * unwinds.  The entry boundary renders this into its ordinary scalar result, so no private
 * runtime pointer or new public ABI type escapes. */
"#,
        diagnostic_fields!(),
        r#"static _Thread_local kiln_refusal_diagnostic kiln_c99_diagnostic;
/* The depth of the running call, saved and restored around every closure invocation: a frame
 * register (see `builtins.c`), so per thread. */
static _Thread_local int kiln_c99_diagnostic_active_depth = 0;

/* THE FOUR WAYS TO PRODUCE `KILN_C99_CONTROL_ERROR`, AND THERE ARE NO OTHERS.
 *
 * A refusal that names nothing sends the reader somewhere else. Four investigations in one day
 * went to the wrong place because the site that knew what was missing said nothing and let an
 * enclosing instruction invent a reason -- `Object.getOwnPropertyDescriptor(f, "prototype")` was
 * reported as an unsupported *callee*, which is a false statement about a supported one.
 *
 * So the status is never written by hand in the prelude C. It comes from one of these, each of
 * which carries the reason at the place that knows it, and `assert_refusals_name_themselves`
 * fails the build if the token appears anywhere else. That is the ADR 0043 move applied one layer
 * down: a gate in `const` evaluation rather than a test somebody has to remember.
 *
 * The first note wins, here as everywhere else in the diagnostic: the innermost helper runs first
 * and is the most specific, so an outer one adds provenance and leaves the detail alone. */
static KILN_COLD int kiln_c99_refuse(const char *lacked) {
    if (kiln_c99_diagnostic_detail == NULL) {
        kiln_c99_diagnostic_detail = lacked;
    }
    return KILN_C99_CONTROL_ERROR;
}

/* A refusal whose subject is a name the site is holding and no instruction carries -- the member
 * behind a `GetProperty` is named by the instruction, but the member behind a backlog entry read
 * through a built-in's store is not.
 *
 * `kiln_c99_diagnostic_detail` is a `const char *` and cannot be formatted into, so the composed
 * text goes in `kiln_c99_diagnostic_compose`, which `GetComputed` also writes -- for the
 * *function* field, and only while no detail exists. One buffer, one writer: `GetComputed` checks
 * whether the detail already points into it, and an inner site that got there first keeps it. */
static KILN_COLD int kiln_c99_refuse_named(const char *lacked, const char *subject, size_t subject_len) {
    if (kiln_c99_diagnostic_detail == NULL) {
        /* Copied rather than `snprintf`ed. Nothing formatted here is a number, so the locale would
         * not change the answer -- but "this unit never calls the C library's printers" is a rule
         * a test can state in one line, and one with an exception for the diagnostic that only
         * joins two strings is not. */
        size_t room = sizeof kiln_c99_diagnostic_compose - 1;
        size_t at = strlen(lacked);
        if (at > room) {
            at = room;
        }
        memcpy(kiln_c99_diagnostic_compose, lacked, at);
        if (at + 2 < room) {
            kiln_c99_diagnostic_compose[at++] = ':';
            kiln_c99_diagnostic_compose[at++] = ' ';
            if (subject_len > room - at) {
                subject_len = room - at;
            }
            memcpy(kiln_c99_diagnostic_compose + at, subject, subject_len);
            at += subject_len;
        }
        kiln_c99_diagnostic_compose[at] = '\0';
        kiln_c99_diagnostic_detail = kiln_c99_diagnostic_compose;
    }
    return KILN_C99_CONTROL_ERROR;
}

static KILN_COLD void kiln_c99_note_refusal(const char *site, size_t instruction,
                                  const char *operation, size_t depth);

/* Generated stack-tail sites already have the exact source coordinate and operation. Their
 * outlined helpers use this form so the diagnostic remains owned by that instruction while the
 * control status is still impossible to manufacture unnamed in runtime C. */
static KILN_COLD int kiln_c99_refuse_at(const char *site, size_t instruction,
                              const char *operation, size_t depth) {
    kiln_c99_note_refusal(site, instruction, operation, depth);
    return KILN_C99_CONTROL_ERROR;
}

/* `KILN_C99_CONTROL_ERROR` is also an *internal* status: a reader hands it to a caller that
 * converts it into something the program sees, and no refusal is ever reported. `JSON.parse` on
 * `{bad` throws a `SyntaxError` in both engines, so the reader's twenty-two exits are not gaps and
 * a detail on one would be noise -- and noise is how a gate earns an exemption and stops meaning
 * anything.
 *
 * The argument names the caller that does the converting. It is compiled away; it is there so the
 * claim can be checked by reading, and `REFUSAL_INTERNAL_STATUS` in the emitter carries the same
 * claim as a row that has to be added before a new one of these will build. */
static KILN_COLD int kiln_c99_internal_status(const char *converted_by) {
    (void)converted_by;
    return KILN_C99_CONTROL_ERROR;
}

#define KILN_C99_REFUSE(lacked) return kiln_c99_refuse(lacked)
#define KILN_C99_REFUSE_NAMED(lacked, subject, subject_len) \
    return kiln_c99_refuse_named((lacked), (subject), (subject_len))
#define KILN_C99_REFUSE_AT(site, instruction, operation, depth) \
    return kiln_c99_refuse_at((site), (instruction), (operation), (depth))
#define KILN_C99_REFUSE_INTERNAL(converted_by) return kiln_c99_internal_status(converted_by)

"#
        )
    };
}

macro_rules! header_extern_diagnostics {
    () => {
        concat!(
        r#"/* Refusals are runtime/compiler diagnostics, not JavaScript exceptions. */
#define KILN_SEPARATE_PRELUDE 1
"#,
        diagnostic_fields!(),
        r#"extern _Thread_local kiln_refusal_diagnostic kiln_c99_diagnostic;
extern _Thread_local int kiln_c99_diagnostic_active_depth;

int kiln_c99_refuse(const char *lacked);
int kiln_c99_refuse_named(const char *lacked, const char *subject, size_t subject_len);
int kiln_c99_refuse_at(const char *site, size_t instruction, const char *operation, size_t depth);
int kiln_c99_internal_status(const char *converted_by);

#define KILN_C99_REFUSE(lacked) return kiln_c99_refuse(lacked)
#define KILN_C99_REFUSE_NAMED(lacked, subject, subject_len) \
    return kiln_c99_refuse_named((lacked), (subject), (subject_len))
#define KILN_C99_REFUSE_AT(site, instruction, operation, depth) \
    return kiln_c99_refuse_at((site), (instruction), (operation), (depth))
#define KILN_C99_REFUSE_INTERNAL(converted_by) return kiln_c99_internal_status(converted_by)

"#
        )
    };
}

macro_rules! header_rest {
    () => {
        concat!(
            r#"static inline void kiln_c99_reset_diagnostics(void) {
    kiln_refusal_diagnostic *diagnostics = &kiln_c99_diagnostic;
    diagnostics->site = NULL;
    diagnostics->operation = NULL;
    diagnostics->instruction = 0;
    diagnostics->depth = 0;
    diagnostics->method = 0;
    diagnostics->receiver_tag = 0;
    diagnostics->has_method = 0;
    diagnostics->detail = NULL;
    diagnostics->function = NULL;
    diagnostics->function_len = 0;
    diagnostics->function_key = NULL;
}

/* THE OTHER HALF OF A REFUSAL, AND THE ONE THAT HAD NO CEREMONY.
 *
 * A refusal says "the language puts a member here and this backend has not built it" and stops
 * the program at the cause. This says the opposite: "the language puts nothing here", whose
 * answer is `undefined` and whose read must not stop anything. `Object.nope` is not a gap.
 *
 * The two are told apart by a membership predicate -- `kiln_c99_is_array_member`,
 * `kiln_c99_is_string_member`, `BUILTIN_SURFACE` plus `BUILTIN_SURFACE_BACKLOG`, and the rest --
 * and when one of those lists is short the wrong branch is taken *silently*. That is the
 * expensive failure this backend has: a refusal stops a program at the cause, an `undefined`
 * walks past `typeof x.member === "function"` and dies three frames later. Thirty of them were
 * measured on one afternoon, and not one was visible in a build or a test.
 *
 * So an `undefined` answer is now spelled here, with a reason that names the predicate proving
 * absence, exactly as a refusal names what it lacked. `assert_absent_answers_are_declared` fails
 * the build if the prelude C hands one back any other way, or if the number of them changes
 * without a row in `ABSENT_ANSWER_SITES` saying which function grew one and why. The reason is
 * compiled away; it is there so the claim can be read, and so that adding a silent `undefined`
 * costs somebody a sentence about why it is not a wrong answer.
 *
 * WHAT THIS CANNOT DO. It cannot prove a predicate is *complete* -- that is the two engines'
 * question, and `scripts/native-surface-inventory.py --gate` is what asks it. What it can do is
 * make every place that could be wrong a place somebody wrote down. */
#define KILN_C99_ABSENT(why, out)      \
    do {                               \
        (void)(why);                   \
        *(out) = kiln_c99_undefined(); \
        return KILN_C99_OK;            \
    } while (0)

/* Recursion runs on the C stack, where exhausting it aborts the process instead of reporting
 * anything. The limit is the interpreter's own, 10,000 calls (parallel issue 30: 2,048 threw at
 * a recursion node and the interpreter finish), which every thread a realm runs on has the stack
 * for: 64 MiB, the launcher's main thread (`kiln_cli_grow_main_stack`, `main_stack_flags`), a
 * worker's and a pinned realm's. A call that goes through the runtime also asks how much of the
 * stack is left (`kiln_c99_stack_exhausted`), since its frames are as big as the C it passes
 * through, so a thread with less stack than that answers the same `RangeError` rather than a
 * segfault. */
#define KILN_C99_MAX_CALL_DEPTH 10000


enum kiln_value_tag {
    KILN_VALUE_UNDEFINED = 0,
    KILN_VALUE_NULL = 1,
    KILN_VALUE_BOOLEAN = 2,
    KILN_VALUE_NUMBER = 3,
    KILN_VALUE_STRING = 4,
    /* The two tags below are internal to a generated unit and never cross the embedding
     * boundary: a function is converted to text before any host call and before the module
     * result, and a cell is never a program-visible value at all.  That is what keeps
     * KILN_C_ABI_VERSION at 1 -- the boundary value is still exactly tags 0 through 4. */

    /* bits holds a kiln_closure*. */
    KILN_VALUE_FUNCTION = 5,
    /* A shared box; bits holds a kiln_value*.  It appears only in a local slot the compiler
     * marked captured, and in a closure's upvalue array. */
    KILN_VALUE_CELL = 6,
    /* A `let` or `const` slot before its declaration runs.  Reading one is the temporal dead
     * zone, and it is a distinct state from undefined: `let x; x` is undefined, `x; let x` is
     * an error.  Only slots the program actually marks are ever tested for it. */
    KILN_VALUE_UNINITIALIZED = 7,
    /* A callable the runtime supplies rather than the program.  bits holds the builtin's code
     * below.  It is a separate tag from a closure because it has no body to point at and no
     * captures to carry. */
    KILN_VALUE_BUILTIN = 8,
    /* bits holds a kiln_object*.  Objects and arrays are separate tags because `typeof` and the
     * two index paths differ; they share nothing but the tag space. */
    KILN_VALUE_OBJECT = 9,
    KILN_VALUE_ARRAY = 10,
    /* A method the runtime supplies.  bits holds its code, and *not* its receiver: `a.map` and
     * `b.map` are the same function object in the language, which is what makes
     * `a.map === b.map` true, and `CallMethod` keeps the receiver on the stack anyway. */
    KILN_VALUE_METHOD = 11,
    /* bits holds a kiln_generator*: a body that has been started and not finished. */
    KILN_VALUE_GENERATOR = 12,
    /* bits holds a double* -- an instant in milliseconds since the epoch. A pointer rather than
     * the value itself because a Date is mutable: `setFullYear` writes through it. */
    KILN_VALUE_DATE = 13,
    /* An array slot that was never written, or was deleted.
     *
     * It *reads* as undefined, which is why the marker must never escape into an expression --
     * but it is not one: `1 in [1, , 3]` is false where `1 in [1, undefined, 3]` is true, a
     * console shows `<1 empty item>`, and every element-walking method skips it. */
    KILN_VALUE_HOLE = 14,
    /* bits holds a kiln_collection*. A Map and a Set are one tag because they are one structure:
     * a Set is a Map whose values are never read. */
    KILN_VALUE_COLLECTION = 15,
    /* bits holds a kiln_promise*: a value that arrives once, later. */
    KILN_VALUE_PROMISE = 16,
    /* The `resolve` and `reject` an executor is handed. bits holds a kiln_settler*. */
    KILN_VALUE_SETTLER = 17,
    /* A callable with its receiver fixed. bits holds a kiln_bound*.
     *
     * Needed because a promise reaction is a handler *and* the thing it runs against, and the
     * queue can only hold one value per slot -- so the pair has to be one value. */
    KILN_VALUE_BOUND = 18,
    /* A property key with an identity of its own. bits holds a kiln_symbol*. */
    KILN_VALUE_SYMBOL = 19,
    /* A compiled pattern and its cursor. bits holds a kiln_regexp*. */
    KILN_VALUE_REGEXP = 20,
    /* An arbitrary-precision integer. bits holds the provider's opaque handle. */
    KILN_VALUE_BIGINT = 21,
    /* Typed views are private heap values.  Their pointers never cross the scalar embedding ABI. */
    KILN_VALUE_ARRAY_BUFFER = 22,
    KILN_VALUE_TYPED_ARRAY = 23,
    /* A target with a handler standing in front of it. bits holds a kiln_proxy*.
     *
     * A tag of its own rather than a flag on an object, because a proxy is not an object with
     * extra behaviour: it can stand in front of an array or a function just as well, `typeof`
     * asks the thing behind it, and every property operation on it is a *call*. Nothing that
     * tests for one of the tags above may accidentally match it. */
    KILN_VALUE_PROXY = 24,
    /* One of the eight `Temporal` types. bits holds the provider's opaque handle.
     *
     * A tag rather than an object because every one of them is immutable and every operation on
     * one has to reach the engine anyway (ADR 0025): an object would be a second place for the
     * state to live and a second thing to keep in step. */
    KILN_VALUE_TEMPORAL = 25,
    /* A graph-scoped CommonJS require function. Private to native graph objects: bits holds a
     * kiln_commonjs_require*, whose callback and opaque context are supplied by the launcher. */
    KILN_VALUE_COMMONJS_REQUIRE = 26,
    /* A `DataView`: a window onto a buffer whose element type and byte order are chosen at each
     * access. bits holds a kiln_data_view*.
     *
     * WHY A TAG AND NOT A TYPED VIEW WITH A FLAG. A DataView is not a typed array wearing a
     * different hat -- it has no element kind, no length in elements and no indexed properties at
     * all -- so sharing KILN_VALUE_TYPED_ARRAY would mean every `tag == KILN_VALUE_TYPED_ARRAY`
     * test in this file had to learn to say no, and each one that was missed would answer
     * confidently and wrongly about `view.length`, `view[0]` or `Array.from(view)`. A silent
     * wrong answer is the one thing this backend is not allowed to give (ADR 0042).
     *
     * WHY NOT A NEW `kiln_typed_kind` EITHER. That one is a real C enum, switched over without a
     * default in `kiln_c99_typed_name`, `_constructor`, `_prototype`, `_width`, `_get` and `_set`
     * -- and the emitted C is compiled with `-Wall -Wextra -Werror`. A new kind would therefore
     * *require* six answers to questions a DataView has no answer to.
     *
     * The tag is cheap for the reason the ones above it are: the embedding boundary carries
     * exactly tags 0 through 4, so KILN_C_ABI_VERSION does not move, and every switch over
     * `value.tag` here has a `default`. */
    KILN_VALUE_DATA_VIEW = 27,
    KILN_VALUE_RETURN_SIGNAL = 28,
    /* A realm-local facade over an ADR 0073 sealed graph; bits holds a GC payload. */
    KILN_VALUE_SHARED = 29
};

#define KILN_C99_BUILTIN_PROMISE 15
#define KILN_C99_BUILTIN_BOOLEAN 16
#define KILN_C99_BUILTIN_REFLECT 17
/* `Object.prototype`, which a program reaches for its methods rather than as an object:
 * `Object.prototype.hasOwnProperty.call(x, k)` is how every transpiled module asks whether a key
 * is own. It answers the three names it carries and nothing else -- it is not yet the object the
 * chain actually ends at, which is what `Object.getPrototypeOf` would need. */
#define KILN_C99_BUILTIN_OBJECT_PROTOTYPE 18
/* ...and the same for the two whose methods a program borrows most:
 * `Array.prototype.slice.call(arguments)` is how a transpiled module makes an array. */
#define KILN_C99_BUILTIN_ARRAY_PROTOTYPE 19
#define KILN_C99_BUILTIN_STRING_PROTOTYPE 20
#define KILN_C99_BUILTIN_SYMBOL 21
#define KILN_C99_BUILTIN_REGEXP 22
/* The rest of the prototype objects a program borrows a method from.
 * `RegExp.prototype.test.call(re, s)` is how a library calls one without trusting the receiver. */
#define KILN_C99_BUILTIN_REGEXP_PROTOTYPE 23
#define KILN_C99_BUILTIN_DATE_PROTOTYPE 24
#define KILN_C99_BUILTIN_NUMBER_PROTOTYPE 25
#define KILN_C99_BUILTIN_FUNCTION_PROTOTYPE 26
/* `Function`, reached for its `prototype` rather than to construct. */
#define KILN_C99_BUILTIN_FUNCTION 27
/* `BigInt`, which converts and never constructs: `new BigInt(1)` is a TypeError in the
 * specification, and the reason is that a BigInt is a primitive. */
#define KILN_C99_BUILTIN_BIGINT 28
#define KILN_C99_BUILTIN_ARRAY_BUFFER 29
#define KILN_C99_BUILTIN_INT8_ARRAY 30
#define KILN_C99_BUILTIN_UINT8_ARRAY 31
#define KILN_C99_BUILTIN_UINT8_CLAMPED_ARRAY 32
#define KILN_C99_BUILTIN_INT16_ARRAY 33
#define KILN_C99_BUILTIN_UINT16_ARRAY 34
#define KILN_C99_BUILTIN_INT32_ARRAY 35
#define KILN_C99_BUILTIN_UINT32_ARRAY 36
#define KILN_C99_BUILTIN_FLOAT32_ARRAY 37
#define KILN_C99_BUILTIN_FLOAT64_ARRAY 38
/* Private prototype markers for the numeric view constructors. Concrete prototypes own their
 * constructor metadata and all inherit one common typed-array method table. */
#define KILN_C99_BUILTIN_TYPED_ARRAY_PROTOTYPE 39
#define KILN_C99_BUILTIN_INT8_ARRAY_PROTOTYPE 40
#define KILN_C99_BUILTIN_UINT8_ARRAY_PROTOTYPE 41
#define KILN_C99_BUILTIN_UINT8_CLAMPED_ARRAY_PROTOTYPE 42
#define KILN_C99_BUILTIN_INT16_ARRAY_PROTOTYPE 43
#define KILN_C99_BUILTIN_UINT16_ARRAY_PROTOTYPE 44
#define KILN_C99_BUILTIN_INT32_ARRAY_PROTOTYPE 45
#define KILN_C99_BUILTIN_UINT32_ARRAY_PROTOTYPE 46
#define KILN_C99_BUILTIN_FLOAT32_ARRAY_PROTOTYPE 47
#define KILN_C99_BUILTIN_FLOAT64_ARRAY_PROTOTYPE 48

/* The two collection prototype tables. They exist for the same reason the others do: a class
 * that extends `Map` needs `Map.prototype` to link its own prototype to, and without one the
 * whole of `class Registry extends Map {}` is unreachable. */
#define KILN_C99_BUILTIN_MAP_PROTOTYPE 49
#define KILN_C99_BUILTIN_SET_PROTOTYPE 50

/* One table for every error kind rather than one each. The chain is not what answers a question
 * about an error here -- `instanceof` reads the kind the instance records, and `name`, `message`
 * and `stack` are own properties of the instance -- so the table exists to be a *link*, which is
 * what `class HttpError extends Error {}` needs and all it needs. */
#define KILN_C99_BUILTIN_ERROR_PROTOTYPE 51

/* `Proxy` is a constructor and not a namespace: `typeof Proxy` is "function", and the only way
 * to reach one is `new`. */
#define KILN_C99_BUILTIN_PROXY 52

/* `Temporal`: the namespace, its eight types, and `Temporal.Now`.
 *
 * The eight are contiguous and in the provider's own kind order, so the code the provider takes is
 * `builtin - KILN_C99_BUILTIN_TEMPORAL_PLAIN_DATE` rather than a translation table that could
 * drift away from it. */
#define KILN_C99_BUILTIN_TEMPORAL 53
#define KILN_C99_BUILTIN_TEMPORAL_PLAIN_DATE 54
#define KILN_C99_BUILTIN_TEMPORAL_PLAIN_TIME 55
#define KILN_C99_BUILTIN_TEMPORAL_PLAIN_DATE_TIME 56
#define KILN_C99_BUILTIN_TEMPORAL_PLAIN_YEAR_MONTH 57
#define KILN_C99_BUILTIN_TEMPORAL_PLAIN_MONTH_DAY 58
#define KILN_C99_BUILTIN_TEMPORAL_DURATION 59
#define KILN_C99_BUILTIN_TEMPORAL_INSTANT 60
#define KILN_C99_BUILTIN_TEMPORAL_ZONED_DATE_TIME 61
#define KILN_C99_BUILTIN_TEMPORAL_NOW 62
/* WHATWG URL. The object it constructs is ordinary; only construction and query lookup need
 * runtime code, so no URL-specific value tag or ABI representation crosses the boundary. */
#define KILN_C99_BUILTIN_URL 63
#define KILN_C99_BUILTIN_BUFFER 64
#define KILN_C99_BUILTIN_SYMBOL_PROTOTYPE 96
#define KILN_C99_BUILTIN_ARRAY_BUFFER_PROTOTYPE 97
#define KILN_C99_BUILTIN_BOOLEAN_PROTOTYPE 98
#define KILN_C99_BUILTIN_BIGINT_PROTOTYPE 99

/* The global `crypto`, a namespace with no rows of its own: every member refuses by name until
 * `node:crypto` is linked, at which point that module replaces the global binding with its own
 * object. It was an empty ordinary object built in `globals.c`, so `crypto.randomUUID` read
 * `undefined` where the interpreter has a function -- eleven silent wrong answers in the
 * 2026-09-19 inventory, one of them the member `uuid` feature-detects on. */
#define KILN_C99_BUILTIN_CRYPTO 100
#define KILN_C99_BUILTIN_BIG_INT64_ARRAY_PROTOTYPE 101
#define KILN_C99_BUILTIN_BIG_UINT64_ARRAY_PROTOTYPE 102
#define KILN_C99_BUILTIN_SHARED_ARRAY_BUFFER_PROTOTYPE 103

/* Array methods, part two.
 *
 * Numbered here rather than beside the first eleven because the block that answers a *namespace*
 * method uses the generated dispatch category, and everything reached through an array receiver
 * has to stay outside it -- as is the block that answers a *string* method. The gap is deliberate:
 * none of these codes reaches a cache key, but renumbering the block would still be a diff nobody
 * could review. */
"#,
    emit_method_define!(KILN_C99_METHOD_ARRAY_AT),
    emit_method_define!(KILN_C99_METHOD_ARRAY_FIND),
    emit_method_define!(KILN_C99_METHOD_ARRAY_FIND_INDEX),
    emit_method_define!(KILN_C99_METHOD_ARRAY_SOME),
    emit_method_define!(KILN_C99_METHOD_ARRAY_EVERY),
    emit_method_define!(KILN_C99_METHOD_ARRAY_SORT),
    emit_method_define!(KILN_C99_METHOD_ARRAY_FLAT),
    emit_method_define!(KILN_C99_METHOD_ARRAY_ENTRIES),
    emit_method_define!(KILN_C99_METHOD_ARRAY_KEYS),
    emit_method_define!(KILN_C99_METHOD_ARRAY_REVERSE),
    emit_method_define!(KILN_C99_METHOD_ARRAY_SHIFT),
    emit_method_define!(KILN_C99_METHOD_ARRAY_UNSHIFT),
    emit_method_define!(KILN_C99_METHOD_ARRAY_FILL),
    emit_method_define!(KILN_C99_METHOD_ARRAY_LAST_INDEX_OF),
    emit_method_define!(KILN_C99_METHOD_ARRAY_SPLICE),
    emit_method_define!(KILN_C99_METHOD_ARRAY_COPY_WITHIN),
    emit_method_define!(KILN_C99_METHOD_ARRAY_TO_STRING),
    emit_method_define!(KILN_C99_METHOD_ARRAY_FLAT_MAP),
    emit_method_define!(KILN_C99_METHOD_ARRAY_FIND_LAST),
    emit_method_define!(KILN_C99_METHOD_ARRAY_FIND_LAST_INDEX),
    emit_method_define!(KILN_C99_METHOD_ARRAY_REDUCE_RIGHT),
    emit_method_define!(KILN_C99_METHOD_ARRAY_TO_SORTED),
    emit_method_define!(KILN_C99_METHOD_ARRAY_TO_REVERSED),
    emit_method_define!(KILN_C99_METHOD_ARRAY_TO_SPLICED),
    emit_method_define!(KILN_C99_METHOD_ARRAY_WITH),
    r#"
/* String methods, part two, numbered above the array block for the same reason the array block is
 * numbered above the namespace one: the guard that routes a code to a receiver kind is a range,
 * and a new range is cheaper than renumbering an old one. */
"#,
    emit_method_define!(KILN_C99_METHOD_STRING_CODE_POINT_AT),
    emit_method_define!(KILN_C99_METHOD_STRING_SUBSTR),
    emit_method_define!(KILN_C99_METHOD_STRING_LOCALE_COMPARE),
    emit_method_define!(KILN_C99_METHOD_STRING_NORMALIZE),
    emit_method_define!(KILN_C99_METHOD_STRING_TO_STRING),
    emit_method_define!(KILN_C99_METHOD_STRING_VALUE_OF),
    r#"/* These three only ever take a pattern, so they are intercepted before the string block: they
 * need to call back into the program, and the string block has no interpreter to call with. */
"#,
    emit_method_define!(KILN_C99_METHOD_STRING_MATCH),
    emit_method_define!(KILN_C99_METHOD_STRING_MATCH_ALL),
    emit_method_define!(KILN_C99_METHOD_STRING_SEARCH),
    r#"
/* `Error.prototype.toString`, which every error inherits and no other object does. Deliberately
 * past the end of every range `kiln_c99_lookup_method` searches: it is not reached by name from a
 * table but handed out by the object read that recognises an error, because an ordinary object
 * with a `toString` of its own must not find this one. */
"#,
    emit_method_define!(KILN_C99_METHOD_ERROR_TO_STRING),
    r#"
/* `DataView.prototype`, whose twenty accessors are one contiguous block so that a receiver check
 * and a name lookup are both a range test. The ten `get`s come first and the ten `set`s second,
 * with the BigInt pair at the end of each half, so that "is this a write" is one comparison
 * against the first `set` rather than a second table. */
"#,
    emit_method_define!(KILN_C99_METHOD_DATA_VIEW_GET_INT8),
    emit_method_define!(KILN_C99_METHOD_DATA_VIEW_GET_UINT8),
    emit_method_define!(KILN_C99_METHOD_DATA_VIEW_GET_INT16),
    emit_method_define!(KILN_C99_METHOD_DATA_VIEW_GET_UINT16),
    emit_method_define!(KILN_C99_METHOD_DATA_VIEW_GET_INT32),
    emit_method_define!(KILN_C99_METHOD_DATA_VIEW_GET_UINT32),
    emit_method_define!(KILN_C99_METHOD_DATA_VIEW_GET_FLOAT32),
    emit_method_define!(KILN_C99_METHOD_DATA_VIEW_GET_FLOAT64),
    emit_method_define!(KILN_C99_METHOD_DATA_VIEW_GET_BIG_INT64),
    emit_method_define!(KILN_C99_METHOD_DATA_VIEW_GET_BIG_UINT64),
    emit_method_define!(KILN_C99_METHOD_DATA_VIEW_SET_INT8),
    emit_method_define!(KILN_C99_METHOD_DATA_VIEW_SET_UINT8),
    emit_method_define!(KILN_C99_METHOD_DATA_VIEW_SET_INT16),
    emit_method_define!(KILN_C99_METHOD_DATA_VIEW_SET_UINT16),
    emit_method_define!(KILN_C99_METHOD_DATA_VIEW_SET_INT32),
    emit_method_define!(KILN_C99_METHOD_DATA_VIEW_SET_UINT32),
    emit_method_define!(KILN_C99_METHOD_DATA_VIEW_SET_FLOAT32),
    emit_method_define!(KILN_C99_METHOD_DATA_VIEW_SET_FLOAT64),
    emit_method_define!(KILN_C99_METHOD_DATA_VIEW_SET_BIG_INT64),
    emit_method_define!(KILN_C99_METHOD_DATA_VIEW_SET_BIG_UINT64),
    r#"
/* The block's endpoints are `..._GET_INT8` and `..._SET_BIG_UINT64`, spelled out at each of the
 * five range tests below rather than aliased to a shorter pair of names.
 *
 * An alias would read better and cannot be had. `parse_numeric_id_defines` in
 * `tests/emitted_c.rs` treats *every* `#define KILN_C99_METHOD_...` line as a numeric ID
 * declaration and fails the run when the value does not parse as a number, and giving the alias a
 * numeric value instead makes it a duplicate of the member it names -- which that file allows for
 * exactly one pair, `TEMPORAL_LAST`. Spelling the endpoints out is also what lets
 * `every_method_a_name_lookup_walks_over_has_a_name` resolve this block's lookup range at all: it
 * reads the two bounds of each `kiln_c99_lookup_method` call out of the `#define` table, so a
 * bound it cannot resolve is a range it silently stops checking. */

#define KILN_C99_BUILTIN_MAP 8
#define KILN_C99_BUILTIN_SET 9
/* The error constructors, contiguous so the kind is the offset: `new TypeError(m)` and
 * `new Error(m)` differ only in the name they write. */
#define KILN_C99_BUILTIN_ERROR 10
#define KILN_C99_BUILTIN_TYPE_ERROR 11
#define KILN_C99_BUILTIN_RANGE_ERROR 12
#define KILN_C99_BUILTIN_REFERENCE_ERROR 13
#define KILN_C99_BUILTIN_SYNTAX_ERROR 14
#define KILN_C99_BUILTIN_URI_ERROR 65
#define KILN_C99_BUILTIN_WEAK_MAP 66
#define KILN_C99_BUILTIN_WEAK_SET 67
#define KILN_C99_BUILTIN_WEAK_MAP_PROTOTYPE 68
#define KILN_C99_BUILTIN_WEAK_SET_PROTOTYPE 69
#define KILN_C99_BUILTIN_PROMISE_PROTOTYPE 70
/* The error kind that reports several failures as one. It has its own value (rather than sharing
 * the promise prototype's marker) because the kind is stored in every error instance. */
#define KILN_C99_BUILTIN_AGGREGATE_ERROR 74
#define KILN_C99_BUILTIN_ABORT_CONTROLLER 71
#define KILN_C99_BUILTIN_ABORT_SIGNAL 72
#define KILN_C99_BUILTIN_TEXT_ENCODER 73
#define KILN_C99_BUILTIN_AGGREGATE_ERROR_PROTOTYPE 75

/* The globals this backend does not implement yet, published anyway so that `typeof` tells the
 * truth about them.
 *
 * WHY A VALUE AT ALL, RATHER THAN LEAVING THE NAME ABSENT. `typeof X === "undefined"` is how real
 * code feature-detects, so a name the interpreter answers `"function"` for and this backend
 * answers `"undefined"` for does not make a compiled program fail -- it makes it silently take
 * the *other* branch. That is a wrong answer, and a wrong answer is the one thing this backend is
 * not allowed to give. A refusal is honest: the program stops and the message names the operation
 * nobody implemented. So each of these carries the right `typeof` and the right `name`, and every
 * other use of one reaches the refusal at the end of the builtin read, or the refusal at the end
 * of the construct dispatch, and says so.
 *
 * Contiguous, and bounded by the two markers, because the only thing the rest of the file needs
 * to ask about them is "is this one of them" -- `kiln_c99_is_unavailable_global`. */
#define KILN_C99_BUILTIN_UNAVAILABLE_FIRST 76
#define KILN_C99_BUILTIN_INTL 76
/* 77 was `DataView` until it became a real constructor. Retired rather than reused: every code
 * between the two markers means "this backend does not implement this global", so handing 77 to
 * the next global to be built would put an implemented one inside the unimplemented range, and
 * closing the hole by shifting the fifteen below it would renumber a range to save one number
 * nobody counts. `RETIRED_BUILTINS` in `tests/emitted_c.rs` is where the hole is recorded, since
 * builtin codes are otherwise required to be dense. */
#define KILN_C99_BUILTIN_SHARED_ARRAY_BUFFER 78
#define KILN_C99_BUILTIN_WEAK_REF 79
#define KILN_C99_BUILTIN_FINALIZATION_REGISTRY 80
#define KILN_C99_BUILTIN_SHADOW_REALM 81
/* `Iterator` is the one member of this block that is no longer one of them: `Iterator.from` and
 * most of `Iterator.prototype` are answered now. It keeps 82 rather than moving out of the range
 * because the builtin IDs are asserted gapless from 1 (`assert_numeric_id_invariants`), so a
 * vacated number is a build failure -- and the two markers around this block are read by nothing
 * today, so the exception costs a comment rather than a dispatch. */
#define KILN_C99_BUILTIN_ITERATOR 82
#define KILN_C99_BUILTIN_FLOAT16_ARRAY 83
#define KILN_C99_BUILTIN_BIG_INT64_ARRAY 84
#define KILN_C99_BUILTIN_BIG_UINT64_ARRAY 85
#define KILN_C99_BUILTIN_DISPOSABLE_STACK 86
#define KILN_C99_BUILTIN_ASYNC_DISPOSABLE_STACK 87
#define KILN_C99_BUILTIN_SUPPRESSED_ERROR 88
#define KILN_C99_BUILTIN_EVAL_ERROR 89
#define KILN_C99_BUILTIN_TEXT_DECODER 90
#define KILN_C99_BUILTIN_EVAL 91
#define KILN_C99_BUILTIN_UNAVAILABLE_LAST 91

/* `performance`, which is answered rather than refused: `performance.now()` reads the realm's
 * virtual clock, which is the only clock a reproducible build is allowed to have (ADR 0015). It
 * keeps the number it was given while it was unavailable and sits one past the end of the range
 * above, because the number is what an emitted unit stores and renumbering it would invalidate
 * every cached artifact to say nothing new. */
#define KILN_C99_BUILTIN_PERFORMANCE 92

/* `DataView`, which used to be one of the globals above and is now a constructor like any other.
 * Numbered past the unavailable range rather than inside the hole it left, because a code in that
 * range is what the comment there promises is unimplemented. Its prototype table is next to it
 * for the reason every other pair here is: the two are read as one thing. */
#define KILN_C99_BUILTIN_DATA_VIEW 93
#define KILN_C99_BUILTIN_DATA_VIEW_PROTOTYPE 94
/* `%IteratorPrototype%`: the table the iterator helpers live on, and the object every helper this
 * backend builds links to. A method table rather than a constructor, so it reports `typeof
 * "object"` through `kiln_c99_is_namespace` exactly as `Array.prototype` does. */
#define KILN_C99_BUILTIN_ITERATOR_PROTOTYPE 95
/* The abstract `%TypedArray%` constructor. Kept distinct from the shared
 * `%TypedArray%.prototype` method table above: concrete typed-array constructors inherit from
 * this function, while concrete typed-array prototype objects inherit from that table. */
#define KILN_C99_BUILTIN_TYPED_ARRAY 104

#define KILN_C99_BUILTIN_DATE 7

/* A body that yielded rather than returned.  Not an error: the caller is the generator's `next`,
 * which turns it into `{ value, done: false }`. */
#define KILN_C99_YIELDED 1

/* `String(x)`.  A template's `${x}` lowers to a call to it rather than to `+`, because the two
 * use different hints: `${x}` asks `toString` first and `+` asks `valueOf` first. */
#define KILN_C99_BUILTIN_STRING 1
#define KILN_C99_BUILTIN_MATH 2
#define KILN_C99_BUILTIN_NUMBER 3
#define KILN_C99_BUILTIN_OBJECT 4
#define KILN_C99_BUILTIN_ARRAY 5
#define KILN_C99_BUILTIN_JSON 6

/* Array methods.  Stable within a generated unit only -- unlike the instruction and constant
 * tags, none of these reaches a cache key. */
"#,
    emit_method_define!(KILN_C99_METHOD_PUSH),
    emit_method_define!(KILN_C99_METHOD_POP),
    emit_method_define!(KILN_C99_METHOD_JOIN),
    emit_method_define!(KILN_C99_METHOD_INDEX_OF),
    emit_method_define!(KILN_C99_METHOD_INCLUDES),
    emit_method_define!(KILN_C99_METHOD_SLICE),
    emit_method_define!(KILN_C99_METHOD_CONCAT),
    emit_method_define!(KILN_C99_METHOD_MAP),
    emit_method_define!(KILN_C99_METHOD_FILTER),
    emit_method_define!(KILN_C99_METHOD_FOR_EACH),
    emit_method_define!(KILN_C99_METHOD_REDUCE),
    r#"
/* String methods.  `slice` and `indexOf` are shared with the array table by name and told apart
 * by the receiver, exactly as the language does. */
"#,
    emit_method_define!(KILN_C99_METHOD_STRING_SLICE),
    emit_method_define!(KILN_C99_METHOD_SUBSTRING),
    emit_method_define!(KILN_C99_METHOD_STRING_INDEX_OF),
    emit_method_define!(KILN_C99_METHOD_LAST_INDEX_OF),
    emit_method_define!(KILN_C99_METHOD_STRING_INCLUDES),
    emit_method_define!(KILN_C99_METHOD_STARTS_WITH),
    emit_method_define!(KILN_C99_METHOD_ENDS_WITH),
    emit_method_define!(KILN_C99_METHOD_CHAR_AT),
    emit_method_define!(KILN_C99_METHOD_CHAR_CODE_AT),
    emit_method_define!(KILN_C99_METHOD_STRING_AT),
    emit_method_define!(KILN_C99_METHOD_TRIM),
    emit_method_define!(KILN_C99_METHOD_TRIM_START),
    emit_method_define!(KILN_C99_METHOD_TRIM_END),
    emit_method_define!(KILN_C99_METHOD_SPLIT),
    emit_method_define!(KILN_C99_METHOD_REPEAT),
    emit_method_define!(KILN_C99_METHOD_PAD_START),
    emit_method_define!(KILN_C99_METHOD_PAD_END),
    emit_method_define!(KILN_C99_METHOD_TO_UPPER_CASE),
    emit_method_define!(KILN_C99_METHOD_TO_LOWER_CASE),
    emit_method_define!(KILN_C99_METHOD_REPLACE),
    emit_method_define!(KILN_C99_METHOD_REPLACE_ALL),
    emit_method_define!(KILN_C99_METHOD_STRING_CONCAT),
    r#"
/* Namespace methods.  Unlike the array and string tables these take no receiver: `Math.floor` is
 * reached through `Math`, but `Math` contributes nothing to the answer. */
"#,
    emit_method_define!(KILN_C99_METHOD_MATH_FLOOR),
    emit_method_define!(KILN_C99_METHOD_MATH_CEIL),
    emit_method_define!(KILN_C99_METHOD_MATH_ROUND),
    emit_method_define!(KILN_C99_METHOD_MATH_TRUNC),
    emit_method_define!(KILN_C99_METHOD_MATH_ABS),
    emit_method_define!(KILN_C99_METHOD_MATH_SIGN),
    emit_method_define!(KILN_C99_METHOD_MATH_SQRT),
    emit_method_define!(KILN_C99_METHOD_MATH_CBRT),
    emit_method_define!(KILN_C99_METHOD_MATH_POW),
    emit_method_define!(KILN_C99_METHOD_MATH_MIN),
    emit_method_define!(KILN_C99_METHOD_MATH_MAX),
    emit_method_define!(KILN_C99_METHOD_MATH_LOG),
    emit_method_define!(KILN_C99_METHOD_MATH_LOG2),
    emit_method_define!(KILN_C99_METHOD_MATH_LOG10),
    emit_method_define!(KILN_C99_METHOD_MATH_EXP),
    emit_method_define!(KILN_C99_METHOD_MATH_HYPOT),
    emit_method_define!(KILN_C99_METHOD_MATH_ATAN2),
    emit_method_define!(KILN_C99_METHOD_MATH_SIN),
    emit_method_define!(KILN_C99_METHOD_MATH_COS),
    emit_method_define!(KILN_C99_METHOD_MATH_TAN),
    emit_method_define!(KILN_C99_METHOD_MATH_ASIN),
    emit_method_define!(KILN_C99_METHOD_MATH_ACOS),
    emit_method_define!(KILN_C99_METHOD_MATH_ATAN),
    emit_method_define!(KILN_C99_METHOD_IS_INTEGER),
    emit_method_define!(KILN_C99_METHOD_IS_SAFE_INTEGER),
    emit_method_define!(KILN_C99_METHOD_IS_NAN),
    emit_method_define!(KILN_C99_METHOD_IS_FINITE),
    emit_method_define!(KILN_C99_METHOD_OBJECT_KEYS),
    emit_method_define!(KILN_C99_METHOD_OBJECT_VALUES),
    emit_method_define!(KILN_C99_METHOD_OBJECT_ENTRIES),
    emit_method_define!(KILN_C99_METHOD_OBJECT_ASSIGN),
    emit_method_define!(KILN_C99_METHOD_OBJECT_PREVENT_EXTENSIONS),
    emit_method_define!(KILN_C99_METHOD_OBJECT_IS_EXTENSIBLE),
    emit_method_define!(KILN_C99_METHOD_HAS_OWN),
    emit_method_define!(KILN_C99_METHOD_IS_ARRAY),
    emit_method_define!(KILN_C99_METHOD_ARRAY_OF),
    emit_method_define!(KILN_C99_METHOD_OBJECT_CREATE),
    emit_method_define!(KILN_C99_METHOD_JSON_STRINGIFY),
    emit_method_define!(KILN_C99_METHOD_JSON_PARSE),
    emit_method_define!(KILN_C99_METHOD_GENERATOR_NEXT),
    emit_method_define!(KILN_C99_METHOD_GENERATOR_RETURN),
    emit_method_define!(KILN_C99_METHOD_GENERATOR_SELF),
    r#"
/* Date readers. Every one of these is UTC, because the realm has one zone and it is UTC
 * (ADR 0015) -- so the local reader and the UTC reader are the same function rather than two
 * that happen to agree. */
"#,
    emit_method_define!(KILN_C99_METHOD_GET_TIME),
    emit_method_define!(KILN_C99_METHOD_DATE_NOW),
    emit_method_define!(KILN_C99_METHOD_DATE_UTC),
    emit_method_define!(KILN_C99_METHOD_DATE_PARSE),
    r#"
/* The end of every chain is `Object.prototype`, and these are what it carries. */
"#,
    emit_method_define!(KILN_C99_METHOD_HAS_OWN_PROPERTY),
    emit_method_define!(KILN_C99_METHOD_OBJECT_TO_STRING),
    emit_method_define!(KILN_C99_METHOD_OBJECT_TO_LOCALE_STRING),
    emit_method_define!(KILN_C99_METHOD_OBJECT_VALUE_OF),
    r#"
/* Map and Set. `size` is a property rather than a method, so it is answered by the read. */
"#,
    emit_method_define!(KILN_C99_METHOD_COLLECTION_GET),
    emit_method_define!(KILN_C99_METHOD_COLLECTION_SET),
    emit_method_define!(KILN_C99_METHOD_COLLECTION_ADD),
    emit_method_define!(KILN_C99_METHOD_COLLECTION_HAS),
    emit_method_define!(KILN_C99_METHOD_COLLECTION_DELETE),
    emit_method_define!(KILN_C99_METHOD_COLLECTION_CLEAR),
    emit_method_define!(KILN_C99_METHOD_COLLECTION_FOR_EACH),
    emit_method_define!(KILN_C99_METHOD_COLLECTION_KEYS),
    emit_method_define!(KILN_C99_METHOD_COLLECTION_VALUES),
    emit_method_define!(KILN_C99_METHOD_COLLECTION_ENTRIES),
    emit_method_define!(KILN_C99_METHOD_COLLECTION_SELF),
    emit_method_define!(KILN_C99_METHOD_ITERATOR_NEXT),
    emit_method_define!(KILN_C99_METHOD_ITERATOR_SELF),
    emit_method_define!(KILN_C99_METHOD_ARRAY_VALUES),
    emit_method_define!(KILN_C99_METHOD_THROW_TYPE_ERROR),
    r#"
/* Promise, and the two halves of driving an async function. */
"#,
    emit_method_define!(KILN_C99_METHOD_THEN),
    emit_method_define!(KILN_C99_METHOD_CATCH),
    emit_method_define!(KILN_C99_METHOD_FINALLY),
    emit_method_define!(KILN_C99_METHOD_PROMISE_RESOLVE),
    emit_method_define!(KILN_C99_METHOD_PROMISE_REJECT),
    emit_method_define!(KILN_C99_METHOD_PROMISE_ALL),
    emit_method_define!(KILN_C99_METHOD_PROMISE_RACE),
    emit_method_define!(KILN_C99_METHOD_ASYNC_RESUME),
    emit_method_define!(KILN_C99_METHOD_ASYNC_THROW),
    emit_method_define!(KILN_C99_METHOD_ALL_STEP),
    r#"
/* The global functions. They are namespace methods with no namespace: nothing is reached through
 * before them, so `LoadBuiltin` pushes the method value directly. Two of them share a name with a
 * `Number` method and not its behaviour -- `isNaN('x')` coerces where `Number.isNaN('x')` does
 * not -- so they are separate codes rather than the same one reached two ways. */
"#,
    emit_method_define!(KILN_C99_METHOD_PARSE_INT),
    emit_method_define!(KILN_C99_METHOD_PARSE_FLOAT),
    emit_method_define!(KILN_C99_METHOD_GLOBAL_IS_NAN),
    emit_method_define!(KILN_C99_METHOD_GLOBAL_IS_FINITE),
    r#"
/* Number methods. Read off the value rather than through a `Number.prototype` object, which is
 * what the interpreter does and for the same reason: there is no such object yet. */
"#,
    emit_method_define!(KILN_C99_METHOD_NUMBER_TO_STRING),
    emit_method_define!(KILN_C99_METHOD_NUMBER_TO_FIXED),
    emit_method_define!(KILN_C99_METHOD_NUMBER_TO_EXPONENTIAL),
    emit_method_define!(KILN_C99_METHOD_NUMBER_TO_PRECISION),
    emit_method_define!(KILN_C99_METHOD_NUMBER_VALUE_OF),
    r#"
/* The text-escaping globals. `escape`/`unescape` are Annex B and work in code units; the four URI
 * functions work in UTF-8 bytes and differ only in which characters they leave alone. */
"#,
    emit_method_define!(KILN_C99_METHOD_ESCAPE),
    emit_method_define!(KILN_C99_METHOD_UNESCAPE),
    emit_method_define!(KILN_C99_METHOD_ENCODE_URI),
    emit_method_define!(KILN_C99_METHOD_ENCODE_URI_COMPONENT),
    emit_method_define!(KILN_C99_METHOD_DECODE_URI),
    emit_method_define!(KILN_C99_METHOD_DECODE_URI_COMPONENT),
    r#"
/* `queueMicrotask(fn)`: the queue promises already drain, reached directly. */
"#,
    emit_method_define!(KILN_C99_METHOD_QUEUE_MICROTASK),
    r#"
/* The timer functions. `clearTimeout` and `clearInterval` do the same thing and are two codes
 * because they are two names: a program can print either one. */
"#,
    emit_method_define!(KILN_C99_METHOD_SET_TIMEOUT),
    emit_method_define!(KILN_C99_METHOD_SET_INTERVAL),
    emit_method_define!(KILN_C99_METHOD_CLEAR_TIMEOUT),
    emit_method_define!(KILN_C99_METHOD_CLEAR_INTERVAL),
    r#"
/* The two combinators that do not fail on the first rejection. Named outside the contiguous
 * `Promise` range because that range is how a namespace's methods are found, and it was already
 * closed. `@@rejectStep` is the rejection-side twin of `@@allStep`: `all` and `race` settle their
 * promise on any rejection and need no handler of their own, and these two do. */
"#,
    emit_method_define!(KILN_C99_METHOD_PROMISE_ALL_SETTLED),
    emit_method_define!(KILN_C99_METHOD_PROMISE_ANY),
    emit_method_define!(KILN_C99_METHOD_PROMISE_WITH_RESOLVERS),
    emit_method_define!(KILN_C99_METHOD_NODE_TTY_ISATTY),
    emit_method_define!(KILN_C99_METHOD_TYPED_TO_STRING_TAG),
    emit_method_define!(KILN_C99_METHOD_REJECT_STEP),
    r#"
/* The two halves of `finally`, bound to the handler. They run it and then restore the outcome
 * they were passing through, which is the whole difference from `then(f, f)`. */
"#,
    emit_method_define!(KILN_C99_METHOD_FINALLY_PASS),
    emit_method_define!(KILN_C99_METHOD_FINALLY_RAISE),
    r#"/* ...and the two that put the outcome back once the handler's own result has settled. Bound to
 * the outcome, so the pair needs no state object either. */
"#,
    emit_method_define!(KILN_C99_METHOD_FINALLY_RESTORE),
    emit_method_define!(KILN_C99_METHOD_FINALLY_RERAISE),
    r#"
/* Resolving a promise *with* a promise. The attach is a job of its own, which is a turn a program
 * can see -- so it is a step rather than something `settle` does on the spot. */
"#,
    emit_method_define!(KILN_C99_METHOD_ADOPT_STEP),
    r#"
/* `Object.freeze` and `Object.isFrozen`, outside the contiguous `Object` range because it was
 * already closed. */
"#,
    emit_method_define!(KILN_C99_METHOD_OBJECT_FREEZE),
    emit_method_define!(KILN_C99_METHOD_OBJECT_IS_FROZEN),
    r#"
/* What every callable answers to, whatever kind of callable it is:
 * `Object.prototype.hasOwnProperty.call(x, k)` is how a transpiled module asks whether a key is
 * own, and `Array.prototype.slice.call(arguments)` is how it makes an array. */
"#,
    emit_method_define!(KILN_C99_METHOD_FUNCTION_CALL),
    emit_method_define!(KILN_C99_METHOD_FUNCTION_APPLY),
    emit_method_define!(KILN_C99_METHOD_FUNCTION_BIND),
    emit_method_define!(KILN_C99_METHOD_FUNCTION_TO_STRING),
    emit_method_define!(KILN_C99_METHOD_ARRAY_BUFFER_IS_VIEW),
    r#"
/* The reflective statics, and the one instance method that walks a chain. */
"#,
    emit_method_define!(KILN_C99_METHOD_OBJECT_GET_PROTOTYPE_OF),
    emit_method_define!(KILN_C99_METHOD_OBJECT_SET_PROTOTYPE_OF),
    emit_method_define!(KILN_C99_METHOD_OBJECT_GET_OWN_PROPERTY_NAMES),
    emit_method_define!(KILN_C99_METHOD_OBJECT_GET_OWN_PROPERTY_SYMBOLS),
    emit_method_define!(KILN_C99_METHOD_OBJECT_FROM_ENTRIES),
    emit_method_define!(KILN_C99_METHOD_ARRAY_FROM),
    emit_method_define!(KILN_C99_METHOD_IS_PROTOTYPE_OF),
    emit_method_define!(KILN_C99_METHOD_PROPERTY_IS_ENUMERABLE),
    r#"
/* `String.raw`, the one built-in written to be used as a template tag. Numbered with the
 * receiver-less statics because that is what it is: the template object arrives as an argument,
 * not as `this`. */
"#,
    emit_method_define!(KILN_C99_METHOD_STRING_RAW),
    emit_method_define!(KILN_C99_METHOD_STRING_FROM_CHAR_CODE),
    r#"
/* The descriptor half of the reflective statics: the operations that talk about a property
 * rather than through it. Numbered in the receiver-less range, because every one of them takes
 * its subject as an argument. */
"#,
    emit_method_define!(KILN_C99_METHOD_OBJECT_DEFINE_PROPERTY),
    emit_method_define!(KILN_C99_METHOD_OBJECT_DEFINE_PROPERTIES),
    emit_method_define!(KILN_C99_METHOD_OBJECT_GET_OWN_PROPERTY_DESCRIPTOR),
    emit_method_define!(KILN_C99_METHOD_OBJECT_GET_OWN_PROPERTY_DESCRIPTORS),
    r#"
/* The global symbol registry. `Symbol.for` is deliberately separate from `Symbol()` because
 * equal registry keys must return one identity across every module in the realm. */
"#,
    emit_method_define!(KILN_C99_METHOD_SYMBOL_FOR),
    emit_method_define!(KILN_C99_METHOD_SYMBOL_KEY_FOR),
    emit_method_define!(KILN_C99_METHOD_NODE_STAT_IS_FILE),
    emit_method_define!(KILN_C99_METHOD_NODE_STAT_IS_DIRECTORY),
    emit_method_define!(KILN_C99_METHOD_ABORT_CONTROLLER_ABORT),
    emit_method_define!(KILN_C99_METHOD_ABORT_SIGNAL_ABORT),
    emit_method_define!(KILN_C99_METHOD_ABORT_SIGNAL_LISTEN),
    emit_method_define!(KILN_C99_METHOD_ABORT_SIGNAL_UNLISTEN),
    emit_method_define!(KILN_C99_METHOD_ABORT_SIGNAL_THROW),
    emit_method_define!(KILN_C99_METHOD_ABORT_SIGNAL_TIMEOUT),
    emit_method_define!(KILN_C99_METHOD_ABORT_SIGNAL_ANY),
    emit_method_define!(KILN_C99_METHOD_ABORT_SIGNAL_SETTLE),
    emit_method_define!(KILN_C99_METHOD_ABORT_SIGNAL_FOLLOW),
    emit_method_define!(KILN_C99_METHOD_TEXT_ENCODER_ENCODE),
    emit_method_define!(KILN_C99_METHOD_TEXT_DECODER_DECODE),
    emit_method_define!(KILN_C99_METHOD_NODE_ASYNC_LOCAL_STORAGE),
    emit_method_define!(KILN_C99_METHOD_NODE_ALS_GET_STORE),
    emit_method_define!(KILN_C99_METHOD_NODE_ALS_RUN),
    emit_method_define!(KILN_C99_METHOD_NODE_ALS_ENTER_WITH),
    emit_method_define!(KILN_C99_METHOD_NODE_ALS_EXIT),
    emit_method_define!(KILN_C99_METHOD_NODE_ALS_DISABLE),
    emit_method_define!(KILN_C99_METHOD_STRING_FROM_CODE_POINT),
    emit_method_define!(KILN_C99_METHOD_ARRAY_PARALLEL_MAP),
    emit_method_define!(KILN_C99_METHOD_NODE_EVENT_LISTENER_COUNT),
    emit_method_define!(KILN_C99_METHOD_PROCESS_KILL),
    emit_method_define!(KILN_C99_METHOD_NODE_READABLE_SET_RAW_MODE),
    emit_method_define!(KILN_C99_METHOD_NODE_OS_HOMEDIR),
    emit_method_define!(KILN_C99_METHOD_NODE_OS_TMPDIR),
    emit_method_define!(KILN_C99_METHOD_NODE_FS_MKDIR),
    emit_method_define!(KILN_C99_METHOD_UINT8_TO_HEX_RAW),
    emit_method_define!(KILN_C99_METHOD_UINT8_TO_BASE64_RAW),
    emit_method_define!(KILN_C99_METHOD_URL_PARSE_RAW),
    emit_method_define!(KILN_C99_METHOD_URL_SET_RAW),
    emit_method_define!(KILN_C99_METHOD_URL_FAST_RAW),
    emit_method_define!(KILN_C99_METHOD_URL_ADOPT_RAW),
    emit_method_define!(KILN_C99_METHOD_ARRAY_BUFFER_DETACH_RAW),
    emit_method_define!(KILN_C99_METHOD_CLONE_DATA_RAW),
    emit_method_define!(KILN_C99_METHOD_JOBS_PENDING_RAW),
    emit_method_define!(KILN_C99_METHOD_QUEUE_IO_RAW),
    emit_method_define!(KILN_C99_METHOD_PROMISE_REJECTIONS_RAW),
    emit_method_define!(KILN_C99_METHOD_TYPED_ARRAY_COPY_RAW),
    emit_method_define!(KILN_C99_METHOD_TYPED_ARRAY_SORT_RAW),
    emit_method_define!(KILN_C99_METHOD_WORKER_OP_RAW),
    emit_method_define!(KILN_C99_METHOD_PORT_WAIT_RAW),
    emit_method_define!(KILN_C99_METHOD_SHARED_BUFFER_RAW),
    emit_method_define!(KILN_C99_METHOD_ATOMICS_RAW),
    emit_method_define!(KILN_C99_METHOD_FS_DESCRIPTOR_RAW),
    emit_method_define!(KILN_C99_METHOD_SHARE_RAW),
    emit_method_define!(KILN_C99_METHOD_SHARED_IS_RAW),
    emit_method_define!(KILN_C99_METHOD_SHARED_WEAK_RAW),
    emit_method_define!(KILN_C99_METHOD_COLLECTION_SIZE_RAW),
    emit_method_define!(KILN_C99_METHOD_JSON_UNBOX_RAW),
    emit_method_define!(KILN_C99_METHOD_BOOLEAN_VALUE_OF),
    emit_method_define!(KILN_C99_METHOD_SYMBOL_VALUE_OF),
    emit_method_define!(KILN_C99_METHOD_PROCESS_EXIT),
    emit_method_define!(KILN_C99_METHOD_NODE_FS_READ_FILE_SYNC),
    emit_method_define!(KILN_C99_METHOD_NODE_FS_WRITE_FILE_SYNC),
    emit_method_define!(KILN_C99_METHOD_NODE_FS_MKDIR_SYNC),
    emit_method_define!(KILN_C99_METHOD_NODE_FS_STAT_SYNC),
    emit_method_define!(KILN_C99_METHOD_NODE_FS_LSTAT_SYNC),
    emit_method_define!(KILN_C99_METHOD_NODE_FS_READDIR_SYNC),
    emit_method_define!(KILN_C99_METHOD_NODE_FS_REALPATH_SYNC),
    emit_method_define!(KILN_C99_METHOD_NODE_FS_UNLINK_SYNC),
    emit_method_define!(KILN_C99_METHOD_NODE_FS_RMDIR_SYNC),
    emit_method_define!(KILN_C99_METHOD_NODE_STAT_IS_SYMBOLIC_LINK),
    emit_method_define!(KILN_C99_METHOD_NODE_HTTP_SERVER_RESPONSE),
    emit_method_define!(KILN_C99_METHOD_NODE_QUERYSTRING_PARSE),
    r#"
/* `Object.is`: `===` except that it separates the two zeroes and finds NaN. */
"#,
    emit_method_define!(KILN_C99_METHOD_OBJECT_IS),
    r#"
/* Private frame-snapshot primitive captured by the TypeScript Error.captureStackTrace facade. */
"#,
    emit_method_define!(KILN_C99_METHOD_ERROR_CAPTURE_STACK_TRACE),
    r#"
/* `Reflect`: the operations a proxy handler forwards with, as ordinary functions.
 *
 * They are not aliases for the `Object` statics that look like them. `Reflect.get` takes a
 * *receiver*, which is what a forwarding handler has to pass on for an inherited accessor to see
 * the proxy; `Reflect.set` and `Reflect.deleteProperty` answer with a boolean where the operator
 * forms answer with the value; and none of them coerces a primitive target -- `Reflect.get(1, k)`
 * is a TypeError where `(1)[k]` is a read off the string prototype. */
"#,
    emit_method_define!(KILN_C99_METHOD_REFLECT_OWN_KEYS),
    emit_method_define!(KILN_C99_METHOD_REFLECT_GET),
    emit_method_define!(KILN_C99_METHOD_REFLECT_SET),
    emit_method_define!(KILN_C99_METHOD_REFLECT_HAS),
    emit_method_define!(KILN_C99_METHOD_REFLECT_DELETE_PROPERTY),
    emit_method_define!(KILN_C99_METHOD_REFLECT_APPLY),
    emit_method_define!(KILN_C99_METHOD_REFLECT_CONSTRUCT),
    emit_method_define!(KILN_C99_METHOD_REFLECT_DEFINE_PROPERTY),
    emit_method_define!(KILN_C99_METHOD_REFLECT_GET_PROTOTYPE_OF),
    emit_method_define!(KILN_C99_METHOD_GENERATOR_THROW),
    r#"
/* `Proxy.revocable(target, handler)` and the function it hands back. The second is reached only
 * through the first, as a bound method whose receiver is the proxy it retires -- which is what
 * gives the returned function the one capability it needs and nothing else. */
"#,
    emit_method_define!(KILN_C99_METHOD_PROXY_REVOCABLE),
    emit_method_define!(KILN_C99_METHOD_PROXY_REVOKE),
    r#"
/* `Math`, part two -- numbered here rather than beside the first range because that range ends
 * where `Number`'s begins. Still below the array boundary, because these take no receiver. */
"#,
    emit_method_define!(KILN_C99_METHOD_MATH_RANDOM),
    emit_method_define!(KILN_C99_METHOD_MATH_SINH),
    emit_method_define!(KILN_C99_METHOD_MATH_COSH),
    emit_method_define!(KILN_C99_METHOD_MATH_TANH),
    emit_method_define!(KILN_C99_METHOD_MATH_ASINH),
    emit_method_define!(KILN_C99_METHOD_MATH_ACOSH),
    emit_method_define!(KILN_C99_METHOD_MATH_ATANH),
    emit_method_define!(KILN_C99_METHOD_MATH_LOG1P),
    emit_method_define!(KILN_C99_METHOD_MATH_EXPM1),
    emit_method_define!(KILN_C99_METHOD_MATH_FROUND),
    emit_method_define!(KILN_C99_METHOD_MATH_CLZ32),
    emit_method_define!(KILN_C99_METHOD_MATH_IMUL),
    r#"
/* What a regular expression answers to. Below the array boundary because they take a receiver
 * exactly as the number methods do. */
"#,
    emit_method_define!(KILN_C99_METHOD_REGEXP_TEST),
    emit_method_define!(KILN_C99_METHOD_REGEXP_EXEC),
    emit_method_define!(KILN_C99_METHOD_REGEXP_TO_STRING),
    r#"/* A collection's own `next`, distinct from the array iterator's because it walks the collection
 * itself rather than a copy of it -- which is the only way a delete during a loop can be seen.
 * Numbered here rather than beside the collection block because the guard that routes a code to
 * a receiver kind is a range, and this has to land inside the one that answers a method reached
 * through an object. */
"#,
    emit_method_define!(KILN_C99_METHOD_COLLECTION_ITERATOR_NEXT),
    r#"/* `toString` on a string or a boolean, which is the value itself rather than `[object String]`.
 * Library code reaches for it constantly on values it has already coerced. */
"#,
    emit_method_define!(KILN_C99_METHOD_PRIMITIVE_TO_STRING),
    r#"/* The two statics a concrete view constructor carries. Kept below the string block that
 * begins at 240, which `kiln_c99_invoke_method` routes by range. */
"#,
    emit_method_define!(KILN_C99_METHOD_TYPED_OF),
    emit_method_define!(KILN_C99_METHOD_TYPED_FROM),
    r#"/* `Array.prototype.map/filter` keep their generic semantics when borrowed onto a typed array.
 * They need distinct codes from the intrinsic typed-array methods, which return a typed view. */
"#,
    emit_method_define!(KILN_C99_METHOD_ARRAY_GENERIC_MAP),
    emit_method_define!(KILN_C99_METHOD_ARRAY_GENERIC_FILTER),
    r#"/* Runtime-module methods. They use the same canonical call path as language intrinsics while
 * keeping their state on ordinary backend objects. Appended outside every range-dispatch block
 * so adding a runtime facade never changes the meaning of an existing intrinsic code. */
"#,
    emit_method_define!(KILN_C99_METHOD_NODE_EVENT_ON),
    emit_method_define!(KILN_C99_METHOD_NODE_EVENT_ONCE),
    emit_method_define!(KILN_C99_METHOD_NODE_EVENT_EMIT),
    emit_method_define!(KILN_C99_METHOD_NODE_EVENT_LISTENERS),
    emit_method_define!(KILN_C99_METHOD_NODE_EVENT_EVENT_NAMES),
    emit_method_define!(KILN_C99_METHOD_NODE_EVENT_REMOVE_LISTENER),
    emit_method_define!(KILN_C99_METHOD_NODE_EVENT_REMOVE_ALL_LISTENERS),
    emit_method_define!(KILN_C99_METHOD_NODE_EVENT_SET_MAX_LISTENERS),
    emit_method_define!(KILN_C99_METHOD_NODE_EVENT_EMITTER),
    emit_method_define!(KILN_C99_METHOD_NODE_NET_CREATE_SERVER),
    emit_method_define!(KILN_C99_METHOD_NODE_SERVER_LISTEN),
    emit_method_define!(KILN_C99_METHOD_NODE_SERVER_CLOSE),
    emit_method_define!(KILN_C99_METHOD_NODE_SERVER_ADDRESS),
    emit_method_define!(KILN_C99_METHOD_NODE_SOCKET_WRITE),
    emit_method_define!(KILN_C99_METHOD_NODE_SOCKET_END),
    emit_method_define!(KILN_C99_METHOD_NODE_SOCKET_DESTROY),
    emit_method_define!(KILN_C99_METHOD_NODE_HTTP_CREATE_SERVER),
    emit_method_define!(KILN_C99_METHOD_NODE_RESPONSE_SET_HEADER),
    emit_method_define!(KILN_C99_METHOD_NODE_RESPONSE_WRITE_HEAD),
    emit_method_define!(KILN_C99_METHOD_NODE_RESPONSE_WRITE),
    emit_method_define!(KILN_C99_METHOD_NODE_RESPONSE_END),
    emit_method_define!(KILN_C99_METHOD_NODE_HTTP_GET),
    emit_method_define!(KILN_C99_METHOD_NODE_HTTP_REQUEST),
    emit_method_define!(KILN_C99_METHOD_NODE_HTTP_CREATE_FETCH_SERVER),
    emit_method_define!(KILN_C99_METHOD_NODE_HTTP_FETCH_RESPOND),
    emit_method_define!(KILN_C99_METHOD_NODE_CLIENT_REQUEST_WRITE),
    emit_method_define!(KILN_C99_METHOD_NODE_CLIENT_REQUEST_END),
    emit_method_define!(KILN_C99_METHOD_NODE_REQUEST_HEADERS_GET),
    emit_method_define!(KILN_C99_METHOD_NODE_REQUEST_HEADERS_SET),
    emit_method_define!(KILN_C99_METHOD_NODE_READABLE_SET_ENCODING),
    emit_method_define!(KILN_C99_METHOD_NODE_NET_CONNECT),
    emit_method_define!(KILN_C99_METHOD_URL_SEARCH_PARAMS_GET),
    emit_method_define!(KILN_C99_METHOD_BUFFER_FROM),
    emit_method_define!(KILN_C99_METHOD_BUFFER_ALLOC),
    emit_method_define!(KILN_C99_METHOD_BUFFER_IS_BUFFER),
    emit_method_define!(KILN_C99_METHOD_BUFFER_BYTE_LENGTH),
    emit_method_define!(KILN_C99_METHOD_BUFFER_CONCAT),
    emit_method_define!(KILN_C99_METHOD_BUFFER_TO_STRING),
    emit_method_define!(KILN_C99_METHOD_BUFFER_TO_STRING_RAW),
    emit_method_define!(KILN_C99_METHOD_NODE_UTIL_DEBUGLOG),
    emit_method_define!(KILN_C99_METHOD_NODE_UTIL_NOOP),
    emit_method_define!(KILN_C99_METHOD_NODE_UTIL_DEPRECATE),
    emit_method_define!(KILN_C99_METHOD_NODE_UTIL_INSPECT),
    emit_method_define!(KILN_C99_METHOD_NODE_UTIL_INHERITS),
    emit_method_define!(KILN_C99_METHOD_NODE_UTIL_FORMAT),
    emit_method_define!(KILN_C99_METHOD_NODE_DIAGNOSTICS_CHANNEL),
    emit_method_define!(KILN_C99_METHOD_NODE_DIAGNOSTICS_TRACING_CHANNEL),
    emit_method_define!(KILN_C99_METHOD_NODE_DIAGNOSTICS_PUBLISH),
    emit_method_define!(KILN_C99_METHOD_NODE_DIAGNOSTICS_RUN_STORES),
    emit_method_define!(KILN_C99_METHOD_NODE_HTTPS_CREATE_SERVER),
    emit_method_define!(KILN_C99_METHOD_NODE_HTTP2_CREATE_SERVER),
    emit_method_define!(KILN_C99_METHOD_NODE_HTTP2_CREATE_SECURE_SERVER),
    emit_method_define!(KILN_C99_METHOD_NODE_DNS_LOOKUP),
    emit_method_define!(KILN_C99_METHOD_NODE_OS_HOSTNAME),
    emit_method_define!(KILN_C99_METHOD_NODE_OS_NETWORK_INTERFACES),
    emit_method_define!(KILN_C99_METHOD_NODE_STREAM),
    emit_method_define!(KILN_C99_METHOD_NODE_READABLE),
    emit_method_define!(KILN_C99_METHOD_NODE_WRITABLE),
    emit_method_define!(KILN_C99_METHOD_NODE_STREAM_ADD_ABORT_SIGNAL),
    emit_method_define!(KILN_C99_METHOD_NODE_STREAM_FINISHED),
    emit_method_define!(KILN_C99_METHOD_NODE_READABLE_PUSH),
    emit_method_define!(KILN_C99_METHOD_NODE_READABLE_READ),
    emit_method_define!(KILN_C99_METHOD_NODE_READABLE_DESTROY),
    emit_method_define!(KILN_C99_METHOD_NODE_READABLE_PAUSE),
    emit_method_define!(KILN_C99_METHOD_NODE_READABLE_RESUME),
    emit_method_define!(KILN_C99_METHOD_NODE_READABLE_PIPE),
    emit_method_define!(KILN_C99_METHOD_NODE_READABLE_UNPIPE),
    emit_method_define!(KILN_C99_METHOD_NODE_WRITABLE_WRITE),
    emit_method_define!(KILN_C99_METHOD_NODE_WRITABLE_END),
    emit_method_define!(KILN_C99_METHOD_NODE_WRITABLE_DESTROY),
    emit_method_define!(KILN_C99_METHOD_PERFORMANCE_NOW),
    emit_method_define!(KILN_C99_METHOD_NODE_FS_EXISTS_SYNC),
    emit_method_define!(KILN_C99_METHOD_NODE_FS_READ_FILE),
    emit_method_define!(KILN_C99_METHOD_NODE_FS_STAT),
    emit_method_define!(KILN_C99_METHOD_NODE_FS_WRITE_FILE),
    emit_method_define!(KILN_C99_METHOD_NODE_PATH_IS_ABSOLUTE),
    emit_method_define!(KILN_C99_METHOD_NODE_PATH_JOIN),
    emit_method_define!(KILN_C99_METHOD_NODE_PATH_RESOLVE),
    emit_method_define!(KILN_C99_METHOD_NODE_PATH_DIRNAME),
    emit_method_define!(KILN_C99_METHOD_NODE_PATH_BASENAME),
    emit_method_define!(KILN_C99_METHOD_NODE_PATH_EXTNAME),
    emit_method_define!(KILN_C99_METHOD_NODE_PATH_RELATIVE),
    emit_method_define!(KILN_C99_METHOD_NODE_BUILTIN_UNMATERIALIZED),
    emit_method_define!(KILN_C99_METHOD_NODE_ASSERT_OK),
    emit_method_define!(KILN_C99_METHOD_NODE_WORKER_CONSTRUCTOR),
    emit_method_define!(KILN_C99_METHOD_NODE_MODULE_CREATE_REQUIRE),
    emit_method_define!(KILN_C99_METHOD_NODE_MODULE),
    emit_method_define!(KILN_C99_METHOD_NODE_FFI_DLOPEN),
    emit_method_define!(KILN_C99_METHOD_NODE_FFI_GET_RAW_POINTER),
    emit_method_define!(KILN_C99_METHOD_NODE_FFI_TO_ARRAY_BUFFER),
    emit_method_define!(KILN_C99_METHOD_NODE_FFI_REGISTER_CALLBACK),
    emit_method_define!(KILN_C99_METHOD_NODE_FFI_UNREGISTER_CALLBACK),
    emit_method_define!(KILN_C99_METHOD_NODE_FFI_CLOSE),
    emit_method_define!(KILN_C99_METHOD_INTL_SEGMENT_RAW),
    emit_method_define!(KILN_C99_METHOD_STRING_LOCALE_CASE_RAW),
    emit_method_define!(KILN_C99_METHOD_INTL_DATETIME_FORMAT_RAW),
    emit_method_define!(KILN_C99_METHOD_NODE_URL_FILE_URL_TO_PATH),
    emit_method_define!(KILN_C99_METHOD_NODE_URL_PARSE),
    emit_method_define!(KILN_C99_METHOD_NODE_URL_PATH_TO_FILE_URL),
    emit_method_define!(KILN_C99_METHOD_NODE_ASYNC_RESOURCE),
    emit_method_define!(KILN_C99_METHOD_NODE_ASYNC_RESOURCE_BIND),
    emit_method_define!(KILN_C99_METHOD_NODE_ASYNC_RESOURCE_DESTROY),
    emit_method_define!(KILN_C99_METHOD_NODE_CRYPTO_RANDOM_UUID),
    emit_method_define!(KILN_C99_METHOD_NODE_CRYPTO_CREATE_HASH),
    emit_method_define!(KILN_C99_METHOD_PROCESS_NEXT_TICK),
    emit_method_define!(KILN_C99_METHOD_PROCESS_START_RAW),
    emit_method_define!(KILN_C99_METHOD_PROCESS_CANCEL_RAW),
    emit_method_define!(KILN_C99_METHOD_PROCESS_HRTIME),
    emit_method_define!(KILN_C99_METHOD_PROCESS_HRTIME_BIGINT),
    emit_method_define!(KILN_C99_METHOD_PROCESS_CWD),
    emit_method_define!(KILN_C99_METHOD_ATOMICS_UNAVAILABLE),
    emit_method_define!(KILN_C99_METHOD_CONSOLE_LOG),
    emit_method_define!(KILN_C99_METHOD_CONSOLE_ERROR),
    emit_method_define!(KILN_C99_METHOD_SET_IMMEDIATE),
    emit_method_define!(KILN_C99_METHOD_CLEAR_IMMEDIATE),
    emit_method_define!(KILN_C99_METHOD_NODE_HTTP_SERVER),
    emit_method_define!(KILN_C99_METHOD_NODE_SERVER_SET_TIMEOUT),
    emit_method_define!(KILN_C99_METHOD_NODE_RESPONSE_GET_HEADER),
    emit_method_define!(KILN_C99_METHOD_NODE_RESPONSE_GET_HEADERS),
    emit_method_define!(KILN_C99_METHOD_NODE_RESPONSE_HAS_HEADER),
    emit_method_define!(KILN_C99_METHOD_NODE_RESPONSE_REMOVE_HEADER),
    r#"/* `Temporal` methods. Numbered above the node block and routed by an explicit range at the top of
 * `kiln_c99_invoke_method`, because the string range below this one is open-ended. */
/* `from` and `compare` are one function per type in the language -- `Temporal.PlainDate.from`
 * accepts a different bag from `Temporal.Duration.from` -- and the call path carries only the
 * method code, not the value the method was reached through. So the kind rides in the code: the
 * eight are contiguous and the base is subtracted to recover it. */
"#,
    emit_method_define!(KILN_C99_METHOD_TEMPORAL_FROM),
    emit_method_define!(KILN_C99_METHOD_TEMPORAL_FROM_LAST),
    emit_method_define!(KILN_C99_METHOD_TEMPORAL_COMPARE),
    emit_method_define!(KILN_C99_METHOD_TEMPORAL_COMPARE_LAST),
    emit_method_define!(KILN_C99_METHOD_TEMPORAL_TO_STRING),
    emit_method_define!(KILN_C99_METHOD_TEMPORAL_TO_JSON),
    emit_method_define!(KILN_C99_METHOD_TEMPORAL_VALUE_OF),
    emit_method_define!(KILN_C99_METHOD_TEMPORAL_NOW_TIME_ZONE_ID),
    r#"/* `Temporal.Now`'s five readers, contiguous so the kind rides in the code the way it does for
 * `from` and `compare`. The clock they read is the realm's, which ADR 0015 makes virtual and UTC.
 * The order is the eight-type order, restricted to the five the namespace offers. */
"#,
    emit_method_define!(KILN_C99_METHOD_TEMPORAL_NOW_READ),
    emit_method_define!(KILN_C99_METHOD_TEMPORAL_NOW_READ_LAST),
    r#"/* The arithmetic and conversion surface. One code per name, contiguous, because the call path
 * carries only the code -- and the provider is keyed by the name, so the two are the same table
 * read from either end (`KILN_C99_TEMPORAL_OPS` below). */
"#,
    emit_method_define!(KILN_C99_METHOD_TEMPORAL_OP),
    emit_method_define!(KILN_C99_METHOD_TEMPORAL_OP_LAST),
    r#"/* `Temporal.Instant`'s two epoch constructors. Statics rather than `from`, and one code each
 * rather than a kind-bearing range, because only `Instant` has them: an epoch count names a point
 * on the timeline, and no other one of the eight is a point on a timeline by itself. */
"#,
    emit_method_define!(KILN_C99_METHOD_TEMPORAL_INSTANT_FROM_EPOCH_MILLISECONDS),
    emit_method_define!(KILN_C99_METHOD_TEMPORAL_INSTANT_FROM_EPOCH_NANOSECONDS),
    emit_method_define!(KILN_C99_METHOD_TEMPORAL_LAST),
    r#"
/* The writers that mirror the readers at 74..82. Contiguous, because a range is how a receiver's
 * methods are found by name, and numbered here rather than beside the readers because that block
 * is full. */
"#,
    emit_method_define!(KILN_C99_METHOD_SET_TIME),
    r#"
/* The iterator helpers. 470..481 is a name range -- `kiln_c99_lookup_method` walks it to answer
 * `Iterator.prototype.map` -- and 470..484 is the dispatch range. A helper's own `next` and
 * `return` sit past the name range so that reading `Iterator.prototype.next` finds nothing, which
 * is correct: the helpers live on the table and `next` lives on the helper object. */
"#,
    emit_method_define!(KILN_C99_METHOD_ITERATOR_MAP),
    emit_method_define!(KILN_C99_METHOD_ITERATOR_FILTER),
    emit_method_define!(KILN_C99_METHOD_ITERATOR_TAKE),
    emit_method_define!(KILN_C99_METHOD_ITERATOR_DROP),
    emit_method_define!(KILN_C99_METHOD_ITERATOR_FLAT_MAP),
    emit_method_define!(KILN_C99_METHOD_ITERATOR_REDUCE),
    emit_method_define!(KILN_C99_METHOD_ITERATOR_TO_ARRAY),
    emit_method_define!(KILN_C99_METHOD_ITERATOR_FOR_EACH),
    emit_method_define!(KILN_C99_METHOD_ITERATOR_SOME),
    emit_method_define!(KILN_C99_METHOD_ITERATOR_EVERY),
    emit_method_define!(KILN_C99_METHOD_ITERATOR_FIND),
    emit_method_define!(KILN_C99_METHOD_ITERATOR_INCLUDES),
    emit_method_define!(KILN_C99_METHOD_ITERATOR_HELPER_NEXT),
    emit_method_define!(KILN_C99_METHOD_ITERATOR_HELPER_RETURN),
    emit_method_define!(KILN_C99_METHOD_ITERATOR_FROM),
    r#"
/* Dispatch membership is generated from the Rust method registry.  Keep these predicates exact:
 * numeric adjacency is an implementation detail, not a category contract. */
#define KILN_C99_GROUP_DIRECT UINT32_C(1)
#define KILN_C99_GROUP_ARRAY_LOOKUP UINT32_C(2)
#define KILN_C99_GROUP_STRING_DISPATCH UINT32_C(4)
#define KILN_C99_GROUP_GENERIC UINT32_C(8)
#define KILN_C99_GROUP_TEMPORAL UINT32_C(16)
#define KILN_C99_GROUP_TEMPORAL_FROM UINT32_C(32)
#define KILN_C99_GROUP_TEMPORAL_COMPARE UINT32_C(64)
#define KILN_C99_GROUP_TEMPORAL_NOW_READ UINT32_C(128)
#define KILN_C99_GROUP_TEMPORAL_OP UINT32_C(256)
#define KILN_C99_GROUP_DATE_SETTER UINT32_C(512)
#define KILN_C99_GROUP_NODE_EVENT UINT32_C(1024)
#define KILN_C99_GROUP_NODE_NET UINT32_C(2048)
#define KILN_C99_GROUP_NODE_SERVER UINT32_C(4096)
#define KILN_C99_GROUP_NODE_HTTP UINT32_C(8192)
#define KILN_C99_GROUP_NODE_RESPONSE UINT32_C(16384)
#define KILN_C99_GROUP_BUFFER UINT32_C(32768)
#define KILN_C99_GROUP_NODE_UTIL UINT32_C(65536)
#define KILN_C99_GROUP_NODE_DIAGNOSTICS UINT32_C(131072)
#define KILN_C99_GROUP_NODE_STREAM UINT32_C(262144)
#define KILN_C99_GROUP_NODE_PATH UINT32_C(524288)
#define KILN_C99_GROUP_MATH_NUMBER UINT32_C(1048576)

/* KILN_RUNTIME_ONLY_METHOD_GROUPS_BEGIN */
static uint32_t kiln_c99_method_groups(uint64_t code) {
    return kiln_c99_method_metadata_for(code).groups;
}

static int kiln_c99_method_in_group(uint64_t code, uint32_t group) {
    return (kiln_c99_method_metadata_for(code).groups & group) != 0;
}
/* KILN_RUNTIME_ONLY_METHOD_GROUPS_END */
"#,
            concat!(kiln_native_runtime::kiln_value_abi_declaration!(), include_str!("runtime/prelude_types.h")),
            kiln_native_provider_contract::provider_abi_registry!(provider_c_declaration),
        )
    };
}

pub(crate) const HEADER: &str = concat!(
    header_prefix!(),
    header_static_diagnostics!(),
    method_metadata_decl!(),
    header_rest!(),
    method_metadata_part!(),
    runtime_parts!(),
    emit_method_code_list!(),
    r#"};

/* Every method the backend can dispatch, ordered by name so a lookup is a binary search.
 *
 * The walk this replaces asked `kiln_c99_method_name` for a name, then `strlen`, then `memcmp`,
 * for every code in the group -- and checked group membership for every code in the table, in
 * group or not. Profiled on the mixed server workload it was 5,427,319 name fetches across
 * 303,496 lookups, about eighteen per lookup over a table of a few hundred.
 *
 * The table is immutable: the codes are constants and the names are literals, so the index is
 * process-global rather than per-realm, and is built once. Two threads racing to build it write
 * identical bytes -- the input is a compile-time constant either way -- so the only cost of the
 * race is doing the work twice. */
typedef struct kiln_method_entry {
    const char *name;
    size_t len;
    uint64_t code;
} kiln_method_entry;

/* Sized from the code table directly rather than through a `#define`: a `KILN_C99_METHOD_*` macro
 * is how this prelude spells a method id, and the guard that checks every id is nameable reads
 * them by prefix. A macro named like an id that is not one is a trap for that reader. */
static kiln_method_entry kiln_c99_method_index[sizeof kiln_c99_method_codes / sizeof *kiln_c99_method_codes];

static uint64_t kiln_c99_method_code_end(void) {
    /* One word, the same from every thread that computes it: a racing second computation only
     * repeats the first. */
    static uint64_t kiln_c99_method_code_end_cached = 0;
    uint64_t end = __atomic_load_n(&kiln_c99_method_code_end_cached, __ATOMIC_RELAXED);
    size_t index;
    if (end != 0) return end;
    for (index = 0; index < sizeof kiln_c99_method_codes / sizeof *kiln_c99_method_codes; ++index) {
        if ((uint64_t)kiln_c99_method_codes[index] >= end) end = (uint64_t)kiln_c99_method_codes[index] + 1;
    }
    __atomic_store_n(&kiln_c99_method_code_end_cached, end, __ATOMIC_RELAXED);
    return end;
}
static size_t kiln_c99_method_index_len;
static int kiln_c99_method_index_ready;

/* Names first, and equal names by code, so the run of codes sharing a name stays contiguous and
 * in a stable order: several groups spell a method `get`, and the walk this replaces returned the
 * first such code in the table that was in the asked-for group. */
static int kiln_c99_method_entry_before(const kiln_method_entry *left,
                                        const kiln_method_entry *right) {
    size_t shortest = left->len < right->len ? left->len : right->len;
    int ordering = memcmp(left->name, right->name, shortest);
    if (ordering != 0) return ordering < 0;
    if (left->len != right->len) return left->len < right->len;
    return left->code < right->code;
}

static void kiln_c99_method_index_build(void) {
    size_t index;
    size_t count = 0;
    for (index = 0; index < sizeof kiln_c99_method_codes / sizeof *kiln_c99_method_codes;
         ++index) {
        uint64_t code = kiln_c99_method_codes[index];
        const char *name = kiln_c99_method_name(code);
        if (name == NULL) continue;
        kiln_c99_method_index[count].name = name;
        kiln_c99_method_index[count].len = strlen(name);
        kiln_c99_method_index[count].code = code;
        ++count;
    }
    /* Insertion sort: a few hundred entries, once per process, and it keeps the prelude free of
     * a comparator indirection through `qsort`. */
    for (index = 1; index < count; ++index) {
        kiln_method_entry pending = kiln_c99_method_index[index];
        size_t behind = index;
        while (behind > 0 && kiln_c99_method_entry_before(&pending,
                                                          &kiln_c99_method_index[behind - 1])) {
            kiln_c99_method_index[behind] = kiln_c99_method_index[behind - 1];
            --behind;
        }
        kiln_c99_method_index[behind] = pending;
    }
    kiln_c99_method_index_len = count;
}

static uint64_t kiln_c99_lookup_method_group(const char *key, size_t key_len, uint32_t group) {
    size_t low = 0;
    size_t high;
    size_t at;
    if (key == NULL || key_len == 0) return 0;
    kiln_c99_once(&kiln_c99_method_index_ready, kiln_c99_method_index_build);
    high = kiln_c99_method_index_len;
    while (low < high) {
        size_t middle = low + (high - low) / 2;
        const kiln_method_entry *entry = &kiln_c99_method_index[middle];
        size_t shortest = entry->len < key_len ? entry->len : key_len;
        int ordering = memcmp(entry->name, key, shortest);
        if (ordering == 0 && entry->len != key_len) ordering = entry->len < key_len ? -1 : 1;
        if (ordering < 0) {
            low = middle + 1;
        } else {
            high = middle;
        }
    }
    /* `low` is the first entry not ordered before the key. Names that match are contiguous from
     * there, and the first of them in the asked-for group is the answer the walk would have given. */
    for (at = low; at < kiln_c99_method_index_len; ++at) {
        const kiln_method_entry *entry = &kiln_c99_method_index[at];
        if (entry->len != key_len || memcmp(entry->name, key, key_len) != 0) break;
        if (kiln_c99_method_in_group(entry->code, group)) return entry->code;
    }
    return 0;
}

/* A few prototype tables intentionally share a code span with another table. Their existing
 * explicit endpoints remain a lookup detail; the receiver-dispatch paths use generated groups.
 *
 * This used to walk `code` from `first` to `last`, asking `kiln_c99_method_name` for each and
 * `strlen`+`memcmp`-ing the result against the key -- the same shape `kiln_c99_lookup_method_group`
 * replaced for the array and string dispatch paths, and just as hot here: every JSON `toJSON`
 * probe on a plain object or array reaches a range through this function on its way to answering
 * "no such method", which is the common case and used to cost the whole range every time.
 *
 * `kiln_c99_method_index` is sorted by name already and holds every code the table above builds
 * from, `[first, last]` included, so the same binary search answers a range exactly as it answers
 * a group: find the run of entries whose name equals the key, and return the first of that run
 * whose code falls in range. Entries with equal names are ordered by ascending code (see
 * `kiln_c99_method_entry_before`), so that is the same code the walk would have found first
 * scanning up from `first` -- the lowest in-range code with a matching name, or 0. */
static uint64_t kiln_c99_lookup_method(const char *key, size_t key_len, uint64_t first,
                                       uint64_t last) {
    size_t low = 0;
    size_t high;
    size_t at;
    if (key == NULL || key_len == 0) return 0;
    kiln_c99_once(&kiln_c99_method_index_ready, kiln_c99_method_index_build);
    high = kiln_c99_method_index_len;
    while (low < high) {
        size_t middle = low + (high - low) / 2;
        const kiln_method_entry *entry = &kiln_c99_method_index[middle];
        size_t shortest = entry->len < key_len ? entry->len : key_len;
        int ordering = memcmp(entry->name, key, shortest);
        if (ordering == 0 && entry->len != key_len) ordering = entry->len < key_len ? -1 : 1;
        if (ordering < 0) {
            low = middle + 1;
        } else {
            high = middle;
        }
    }
    for (at = low; at < kiln_c99_method_index_len; ++at) {
        const kiln_method_entry *entry = &kiln_c99_method_index[at];
        if (entry->len != key_len || memcmp(entry->name, key, key_len) != 0) break;
        if (entry->code >= first && entry->code <= last) return entry->code;
    }
    return 0;
}

/* `Iterator.prototype`'s members, by name.
 *
 * The 470..481 range is the table: adding a helper is adding a code with a name to it, and this
 * function does not have to be edited. `@@iterator` is answered off the range because an iterator
 * is its own iterable and the function that says so is the one every *other* iterator in this
 * backend answers with -- a helper and an array iterator must hand back the same value, or
 * `it[Symbol.iterator] === it[Symbol.iterator]` stops being an identity a program can rely on. */
static uint64_t kiln_c99_iterator_prototype_method(const char *key, size_t key_len) {
    if (kiln_c99_same_name(key, key_len, "@@iterator")) {
        return KILN_C99_METHOD_ITERATOR_SELF;
    }
    return kiln_c99_lookup_method(key, key_len, KILN_C99_METHOD_ITERATOR_MAP,
                                  KILN_C99_METHOD_ITERATOR_INCLUDES);
}

static int kiln_c99_extension_withheld(uint64_t builtin, const char *key, size_t key_len);

/* An array's methods by name. A kiln extension the realm withholds (ADR 0068) is not one of them,
 * so `[].parallelMap` reads `undefined` under `KILN_EXTENSIONS=0` exactly as it does in Node. */
static uint64_t kiln_c99_array_method(const char *key, size_t key_len) {
    uint64_t code = kiln_c99_lookup_method_group(key, key_len, KILN_C99_GROUP_ARRAY_LOOKUP);
    if (code != 0 && kiln_c99_extension_withheld(KILN_C99_BUILTIN_ARRAY_PROTOTYPE, key, key_len)) {
        return 0;
    }
    return code;
}

/* Whether a name is a member of `Array.prototype` *in the language*, and the same question for
 * `Date.prototype`. Both are the string list's question, one type over, and both were missing.
 *
 * `a["nope"]` and `date["nope"]` are not gaps in this backend -- they are what the language
 * answers, which is undefined -- and refusing them ended the process where it should have
 * answered. A bundled `vue-reactivity` reads arbitrary names off an array because that is what a
 * reactive proxy does, and `dayjs` does the same to a date; both stopped here.
 *
 * Keeping the list is what preserves the other half. A name that really is a member but is not
 * implemented yet still refuses by name, so `a.toLocaleString()` says so rather than becoming
 * "undefined is not a function" three frames from the cause -- and, worse, rather than quietly
 * failing a `typeof a.toLocaleString === "function"` feature test. The four copying methods used
 * to be this comment's example and are now implemented, which is what a name on this list is
 * supposed to do: hold the refusal open until somebody answers it. */
static int kiln_c99_is_array_member(const char *key, size_t key_len) {
    static const char *const members[] = {
        "at", "concat", "constructor", "copyWithin", "entries", "every", "fill", "filter", "find",
        "findIndex", "findLast", "findLastIndex", "flat", "flatMap", "forEach", "includes",
        "indexOf", "join", "keys", "lastIndexOf", "length", "map", "parallelMap", "pop", "push",
        "reduce", "reduceRight", "reverse", "shift", "slice", "some", "sort", "splice",
        "toLocaleString", "toReversed", "toSorted", "toSpliced", "toString", "unshift", "values",
        "with"};
    size_t index;
    /* A withheld kiln extension is not a member at all (ADR 0068), which is the answer that keeps
     * a read of it `undefined` rather than a refusal. */
    if (kiln_c99_extension_withheld(KILN_C99_BUILTIN_ARRAY_PROTOTYPE, key, key_len)) {
        return 0;
    }
    for (index = 0; index < sizeof members / sizeof *members; ++index) {
        if (kiln_c99_same_name(key, key_len, members[index])) {
            return 1;
        }
    }
    /* `@@iterator` and `@@unscopables` are members too, and reserved keys reach here spelled with
     * the `@@` prefix. */
    return kiln_c99_same_name(key, key_len, "@@iterator") ||
           kiln_c99_same_name(key, key_len, "@@unscopables");
}

static int kiln_c99_is_date_member(const char *key, size_t key_len) {
    /* All other Date.prototype members are answered by date.ts from the prototype's store.
     * The toLocale* methods require Intl locale data (ADR 0063 rule 5) and remain unbuilt. */
    static const char *const members[] = {
        "toLocaleDateString", "toLocaleString", "toLocaleTimeString"};
    size_t index;
    for (index = 0; index < sizeof members / sizeof *members; ++index) {
        if (kiln_c99_same_name(key, key_len, members[index])) {
            return 1;
        }
    }
    return 0;
}

/* The same question one type further over: whether a name is a member of the prototype the
 * language gives *this* collection.
 *
 * The four kinds do not share a member list -- a `Set` has no `get`, a `WeakMap` has neither
 * `size` nor an iterator -- so the answer depends on the receiver, not only on the name. Every
 * other name is absent, and absent is undefined: `m.__v_isRef` and `m.nope` are what the
 * language answers, not gaps in this backend, and refusing them ended the process where it
 * should have answered. A bundled `vue-reactivity` reads arbitrary names off a reactive `Map`
 * for exactly the reason it reads them off a reactive array, and stopped here after the array
 * was fixed. */
static int kiln_c99_collection_kind_member(int is_map, int is_weak, const char *key,
                                          size_t key_len) {
    /* `Object.prototype` is above every one of them, so its members count as members here too:
     * a name this backend cannot resolve yet has to keep refusing rather than answer undefined. */
    static const char *const shared[] = {
        "__defineGetter__", "__defineSetter__", "__lookupGetter__", "__lookupSetter__",
        "__proto__", "constructor", "delete", "has", "hasOwnProperty", "isPrototypeOf",
        "propertyIsEnumerable", "toLocaleString", "toString", "valueOf"};
    static const char *const strong[] = {"clear", "entries", "forEach", "keys", "size", "values"};
    size_t index;
    for (index = 0; index < sizeof shared / sizeof *shared; ++index) {
        if (kiln_c99_same_name(key, key_len, shared[index])) {
            return 1;
        }
    }
    if (!is_weak) {
        for (index = 0; index < sizeof strong / sizeof *strong; ++index) {
            if (kiln_c99_same_name(key, key_len, strong[index])) {
                return 1;
            }
        }
    }
    if (is_map && (kiln_c99_same_name(key, key_len, "get") ||
                   kiln_c99_same_name(key, key_len, "set"))) {
        return 1;
    }
    if (!is_map && kiln_c99_same_name(key, key_len, "add")) {
        return 1;
    }
    /* The two symbol-keyed members, spelled the way a reserved key reaches here. They are named
     * rather than answered for the whole `@@` prefix because these prototypes really do carry
     * only these two: `m[Symbol.species]` is absent in the language, and absent is undefined.
     * `@@toStringTag` is what `Object.prototype.toString` reads, so a weak collection has it. */
    if (kiln_c99_same_name(key, key_len, "@@toStringTag")) {
        return 1;
    }
    return !is_weak && kiln_c99_same_name(key, key_len, "@@iterator");
}

/* The same question asked of a live collection, which is the only caller that has one.
 *
 * The backlog is consulted too: a `Set.prototype` member the surface table declares and this
 * backend has not built is a member, whatever the hand list above says. `s.union(x)` read
 * `undefined` off an instance for as long as the list was the only answer, while
 * `Set.prototype.union` refused honestly one path over -- the two paths now agree. */
static int kiln_c99_is_collection_member(const kiln_collection *collection, const char *key,
                                         size_t key_len) {
    if (kiln_c99_collection_kind_member(collection->is_map, collection->is_weak, key, key_len)) {
        return 1;
    }
    return !collection->is_weak &&
           kiln_c99_backlog_names(collection->is_map ? "Map.prototype" : "Set.prototype", key,
                                  key_len);
}

/* Whether a name is a member of `String.prototype` *in the language*, which is a different
 * question from whether this backend implements it.
 *
 * A string's properties are its indices, `length`, and these; anything else a program names is
 * absent, and absent is `undefined`. Refusing every unrecognised name was the bug -- `"abc"[-1]`,
 * `"abc"[1.5]` and `"abc"["nope"]` are not gaps in this backend, they are what the language
 * answers -- and refusing ended the process where it should have answered. Keeping the list is
 * what preserves the other half: a name that really is a member but is not implemented yet still
 * refuses by name, which is the diagnostic the refusal exists to give. */
static int kiln_c99_is_string_member(const char *key, size_t key_len) {
    static const char *const members[] = {
        "anchor",       "at",         "big",           "blink",         "bold",
        "charAt",       "charCodeAt", "codePointAt",   "concat",        "constructor",
        "endsWith",     "fixed",      "fontcolor",     "fontsize",      "includes",
        "indexOf",      "isWellFormed", "italics",     "lastIndexOf",   "link",
        "localeCompare","match",      "matchAll",      "normalize",     "padEnd",
        "padStart",     "repeat",     "replace",       "replaceAll",    "search",
        "slice",        "small",      "split",         "startsWith",    "strike",
        "sub",          "substr",     "substring",     "sup",           "toLocaleLowerCase",
        "toLocaleUpperCase", "toLowerCase", "toString", "toUpperCase",  "toWellFormed",
        "trim",         "trimEnd",    "trimLeft",      "trimRight",     "trimStart",
        "valueOf"};
    size_t index;
    for (index = 0; index < sizeof members / sizeof *members; ++index) {
        if (kiln_c99_same_name(key, key_len, members[index])) {
            return 1;
        }
    }
    return 0;
}

/* Whether a name is a member of `%TypedArray%.prototype` *in the language*, which is a different
 * question from whether this backend implements it -- the same distinction
 * `kiln_c99_is_string_member` draws, and for the same reason.
 *
 * A view's properties are its live indices, its buffer metadata, and the methods above. Anything
 * else a program names is absent, and absent is `undefined`. These are the names that are not
 * absent and are not implemented either: answering `undefined` for one would be a silent wrong
 * answer, so a read of one refuses instead and lands in the backlog by name. */
static int kiln_c99_is_typed_array_member(int byte_view, const char *key, size_t key_len) {
    /* copyWithin, toLocaleString, toReversed, toSorted, with, and the Uint8Array codecs are
     * answered by typedarray.ts. */
    static const char *const byte_members[] = {"setFromBase64", "setFromHex"};
    size_t index;
    if (!byte_view) {
        return 0;
    }
    for (index = 0; index < sizeof byte_members / sizeof *byte_members; ++index) {
        if (kiln_c99_same_name(key, key_len, byte_members[index])) {
            return 1;
        }
    }
    return 0;
}

/* Whether a name is a member of `ArrayBuffer.prototype` *in the language* that this backend does
 * not answer -- the same question `kiln_c99_is_typed_array_member` asks one type over. */
static int kiln_c99_is_array_buffer_member(const char *key, size_t key_len) {
    /* detached is answered by the instance read; resize, transfer and transferToFixedLength by
     * arraybuffer.ts from the prototype's store; maxByteLength and resizable are absent (no
     * resizable buffers). */
    static const char *const members[] = {"immutable", "sliceToImmutable", "transferToImmutable"};
    size_t index;
    for (index = 0; index < sizeof members / sizeof *members; ++index) {
        if (kiln_c99_same_name(key, key_len, members[index])) {
            return 1;
        }
    }
    return 0;
}

/* Whether a name is a member of `DataView.prototype` *in the language* that this backend does not
 * answer -- the same question `kiln_c99_is_typed_array_member` asks one type over, and it has the
 * same two-name answer.
 *
 * `getFloat16` and `setFloat16` are the whole list. Half precision has no decode here and the
 * number provider exports no `f16round` either (`Math.f16round` is in the backlog for that
 * reason), and answering a `Float32` decode of the same two bytes would be a wrong answer no
 * program could see coming. The interpreter *does* answer both, so `undefined` would be a
 * divergence the gate cannot see; a refusal is one it can. */
static int kiln_c99_is_data_view_member(const char *key, size_t key_len) {
    return kiln_c99_same_name(key, key_len, "getFloat16") ||
           kiln_c99_same_name(key, key_len, "setFloat16");
}

/* `String.prototype`'s members, by name.
 *
 * Two of them are not operations of their own and so have no code in the generated dispatch
 * group, and a name the group did not carry read as `undefined` here where the interpreter has a
 * function -- the silent wrong answer, on the prototype object *and* on every string instance.
 * `values` is the string iterator, which is the same function `@@iterator` hands out, so
 * `[..."ab"]` and `"ab".values()` walk the same scalar values. `toLocaleString` invokes the
 * receiver's own `toString`, which for a string is the string: there is no locale in it to
 * consult, so nothing here is the Intl surface ADR 0042 refuses by name. */
static uint64_t kiln_c99_string_method(const char *key, size_t key_len) {
    /* `values` is deliberately absent: a string carries `@@iterator` and no `values` beside it,
     * unlike an array. Answering it here made `"abc".values` a function and put `values` in
     * `Object.getOwnPropertyNames(String.prototype)`, a name node has on neither. The iterator
     * itself is unaffected -- the `@@iterator` cases in the string read and in
     * `kiln_c99_namespace_method` reach `KILN_C99_METHOD_ARRAY_VALUES` directly. */
    if (kiln_c99_same_name(key, key_len, "toLocaleString")) {
        return KILN_C99_METHOD_OBJECT_TO_LOCALE_STRING;
    }
    if (kiln_c99_same_name(key, key_len, "trimRight")) {
        return KILN_C99_METHOD_TRIM_END;
    }
    if (kiln_c99_same_name(key, key_len, "trimLeft")) {
        return KILN_C99_METHOD_TRIM_START;
    }
    return kiln_c99_lookup_method_group(key, key_len, KILN_C99_GROUP_STRING_DISPATCH);
}

/* The static surface of every built-in this backend answers for: one row per (built-in, member).
 *
 * ONE TABLE, TWO EMITTERS (ADR 0043). Every row below is generated from `BUILTIN_SURFACE` in this
 * backend's Rust, which is also what the drift test reads. Before it the same surface was written
 * down twice -- once in the interpreter and once as `kiln_c99_same_name` chains here -- and
 * nothing compared them, so a member the interpreter grew was discovered by whichever npm bundle
 * happened to read it. `Number` had fifteen statics on one side and five here; `Number.MAX_VALUE`
 * is the first property a bundled `zod` reads.
 *
 * A row carries what the member *is*, not merely that it exists, so the read path, the call path
 * and `Object.getOwnPropertyNames` all answer out of it. That is what makes a member which is
 * implemented but unreachable inexpressible: reachability is the row. A `@@` name is symbol-keyed
 * and is reached through a Symbol rather than as a string key. */
#define KILN_C99_MEMBER_METHOD 1
#define KILN_C99_MEMBER_NUMBER 2
#define KILN_C99_MEMBER_TEXT 3
#define KILN_C99_MEMBER_SYMBOL 4
#define KILN_C99_MEMBER_BUILTIN 5
#define KILN_C99_MEMBER_UNSCOPABLES 6
#define KILN_C99_MEMBER_ACCESSOR 7

typedef struct {
    uint16_t builtin;
    uint16_t kind;
    /* A method code, or the built-in a `KILN_C99_MEMBER_BUILTIN` row hands back. */
    uint16_t code;
    double number;
    const char *name;
    /* The `@@` key of a well-known symbol, or the text a tag answers with. */
    const char *text;
} kiln_builtin_member;

static const kiln_builtin_member kiln_c99_builtin_members[] = {
"#,
    emit_builtin_surface_rows!(),
    r#"};

#define KILN_C99_SURFACE_MEMBER_COUNT \
    (sizeof kiln_c99_builtin_members / sizeof *kiln_c99_builtin_members)

/* One flag per row above: 1 where the row is one of kiln's own members rather than the
 * language's (ADR 0068), from the row's `kiln_extension` mark, which the Rust build holds equal to
 * `kiln_builtin_surface::KILN_EXTENSIONS`. `kiln_c99_builtin_row_withheld` in `surface.c` is the
 * one place it is read. */
static const unsigned char kiln_c99_builtin_member_is_extension[] = {
"#,
    emit_builtin_extension_flags!(),
    r#"
};

/* The members this backend knows the language has and it does not answer, by the global that owns
 * them. Generated from the same declaration as the Rust list, so the two cannot disagree.
 *
 * The read path needs it to tell apart two things that are the same absence inside the surface
 * table: a name the language never put on this built-in, which is `undefined`, and a member that
 * belongs there and is not built yet, which must refuse by name. Answering `undefined` for the
 * second would break `typeof Object.groupBy === "function"` -- the program takes the other branch
 * and nothing reports anything, which is the failure this backend exists to avoid. */
typedef struct {
    const char *owner;
    const char *member;
} kiln_builtin_gap;

static const kiln_builtin_gap kiln_c99_builtin_backlog[] = {
"#,
    emit_builtin_backlog_rows!(),
    include_str!(concat!(env!("OUT_DIR"), "/prelude_literal_keys.h")),
    core_parts!(),
);

pub(crate) const PRELUDE_HEADER: &str = concat!(
    header_prefix!(),
    header_extern_diagnostics!(),
    header_rest!(),
    include_str!("runtime/prelude_declarations.h"),
);

static DEFAULT_RUNTIME_PRELUDE_SOURCE: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// Return the program-independent C11 runtime prelude source for standalone compilation with a specific policy.
pub fn runtime_prelude_source_with_policy(policy: &EmissionPolicy) -> String {
    if *policy == EmissionPolicy::default() {
        return runtime_prelude_source().to_owned();
    }
    let mut output = String::new();
    render_emission_policy(&mut output, policy);
    let text = HEADER.replace(
        "/* KILN_NATIVE_CONSTRUCTOR_TABLE */",
        &render_native_constructor_table(),
    );
    output.push_str(&build_runtime_prelude_source(&text));
    output
}

/// Return the deterministic, program-independent C11 runtime prelude source for standalone compilation.
pub fn runtime_prelude_source() -> &'static str {
    DEFAULT_RUNTIME_PRELUDE_SOURCE.get_or_init(|| {
        let text = HEADER.replace(
            "/* KILN_NATIVE_CONSTRUCTOR_TABLE */",
            &render_native_constructor_table(),
        );
        build_runtime_prelude_source(&text)
    })
}

/// Return the C11 declarations header for modules compiled against the standalone runtime prelude.
pub fn runtime_prelude_header() -> &'static str {
    // A module unit's copy of the header declares the program-wide mutable state the header
    // defines for the prelude unit; a definition in every unit would give each its own copy.
    static FOR_UNITS: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    FOR_UNITS.get_or_init(|| {
        let mut text = PRELUDE_HEADER.replace(
            "static uint64_t kiln_c99_builtin_store_epoch;",
            "extern uint64_t kiln_c99_builtin_store_epoch __attribute__((visibility(\"hidden\")));",
        );
        assert!(
            text != PRELUDE_HEADER,
            "the built-in store summary's definition moved"
        );
        // One streak table per thread, not one per unit (realms issue 08): a `static` in a
        // header every chunk includes gave each unit its own kilobyte of thread-local storage,
        // and every thread's TLS block is zeroed whole when the thread starts -- ten copies were
        // ten kilobytes resident in every realm thread, pool thread and runtime thread.
        let streaks = "static _Thread_local kiln_site_streak kiln_c99_streaks[64];";
        assert!(text.contains(streaks), "the site streak table's definition moved");
        text = text.replace(
            streaks,
            "extern _Thread_local kiln_site_streak kiln_c99_streaks[64] __attribute__((visibility(\"hidden\")));",
        );
        let json_document =
            "static int kiln_c99_json_document(const kiln_string *source, kiln_val *out);";
        assert!(
            text.contains(json_document),
            "the JSON document bridge declaration moved"
        );
        text = text.replace(
            json_document,
            "extern int kiln_c99_json_document(const kiln_string *source, kiln_val *out);",
        );
        const ASCII_STRINGS_BEGIN: &str = "/* KILN_RUNTIME_ONLY_ASCII_STRINGS_BEGIN */";
        const ASCII_STRINGS_END: &str = "/* KILN_RUNTIME_ONLY_ASCII_STRINGS_END */";
        let begin = text
            .find(ASCII_STRINGS_BEGIN)
            .expect("runtime-only ASCII strings begin marker moved");
        let end = text
            .find(ASCII_STRINGS_END)
            .expect("runtime-only ASCII strings end marker moved")
            + ASCII_STRINGS_END.len();
        text.replace_range(
            begin..end,
            concat!(
                "extern const char kiln_c99_empty_chars[] __attribute__((visibility(\"hidden\")));\n",
                "extern const kiln_string kiln_c99_empty_string __attribute__((visibility(\"hidden\")));\n",
                "extern const char kiln_c99_single_char_bytes[256][2] __attribute__((visibility(\"hidden\")));\n",
                "extern const kiln_string kiln_c99_ascii_strings[256] __attribute__((visibility(\"hidden\")));",
            ),
        );
        const RUNTIME_ONLY_BEGIN: &str = "/* KILN_RUNTIME_ONLY_METHOD_GROUPS_BEGIN */";
        const RUNTIME_ONLY_END: &str = "/* KILN_RUNTIME_ONLY_METHOD_GROUPS_END */";
        let begin = text
            .find(RUNTIME_ONLY_BEGIN)
            .expect("runtime-only method groups begin marker moved");
        let end = text
            .find(RUNTIME_ONLY_END)
            .expect("runtime-only method groups end marker moved")
            + RUNTIME_ONLY_END.len();
        text.replace_range(begin..end, "");
        text
    })
}

fn build_runtime_prelude_source(src: &str) -> String {
    let src = src.replace(
        "/* KILN_NATIVE_METHOD_LENGTH_ROWS */",
        &render_native_method_length_rows(),
    );
    let src = src.as_str();
    let mut out = String::with_capacity(src.len() + 1024);
    const VARS_TO_EXPORT: &[&str] = &[
        "kiln_c99_diagnostic_active_depth",
        "kiln_c99_diagnostic;",
        "kiln_c99_new_target",
        "kiln_c99_inheriting_new_target",
        "kiln_c99_typed_status",
        "kiln_c99_typed_thrown",
        "kiln_c99_typed_overflow",
        "kiln_c99_adopted_this",
        "kiln_c99_frame_args",
        "kiln_c99_frame_argc",
        "kiln_c99_captured_arguments",
        "kiln_c99_frame_arguments_object",
        "kiln_c99_frame_is_arrow",
        "kiln_c99_frame_mapped_arguments",
        "kiln_c99_frame_callee",
        "kiln_c99_empty_chars",
        "kiln_c99_empty_string",
        "kiln_c99_single_char_bytes",
        "kiln_c99_ascii_strings",
        "kiln_c99_globals",
        "kiln_c99_process",
        "kiln_c99_atomics",
        "kiln_c99_builtin_store_epoch",
        "kiln_c99_streaks[64];",
    ];

    for line in src.lines() {
        if let Some(rem) = line.strip_prefix("static ") {
            if line.contains('(') {
                if let Some(tail) = rem.strip_prefix("inline int kiln_c99_invoke_closure(") {
                    out.push_str("int kiln_c99_invoke_closure(");
                    out.push_str(tail);
                } else if rem.starts_with("inline ") {
                    out.push_str(line);
                } else {
                    out.push_str(rem);
                }
            } else {
                let mut matched = false;
                for var in VARS_TO_EXPORT {
                    if line.contains(var) {
                        out.push_str(rem);
                        matched = true;
                        break;
                    }
                }
                if !matched {
                    out.push_str(line);
                }
            }
        } else {
            out.push_str(line);
        }
        out.push('\n');
    }
    out
}

/// Mutable static state embedded in the generated-C prelude, classified by its required owner.
///
/// This is the migration manifest for ADR 0036, and since realms issue 04 step 5 (ADR 0070) it has
/// no `Realm` owner: realm state is not a static at all but a field of `kiln_c99_realm_state`
/// (`runtime/foundation.c`), reached through the realm's heap. Adding a mutable `kiln_c99_*`
/// static without naming one of the owners below fails the architecture test, and the owners that
/// are per thread fail it unless the declaration says `_Thread_local`.
#[cfg(test)]
pub(crate) const C99_MUTABLE_STATE_OWNERS: &[(&str, C99StateOwner)] = &[
    ("kiln_c99_diagnostic", C99StateOwner::Frame),
    ("kiln_c99_diagnostic_active_depth", C99StateOwner::Frame),
    ("kiln_c99_active_http_response", C99StateOwner::Frame),
    ("kiln_c99_new_target", C99StateOwner::Frame),
    ("kiln_c99_inheriting_new_target", C99StateOwner::Frame),
    ("kiln_c99_typed_status", C99StateOwner::Frame),
    ("kiln_c99_typed_thrown", C99StateOwner::Frame),
    ("kiln_c99_adopted_this", C99StateOwner::Frame),
    ("kiln_c99_frame_args", C99StateOwner::Frame),
    ("kiln_c99_frame_argc", C99StateOwner::Frame),
    ("kiln_c99_captured_arguments", C99StateOwner::Frame),
    ("kiln_c99_frame_arguments_object", C99StateOwner::Frame),
    ("kiln_c99_frame_is_arrow", C99StateOwner::Frame),
    ("kiln_c99_frame_mapped_arguments", C99StateOwner::Frame),
    ("kiln_c99_frame_callee", C99StateOwner::Frame),
    ("kiln_c99_ffi_callback_status", C99StateOwner::Frame),
    ("kiln_c99_ffi_callback_host", C99StateOwner::Frame),
    ("kiln_c99_ffi_callback_thrown", C99StateOwner::Frame),
    ("kiln_c99_ffi_callback_depth", C99StateOwner::Frame),
    ("kiln_c99_ffi_callback_active", C99StateOwner::Frame),
    // Set only around one synchronous kiln:shared module initializer; no turn boundary sees it.
    ("kiln_c99_kiln_shared_hooks_active", C99StateOwner::Frame),
    ("kiln_c99_fixture_method_stores", C99StateOwner::TestFixture),
    ("kiln_c99_gc_fallback", C99StateOwner::TestFixture),
    ("kiln_c99_keys_check_mode", C99StateOwner::TestFixture),
    ("kiln_c99_method_index", C99StateOwner::Constant),
    ("kiln_c99_method_index_len", C99StateOwner::Constant),
    ("kiln_c99_method_index_ready", C99StateOwner::Constant),
    // The surface table's index: the same rows sorted once, a constant once built.
    ("kiln_c99_builtin_member_index", C99StateOwner::Constant),
    ("kiln_c99_builtin_member_index_len", C99StateOwner::Constant),
    (
        "kiln_c99_builtin_member_index_ready",
        C99StateOwner::Constant,
    ),
    // Per thread, and trusted only under the realm generation it was filled under.
    ("kiln_c99_realm_cached", C99StateOwner::ThreadCache),
    // ADR 0071: only whether *some* realm has modified *some* built-in; which ones a realm has
    // modified is on its heap, and no cache entry holds a value of this.
    ("kiln_c99_builtin_store_epoch", C99StateOwner::Process),
    // Recycled byte blocks: plain memory that names no realm's value.
    ("kiln_c99_bytes_cache", C99StateOwner::ThreadCache),
    // Parallel issue 30: the lowest address this thread's calls may start at, found once.
    ("kiln_c99_stack_floor", C99StateOwner::ThreadCache),
    // Per thread, and trusted only for the heap serial it was built under: the `node:http` layouts.
    ("kiln_c99_node_http_layouts", C99StateOwner::ThreadCache),
    ("kiln_c99_gc_stress_checks", C99StateOwner::Process),
    // Monotonic ID for candidate-only chunks; the arena uses an atomic increment, and no
    // realm object or graph pointer is stored in this process-wide counter.
    ("kiln_c99_graph_candidate_next_id", C99StateOwner::Process),
    // Parallel issue 06's write detector (`KILN_PARALLEL_PROTECT`), read from the environment once.
    ("kiln_c99_par_protect_state", C99StateOwner::Process),
    // Parallel issue 08's worker arena budget (`KILN_PARALLEL_ARENA_KIB`), read once.
    ("kiln_c99_par_arena_budget_bytes", C99StateOwner::Process),
    // `KILN_PARALLEL_TRACE`: whether each arena join says what it adopted, read once.
    ("kiln_c99_par_trace_state", C99StateOwner::Process),
    ("kiln_c99_introspect_pending", C99StateOwner::Process),
    ("kiln_c99_introspect_armed", C99StateOwner::Process),
    // The process-shared block (ADR 0071): shared keys and the shape root, one per process.
    // Each unit caches the pointer to it; the block itself is the provider archive's.
    ("kiln_c99_shared_block", C99StateOwner::Process),
    ("kiln_c99_speculate_mode", C99StateOwner::Process),
    // ADR 0068: whether this process installs kiln's own members, read from the environment once.
    ("kiln_c99_kiln_extensions_state", C99StateOwner::Process),
    ("kiln_c99_node_stdin_paused", C99StateOwner::Process),
    ("kiln_c99_node_stdin_read_active", C99StateOwner::Process),
    ("kiln_c99_node_stdin_request", C99StateOwner::Process),
    // Function-scope statics are state too (realms issue 05: nothing process-wide unclassified).
    // Derived once from constants; a racing second computation writes the same word.
    ("kiln_c99_method_code_end_cached", C99StateOwner::Constant),
    // `1n` and 2^64-1 as provider handles, made once; the first published is kept.
    ("kiln_c99_bigint_one_handle", C99StateOwner::Constant),
    ("kiln_c99_bigint_u64_mask_handle", C99StateOwner::Constant),
    // Headers for built-in text, claimed and published once each (`kiln_c99_static_text`).
    ("kiln_c99_static_texts", C99StateOwner::Constant),
    // `Symbol()` serials: process-wide so no realm mints one twice.
    ("kiln_c99_symbol_serial", C99StateOwner::Process),
    // Each well-known symbol's shared canonical key, published once (`kiln_c99_symbol_ikey`).
    ("kiln_c99_well_known_ikeys", C99StateOwner::Constant),
    // The `KILN_SPECULATE=exit` test mode's draw, per thread.
    ("kiln_c99_speculate_rng", C99StateOwner::ThreadCache),
    // Which later entry of which site answered this thread last (`kiln_c99_site_answered`).
    ("kiln_c99_streaks", C99StateOwner::ThreadCache),
    // The no-write read path (realms issue 08, parallel-builtins issue 06): whether this thread
    // is reading without writing, around a section that leaves it as it found it.
    ("kiln_c99_no_write_depth", C99StateOwner::Frame),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg(test)]
pub(crate) enum C99StateOwner {
    /// A register of the running call, saved and restored around it, so idle at every turn
    /// boundary: per thread (`_Thread_local`), because the machine stack it spills is the
    /// thread's. See `runtime/builtins.c`.
    Frame,
    /// A per-thread cache that is either validated against the realm it was filled in (the realm
    /// generation, a heap serial) or holds nothing any realm made. `_Thread_local`.
    ThreadCache,
    TestFixture,
    /// Derived once from compile-time constants and never changed again, so every realm in the
    /// process would compute the same bytes. Realms on several threads may ask for it first at
    /// once: an index sorted in place is built under `kiln_c99_once`, so nobody searches it
    /// half-sorted; a single word or an entry is published once, by one store or compare-and-swap,
    /// and a racing second computation of it writes the same bytes or loses. A realm neither owns
    /// it nor has to reset it.
    Constant,
    /// Belongs to the process and its signal disposition rather than to any realm, and outlives
    /// every realm the process makes. A realm must not reset it: the `SIGUSR1` handler installed
    /// for the first realm is still installed for the second, and the flag it sets names a
    /// request that has arrived, not a value some realm made.
    Process,
}

#[cfg(test)]
impl C99StateOwner {
    /// Whether the owner requires the declaration to be `_Thread_local`.
    pub(crate) fn per_thread(self) -> bool {
        matches!(self, Self::Frame | Self::ThreadCache)
    }
}

/// The stack each emitted C function needs: the module body, then one entry per compiled
/// function body in [`Program::functions`], in the same order.
pub(crate) struct StackPlan {
    pub(crate) body: usize,
    pub(crate) functions: Vec<usize>,
}
