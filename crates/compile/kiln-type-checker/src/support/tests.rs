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
