use super::{
    alloc::{Arena, ArenaVecExt, transfer_arena, vec_from_iter_in},
    core::{identifier, lexer, strings},
    json,
    paths::{platform::Posix, resolve_path::relative_normalized},
};
use crate::engine::json::Json;

crate::support::core::comptime_string_map! {
    static TEST_BYTE_MAP: u8 = { b"key" => 7 };
}
crate::support::core::comptime_string_set! {
    static TEST_TEXT_SET = { "key" };
}

#[test]
fn wtf8_keeps_lone_surrogate_and_utf16_counts_as_two_units() {
    assert_eq!(strings::decode_wtf8(&[0xed, 0xa0, 0x80]), Some((0xd800, 3)));
    assert_eq!(
        strings::decode_wtf8(&[0xf0, 0x9f, 0xa7, 0xa0]),
        Some((0x1f9e0, 4))
    );
    assert_eq!(kiln_string::StringUnits::from("🧠").len(), 2);
}

#[test]
fn astral_identifier_uses_ecmascript_unicode_tables() {
    assert!(identifier::is_identifier_start(0x10400));
    assert!(lexer::is_identifier("𐐀name".as_bytes()));
}

#[test]
fn package_json_preserves_duplicate_keys_and_condition_order() {
    let json =
        json::parse(br#"{"exports":{"import":"first","default":"fallback","import":"second"}}"#)
            .unwrap();
    let Json::Object(root) = json else {
        panic!("object root expected")
    };
    let Json::Object(exports) = &root[0].1 else {
        panic!("exports object expected")
    };
    assert_eq!(
        exports
            .iter()
            .map(|(key, _)| key.as_slice())
            .collect::<Vec<_>>(),
        [b"import".as_slice(), b"default", b"import"]
    );
}

#[test]
fn jsonc_config_reader_keeps_ordered_duplicate_properties_and_source_spans() {
    use crate::engine::{
        atom::Interner,
        hir::{ExprKind, StmtKind},
        session::Session,
    };

    let session = Session::new();
    let atoms = Interner::new_in(&session);
    let source = br#"{
      // config comment
      "compilerOptions": { "strict": true, "strict": false, },
      "negative": -2,
    }"#;
    let hir = json::parse_source_file(session.arena(), &session, &atoms, source);
    assert!(!hir.has_errors);
    let stmt = hir.ids(hir.body).next().unwrap();
    let root = match hir[stmt].kind {
        StmtKind::ExportAssign(expr) => expr,
        _ => panic!("JSON root expression expected"),
    };
    let ExprKind::Object(root_props) = hir[root].kind else {
        panic!("JSON root object expected")
    };
    let props: Vec<_> = root_props.iter().map(|id| hir[id]).collect();
    assert_eq!(props.len(), 2);
    assert_eq!(
        atoms.bytes(props[0].key.name().unwrap()),
        b"compilerOptions"
    );
    let ExprKind::Object(options) = hir[props[0].value].kind else {
        panic!("compilerOptions object expected")
    };
    let option_props: Vec<_> = options.iter().map(|id| hir[id]).collect();
    assert_eq!(option_props.len(), 2);
    assert!(option_props[0].pos < option_props[0].end);
    assert!(option_props[0].end <= option_props[1].pos);
    assert_eq!(atoms.bytes(option_props[0].key.name().unwrap()), b"strict");
    assert_eq!(atoms.bytes(option_props[1].key.name().unwrap()), b"strict");
    assert!(matches!(
        hir[props[1].value].kind,
        ExprKind::Unary {
            op: crate::engine::hir::UnOp::Minus,
            ..
        }
    ));
}

#[test]
fn jsonc_config_reader_accepts_bom_and_marks_malformed_input() {
    use crate::engine::{atom::Interner, session::Session};

    let session = Session::new();
    let atoms = Interner::new_in(&session);
    let valid =
        json::parse_source_file(session.arena(), &session, &atoms, b"\xef\xbb\xbf{\"a\":1,}");
    assert!(!valid.has_errors);

    let malformed = json::parse_source_file(session.arena(), &session, &atoms, b"{\"a\":1");
    assert!(malformed.has_errors);
    assert!(malformed.has_parse_diagnostics);
    assert_eq!(malformed.syntax_errors, 1);
    let diagnostic = &malformed.diagnostics[0];
    assert_eq!(diagnostic.code, 1005);
    assert_eq!(diagnostic.args[0].as_ref(), b"}");
    assert_eq!(diagnostic.start as usize, 6);
}

#[test]
fn relative_path_keeps_rooted_path_components() {
    assert_eq!(
        relative_normalized::<Posix, true>(b"/root/project", b"/root/target").as_ref(),
        b"../target"
    );
    assert_eq!(
        relative_normalized::<Posix, true>(b"/root", b"/root/child").as_ref(),
        b"child"
    );
}

#[test]
fn nested_path_buffer_guards_get_distinct_initialized_storage() {
    let mut outer = super::paths::path_buffer_pool::get();
    outer[0] = b'x';
    let inner = super::paths::path_buffer_pool::get();
    assert!(outer.len() >= 32 * 1024);
    assert!(inner.len() >= 32 * 1024);
    assert_ne!(outer.as_ptr(), inner.as_ptr());
    assert_eq!(outer[0], b'x');
}

#[test]
fn arena_vector_transfer_preserves_values_and_uses_destination_allocator() {
    let source = Arena::new();
    let target = Arena::new();
    let mut values = vec_from_iter_in([1u32, 2, 3], &source);
    transfer_arena(&mut values, &target);
    assert_eq!(&*values, &[1, 2, 3]);
    assert!(std::ptr::eq(ArenaVecExt::allocator(&values), &target));
}

#[test]
fn static_key_macros_accept_byte_map_and_text_set_literals() {
    assert_eq!(TEST_BYTE_MAP.get(b"key"), Some(&7));
    assert!(TEST_TEXT_SET.contains(b"key"));
}
