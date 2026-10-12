//! Internal one-word value layout (ADR 0053). The public `kiln_value` ABI is separate.
//! This registry generates both Rust constants and the C declarations used by every prelude.

#[macro_export]
macro_rules! value_layout_registry {
    ($consumer:ident) => {
        $consumer! {
            KILN_QNAN_BASE = 0x7FF8000000000000;
            KILN_TAG_MASK = 0xFFFF000000000000;
            KILN_PAYLOAD_MASK = 0x0000FFFFFFFFFFF8;
            KILN_VAL_UNDEFINED = 0xFFF8000000000001;
            KILN_VAL_NULL = 0xFFF8000000000002;
            KILN_VAL_FALSE = 0xFFF8000000000004;
            KILN_VAL_TRUE = 0xFFF8000000000005;
            KILN_VAL_HOLE = 0xFFF8000000000006;
            KILN_VAL_UNINITIALIZED = 0xFFF8000000000007;
            KILN_VAL_RETURN_SIGNAL = 0xFFF8000000000009;
            KILN_VAL_BUILTIN_PREFIX = 0xFFF8000100000000;
            KILN_VAL_METHOD_PREFIX = 0xFFF8000200000000;
            KILN_TAG_STRING_PREFIX = 0xFFF9000000000000;
            KILN_TAG_OBJECT_PREFIX = 0xFFFA000000000000;
            KILN_TAG_CLOSURE_PREFIX = 0xFFFB000000000000;
            KILN_TAG_BUFFER_PREFIX = 0xFFFC000000000000;
            KILN_TAG_SYMBOL_PREFIX = 0xFFFD000000000000;
            KILN_TAG_CELL_PREFIX = 0xFFFF000000000000;
            KILN_TAG_RARE_PREFIX = 0xFFFE000000000000;
            KILN_VAL_TYPED_FAIL = 0xFFF8000000000008;
            KILN_IMMEDIATE_PREFIX = 0xFFF8000000000000;
            KILN_REF_TAG_MASK = 0xFFFFFFFF00000000;
            KILN_REF_ID_MASK = 0x00000000FFFFFFFF;
            KILN_BOOLEAN_MASK = 0xFFFFFFFFFFFFFFFE;
            KILN_RARE_TAG_MASK = 0xFFFF000000000007;
            KILN_TAG_BIGINT_PREFIX = 0xFFFE000000000000;
            KILN_TAG_TEMPORAL_PREFIX = 0xFFFE000000000001;
            KILN_TAG_SHARED_PREFIX = 0xFFFE000000000002;
        }
    };
}

macro_rules! rust_constants {
    ($($name:ident = $value:literal;)*) => {
        $(pub const $name: u64 = $value;)*
    };
}
value_layout_registry!(rust_constants);

#[macro_export]
macro_rules! value_layout_c_declaration {
    ($($name:ident = $value:literal;)*) => {
        concat!("/* Generated from kiln-value-abi: do not copy layout literals. */\n",
            $("#define ", stringify!($name), " UINT64_C(", stringify!($value), ")\n",)*)
    };
}

/// C declarations generated from exactly the same rows as the Rust constants.
pub const C_DECLARATION: &str = value_layout_registry!(value_layout_c_declaration);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_pointer_kinds_keep_48_bits_and_rare_kinds_remain_distinct() {
        let prefixes = [
            KILN_TAG_STRING_PREFIX,
            KILN_TAG_OBJECT_PREFIX,
            KILN_TAG_CLOSURE_PREFIX,
            KILN_TAG_BUFFER_PREFIX,
            KILN_TAG_SYMBOL_PREFIX,
            KILN_TAG_CELL_PREFIX,
            KILN_TAG_BIGINT_PREFIX,
            KILN_TAG_TEMPORAL_PREFIX,
            KILN_TAG_SHARED_PREFIX,
        ];
        for address in [0, 8, 1 << 47, KILN_PAYLOAD_MASK] {
            for prefix in prefixes {
                let boxed = prefix | address;
                assert_eq!(boxed & KILN_PAYLOAD_MASK, address);
                assert_eq!(boxed & KILN_RARE_TAG_MASK, prefix);
                assert!(boxed >= KILN_IMMEDIATE_PREFIX);
            }
        }
        assert_ne!(KILN_TAG_BIGINT_PREFIX, KILN_TAG_TEMPORAL_PREFIX);
        assert_ne!(KILN_TAG_SHARED_PREFIX, KILN_TAG_TEMPORAL_PREFIX);
        assert_eq!(KILN_QNAN_BASE & KILN_TAG_MASK, KILN_QNAN_BASE);
        assert!(KILN_QNAN_BASE < KILN_IMMEDIATE_PREFIX);
    }

    /// Catch raw prefixes in runtime C, Rust ABI adapters and C emitter strings.
    /// Test snapshots contain generated declarations and are intentionally not source inputs.
    #[test]
    fn layout_literals_have_one_owner() {
        fn visit(path: &std::path::Path, violations: &mut Vec<String>) {
            if path.is_dir() {
                for entry in std::fs::read_dir(path).unwrap() {
                    visit(&entry.unwrap().path(), violations);
                }
                return;
            }
            if path.canonicalize().unwrap()
                == std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("src/lib.rs")
                    .canonicalize()
                    .unwrap()
                || !path
                    .extension()
                    .is_some_and(|ext| ext == "rs" || ext == "c" || ext == "h")
            {
                return;
            }
            let source = std::fs::read_to_string(path).unwrap();
            let bytes = source.as_bytes();
            let mut at = 0;
            while at + 2 < bytes.len() {
                if bytes[at] != b'0' || !matches!(bytes[at + 1], b'x' | b'X') {
                    at += 1;
                    continue;
                }
                let start = at;
                at += 2;
                let digits = at;
                while at < bytes.len() && (bytes[at].is_ascii_hexdigit() || bytes[at] == b'_') {
                    at += 1;
                }
                let literal = source[digits..at].replace('_', "");
                if literal.len() != 16 {
                    continue;
                }
                let value = u64::from_str_radix(&literal, 16).unwrap();
                let is_prefix = value >= KILN_IMMEDIATE_PREFIX
                    && value & KILN_REF_ID_MASK < 16
                    && value & 0x00007fff00000000 <= 0x0000000200000000;
                let is_pointer_mask = value == (KILN_PAYLOAD_MASK | 7)
                    || value == KILN_PAYLOAD_MASK
                    || value == 0x00007fffffffffff
                    || value == 0xffff800000000000;
                if is_prefix || is_pointer_mask {
                    let line = source[..start].bytes().filter(|b| *b == b'\n').count() + 1;
                    violations.push(format!("{}:{line}: {}", path.display(), &source[start..at]));
                }
            }
        }
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let mut violations = Vec::new();
        visit(&root.join("crates"), &mut violations);
        assert!(
            violations.is_empty(),
            "use kiln-value-abi constants or KILN_VAL predicates:\n{}",
            violations.join("\n")
        );
    }
}
