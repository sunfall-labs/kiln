use super::*;
use kiln_ir::Builtin;
use std::io::Write as _;
use std::process::{Command, Stdio};

mod c99_state_owners;
use c99_state_owners::{C99_MUTABLE_STATE_OWNERS, C99StateOwner};

mod fixture_cache;

fn program(constants: Vec<Constant>, code: Vec<Instruction>, local_count: u32) -> Program {
    Program {
        source: "".into(),
        version: BYTECODE_VERSION,
        source_origin: None,
        static_source_identity: None,
        constants,
        code,
        local_count,
        functions: Vec::new(),
        template_sites: Vec::new(),
        debug_call_sites: Vec::new(),
    }
}

fn module(
    program: Program,
    imports: Vec<kiln_ir::ImportBinding>,
    exports: Vec<kiln_ir::ExportBinding>,
) -> ModuleProgram {
    ModuleProgram {
        program,
        dependencies: Vec::new(),
        imports,
        exports,
        strictness: kiln_ir::Strictness::default(),
        ..Default::default()
    }
}

/// The engine half of the provider ABI, for tests that link the vendored C number runtime.
///
/// The provider contract's C is a *test fixture*: production links the Rust provider archive,
/// which defines these. Tests here exercise the emitted C's own logic and none of them
/// compiles a pattern or does arbitrary-precision arithmetic -- so a fixture that refuses
/// every one is honest, and a test that did need one would get a visible error rather than a
/// quiet wrong answer. Such a test belongs in `conformance/native`, where the real archive is
/// linked.
const REGEXP_FIXTURE: &str = r#"
int32_t kiln_regexp_compile(const kiln_string *pattern, const kiln_string *flags,
                            void **out_handle) {
    (void)pattern; (void)flags; (void)out_handle;
    return -1;
}
void kiln_regexp_free(void *handle) { (void)handle; }
int32_t kiln_regexp_search(void *handle, const kiln_string *subject, size_t start,
                           int64_t *out_ranges, size_t out_capacity) {
    (void)handle; (void)subject; (void)start; (void)out_ranges;
    (void)out_capacity;
    return -1;
}
size_t kiln_regexp_group_count(const void *handle) { (void)handle; return 0; }
int32_t kiln_regexp_group_name(const void *handle, size_t index, unsigned char *buffer,
                               size_t capacity) {
    (void)handle; (void)index; (void)buffer; (void)capacity;
    return -1;
}
size_t kiln_regexp_named_group_count(const void *handle) { (void)handle; return 0; }
int32_t kiln_regexp_named_group_at(const void *handle, size_t index, unsigned char *buffer,
                                   size_t capacity, int64_t *from, int64_t *to, size_t *capture) {
    (void)handle; (void)index; (void)buffer; (void)capacity; (void)from; (void)to; (void)capture;
    return -1;
}
int32_t kiln_bigint_parse(const unsigned char *text, size_t len, uint32_t radix, void **out) {
    (void)text; (void)len; (void)radix; (void)out;
    return -1;
}
int32_t kiln_bigint_from_double(double value, void **out) { (void)value; (void)out; return -1; }
void kiln_bigint_free(void *handle) { (void)handle; }
double kiln_bigint_to_double(const void *handle) { (void)handle; return 0.0; }
int32_t kiln_bigint_to_string(const void *handle, uint32_t radix, unsigned char *buffer,
                              size_t capacity) {
    (void)handle; (void)radix; (void)buffer; (void)capacity;
    return -1;
}
int32_t kiln_bigint_binary(uint32_t operation, const void *left, const void *right, void **out) {
    (void)operation; (void)left; (void)right; (void)out;
    return -1;
}
int32_t kiln_bigint_negate(const void *handle, void **out) { (void)handle; (void)out; return -1; }
int32_t kiln_bigint_compare(const void *left, const void *right) {
    (void)left; (void)right;
    return 0;
}
int32_t kiln_bigint_is_zero(const void *handle) { (void)handle; return 1; }
int32_t kiln_bigint_to_i128(const void *handle, int64_t *out_high, uint64_t *out_low) {
    (void)handle; (void)out_high; (void)out_low;
    return -1;
}
int32_t kiln_temporal_from_fields(uint32_t kind, const double *fields, size_t field_count,
                                  const unsigned char *text, const int64_t *text_lengths,
                                  size_t text_count, const unsigned char *option_text,
                                  const int64_t *option_text_lengths, size_t option_text_count,
                                  void **out_handle) {
    (void)kind; (void)fields; (void)field_count; (void)text; (void)text_lengths;
    (void)text_count; (void)option_text; (void)option_text_lengths; (void)option_text_count;
    (void)out_handle;
    return -1;
}
int32_t kiln_temporal_duration_from_fields(const double *fields, size_t field_count,
                                           void **out_handle) {
    (void)fields; (void)field_count; (void)out_handle;
    return -1;
}
int32_t kiln_temporal_construct(uint32_t kind, const double *fields, size_t field_count,
                                const unsigned char *text, const int64_t *text_lengths,
                                size_t text_count, int64_t epoch_high, uint64_t epoch_low,
                                void **out_handle) {
    (void)kind; (void)fields; (void)field_count; (void)text; (void)text_lengths;
    (void)text_count; (void)epoch_high; (void)epoch_low; (void)out_handle;
    return -1;
}
int32_t kiln_temporal_parse(uint32_t kind, const unsigned char *bytes, size_t len,
                            const unsigned char *option_text,
                            const int64_t *option_text_lengths, size_t option_text_count,
                            void **out_handle) {
    (void)kind; (void)bytes; (void)len; (void)option_text; (void)option_text_lengths;
    (void)option_text_count; (void)out_handle;
    return -1;
}
int32_t kiln_temporal_now(uint32_t kind, int64_t high, uint64_t low, const unsigned char *zone,
                          size_t zone_len, void **out_handle) {
    (void)kind; (void)high; (void)low; (void)zone; (void)zone_len; (void)out_handle;
    return -1;
}
int32_t kiln_temporal_instant_from_epoch_milliseconds(double milliseconds, void **out_handle) {
    (void)milliseconds; (void)out_handle;
    return -1;
}
int32_t kiln_temporal_instant_from_epoch_nanoseconds(int64_t high, uint64_t low,
                                                     void **out_handle) {
    (void)high; (void)low; (void)out_handle;
    return -1;
}
void kiln_temporal_free(void *handle) { (void)handle; }
int32_t kiln_temporal_kind(const void *handle) { (void)handle; return -1; }
int32_t kiln_temporal_get(const void *handle, const unsigned char *name, size_t name_len,
                          double *out_number, unsigned char *buffer, size_t capacity,
                          int32_t *out_length) {
    (void)handle; (void)name; (void)name_len; (void)out_number; (void)buffer; (void)capacity;
    (void)out_length;
    return -1;
}
int32_t kiln_temporal_to_string(const void *handle, unsigned char *buffer, size_t capacity) {
    (void)handle; (void)buffer; (void)capacity;
    return -1;
}
int32_t kiln_temporal_compare(const void *left, const void *right, const void *relative_to,
                              const unsigned char *option_text,
                              const int64_t *option_text_lengths, size_t option_text_count,
                              int32_t *out_order) {
    (void)left; (void)right; (void)relative_to; (void)option_text; (void)option_text_lengths;
    (void)option_text_count; (void)out_order;
    return -1;
}
int32_t kiln_temporal_to_kind(const void *handle, uint32_t kind, void **out_handle) {
    (void)handle; (void)kind; (void)out_handle;
    return -1;
}
int32_t kiln_temporal_last_error(int32_t *out_kind, unsigned char *buffer, size_t capacity) {
    (void)out_kind; (void)buffer; (void)capacity;
    return -1;
}
/* The typed-array default-order sort is the provider's (`kiln_number_core::typed_order`); a
 * fixture that sorted would see `__kilnTypedArraySort` decline, and the TypeScript sort instead. */
int32_t kiln_typed_array_sort(uint8_t *data, size_t count, int32_t kind) {
    (void)data; (void)count; (void)kind;
    return -1;
}
/* Cross-realm ports and workers are the provider's (`kiln_runtime::workers`); a fixture has none,
 * so every port is empty and no worker starts. */
int kiln_worker_post(uint32_t op, uint64_t port, uint32_t kind, kiln_worker_token *tokens, uint64_t count) {
    (void)op; (void)port; (void)kind; (void)tokens; (void)count;
    return 0;
}
int kiln_worker_post_plain(uint64_t port, uint64_t root, void *context,
                           kiln_worker_plain_describe describe, kiln_worker_plain_field field) {
    (void)port; (void)root; (void)context; (void)describe; (void)field;
    return 0;
}
int kiln_worker_take(uint64_t port, uint32_t *out_kind, kiln_worker_token **out_tokens, uint64_t *out_count, uint32_t *out_plain, void **out_message) {
    (void)port; (void)out_kind; (void)out_tokens; (void)out_count; (void)out_plain; (void)out_message;
    return 0;
}
void kiln_worker_message_free(void *message) { (void)message; }
int kiln_worker_op(uint32_t op, uint64_t a, uint64_t *out) {
    (void)op; (void)a;
    if (out != NULL) { out[0] = 0; out[1] = 0; }
    return 0;
}
int kiln_worker_broadcast_join(const kiln_string *name, uint64_t *out) {
    (void)name;
    if (out != NULL) *out = 0;
    return 0;
}
int kiln_worker_spawn(const kiln_string *entry, int32_t eval, const kiln_string *argv, uint64_t argc, uint64_t port, uint64_t control, const kiln_string *info, uint64_t *out_key, uint64_t *out_thread) {
    (void)entry; (void)eval; (void)argv; (void)argc; (void)port; (void)control; (void)info; (void)out_key; (void)out_thread;
    return -2;
}
int kiln_worker_self(uint64_t *out_thread, uint64_t *out_port, uint64_t *out_control, const uint8_t **out_info, uint64_t *out_info_len) {
    (void)out_thread; (void)out_port; (void)out_control; (void)out_info; (void)out_info_len;
    return 0;
}
int kiln_worker_stdio(uint32_t stream, const uint8_t *bytes, uint64_t len) {
    (void)stream; (void)bytes; (void)len;
    return 0;
}
int kiln_worker_exit(int32_t code) { (void)code; return 1; }
int kiln_shared_create(uint64_t length, uint64_t capacity, uint8_t **out_bytes, void **out_handle) {
    (void)length; (void)capacity; (void)out_bytes; (void)out_handle;
    return -1;
}
void kiln_shared_release(void *handle) { (void)handle; }
int kiln_shared_grow(void *handle, uint64_t length, uint64_t *out) {
    (void)handle; (void)length; (void)out;
    return -1;
}
int kiln_atomics_wait(void *handle, uint64_t offset, uint32_t width, int64_t expected, double timeout) {
    (void)handle; (void)offset; (void)width; (void)expected; (void)timeout;
    return 2;
}
int kiln_atomics_wait_async_register(void *handle, uint64_t offset, uint32_t width, int64_t expected, uint64_t *out_ticket) {
    (void)handle; (void)offset; (void)width; (void)expected; (void)out_ticket;
    return 1;
}
void kiln_atomics_wait_async_cancel(uint64_t ticket) { (void)ticket; }
uint64_t kiln_atomics_notify(void *handle, uint64_t offset, uint64_t count) {
    (void)handle; (void)offset; (void)count;
    return 0;
}
int32_t kiln_intl_segment(const unsigned char *bytes, size_t len, uint32_t granularity,
                          const uint32_t **out_records, size_t *out_count) {
    (void)bytes; (void)len; (void)granularity; (void)out_records; (void)out_count;
    return -1;
}
/* `JSON.parse`'s reader lives in the Rust provider archive too, and this host does not link it.
 * Refusing every document is honest for the same reason the regexp stub is: no test here parses
 * one, and a test that did would see a SyntaxError rather than a quiet wrong answer. */
int32_t kiln_json_parse(const kiln_string *source, const kiln_json_node **out_nodes,
                        size_t *out_count, kiln_string *out_text) {
    (void)source; (void)out_nodes; (void)out_count; (void)out_text;
    return -1;
}
"#;

/// Link-only Realm provider for backend unit tests.
///
/// Scheduler behavior is covered through the real Rust archive in `kiln-native`; these tests
/// exercise generated scalar C directly. The fixture therefore owns and disposes any work it
/// is unexpectedly given without duplicating a second event loop in the backend test module.
const REALM_FIXTURE: &str = r#"
static uint64_t kiln_test_next_timer_id = 1;
static int kiln_test_realm_active = 0;
int kiln_test_string_code_generation_allowed = 1;
static uint64_t kiln_test_symbol_id = 1;
static uint16_t kiln_test_symbol_key[256];
static uint64_t kiln_test_symbol_key_len = 0;
static uint32_t kiln_test_callable_tags[256];
static uint64_t kiln_test_callable_codes[256];
static void *kiln_test_callable_stores[256];
static size_t kiln_test_callable_store_count = 0;
static kiln_value kiln_test_value_slots[13];
static kiln_gc_visit_roots_fn kiln_test_registered_visitors[32];
static void *kiln_test_registered_contexts[32];
static size_t kiln_test_registered_count = 0;
static int kiln_test_legacy_roots = 0;
static int kiln_test_fail_root_registration = 0;
static uint64_t kiln_test_entropy_seed = 0;
static uint64_t kiln_test_entropy_state = 0;
static int kiln_test_entropy_initialized = 0;

/* Test-only comparison: a string value carries a headed `kiln_string` at `ptr`, so a byte
 * memcmp or strstr on `ptr` reads the header's words as units. Compare units instead; every
 * expected spelling used here is ASCII, where a unit is a byte. */
static int kiln_test_units_equal(kiln_value value, const char *bytes, size_t len) {
    const kiln_string *string = (const kiln_string *)value.ptr;
    size_t index;
    if (value.tag != KILN_VALUE_STRING || value.ptr == NULL || string->unit_len != len) {
        return 0;
    }
    for (index = 0; index < len; ++index) {
        if (kiln_string_unit_at(string, index) != (uint8_t)bytes[index]) return 0;
    }
    return 1;
}
/* Same unit comparison for a bare headed key, as stored on a property. */
static int kiln_test_key_equal(const kiln_string *key, const char *bytes, size_t len) {
    size_t index;
    if (key == NULL || key->unit_len != len) return 0;
    for (index = 0; index < len; ++index) {
        if (kiln_string_unit_at(key, index) != (uint8_t)bytes[index]) return 0;
    }
    return 1;
}
static int kiln_test_units_contain(kiln_value value, const char *needle) {    const kiln_string *string = (const kiln_string *)value.ptr;
    size_t hay_len;
    size_t length;
    size_t start;
    size_t index;
    if (value.tag != KILN_VALUE_STRING || value.ptr == NULL || needle == NULL) return 0;
    hay_len = string->unit_len;
    length = strlen(needle);
    if (length == 0) return 1;
    if (length > hay_len) return 0;
    for (start = 0; start + length <= hay_len; ++start) {
        for (index = 0; index < length; ++index) {
            if (kiln_string_unit_at(string, start + index) != (uint8_t)needle[index]) break;
        }
        if (index == length) return 1;
    }
    return 0;
}

int kiln_realm_create(uint64_t *out_realm) {
    if (out_realm == NULL) return KILN_C99_INVALID_ARGUMENT;
    kiln_test_registered_count = 0;
    kiln_test_legacy_roots = 0;
    *out_realm = 1;
    return KILN_C99_OK;
}
int kiln_realm_set_string_code_generation(uint64_t realm, int allowed) {
    if (realm != 1 || (allowed != 0 && allowed != 1) || kiln_test_realm_active)
        return KILN_C99_INVALID_ARGUMENT;
    kiln_test_string_code_generation_allowed = allowed;
    return KILN_C99_OK;
}
int kiln_realm_enter(uint64_t realm, kiln_realm_entry_fn entry, void *context) {
    int status;
    if (realm != 1 || entry == NULL || kiln_test_realm_active) {
        return KILN_C99_INVALID_ARGUMENT;
    }
    kiln_test_realm_active = 1;
    status = entry(context);
    kiln_test_realm_active = 0;
    return status;
}
int kiln_realm_drive(uint64_t realm, void *host, void *thrown) {
    (void)host;
    (void)thrown;
    return realm == 1 ? KILN_C99_OK : KILN_C99_INVALID_ARGUMENT;
}
int kiln_realm_destroy(uint64_t realm) {
    if (realm != 1 || kiln_test_realm_active) return KILN_C99_INVALID_ARGUMENT;
    return KILN_C99_OK;
}
/* Here rather than beside the other provider stubs, because its argument is a struct the runtime
 * prelude declares and this fixture is the first text that follows the prelude. */
int32_t kiln_temporal_invoke(const kiln_temporal_call *call) { (void)call; return -1; }
int kiln_realm_is_active(void) { return kiln_test_realm_active; }
int kiln_realm_diagnostic_reset(void) { return KILN_C99_OK; }
int kiln_realm_diagnostic_note(const uint8_t *site, uint64_t site_len, uint64_t instruction,
                               const uint8_t *operation, uint64_t operation_len, uint64_t depth,
                               const uint8_t *detail, uint64_t detail_len,
                               const uint8_t *function, uint64_t function_len,
                               uint64_t method, uint64_t receiver_tag, int has_method) {
    (void)site; (void)site_len; (void)instruction; (void)operation; (void)operation_len;
    (void)depth; (void)detail; (void)detail_len; (void)function; (void)function_len;
    (void)method; (void)receiver_tag; (void)has_method;
    return KILN_C99_OK;
}
int kiln_realm_diagnostic_render(uint8_t *out, uint64_t capacity, uint64_t *out_len) {
    (void)out; (void)capacity;
    if (out_len == NULL) return KILN_C99_INVALID_ARGUMENT;
    *out_len = 0;
    return KILN_C99_INVALID_ARGUMENT;
}
/* `node:path`'s decompositions come from the Rust provider, which this test host does not link:
 * the backend's own tests compile generated C against a shim, not against the shipped archive.
 * Refusing here is the honest answer -- a second implementation living in the test host is exactly
 * the drift the provider exists to end. `kiln test conformance/*` and `scripts/native.sh` run
 * against the real archive and are where these are exercised. */
int kiln_node_path_part(uint32_t part, const uint8_t *path, uint64_t path_len,
                        const uint8_t *suffix, uint64_t suffix_len, uint8_t *out,
                        uint64_t capacity, uint64_t *out_len) {
    (void)part; (void)path; (void)path_len; (void)suffix; (void)suffix_len; (void)out;
    (void)capacity;
    if (out_len == NULL) return KILN_C99_INVALID_ARGUMENT;
    *out_len = 0;
    return KILN_C99_INVALID_ARGUMENT;
}
int kiln_string_case(const uint16_t *units, uint64_t count, uint32_t upper, uint16_t *out,
                     uint64_t capacity, uint64_t *out_len) {
    (void)units; (void)count; (void)upper; (void)out; (void)capacity;
    if (out_len == NULL) return KILN_C99_INVALID_ARGUMENT;
    *out_len = 0;
    return KILN_C99_INVALID_ARGUMENT;
}
int kiln_string_normalize(const uint16_t *units, uint64_t count, uint32_t form, uint16_t *out,
                          uint64_t capacity, uint64_t *out_len) {
    (void)units; (void)count; (void)form; (void)out; (void)capacity;
    if (out_len == NULL) return KILN_C99_INVALID_ARGUMENT;
    *out_len = 0;
    return KILN_C99_INVALID_ARGUMENT;
}
/* Provider-disabled C fixtures still link every prelude handler. Provider semantics are covered
 * by the native-provider and native graph suites rather than these collector fixtures. */
double kiln_date_parse(const kiln_string *source) {
    (void)source;
    return NAN;
}
int kiln_intl_string_case(const uint16_t *units, uint64_t count, const uint8_t *locale,
                         uint64_t locale_len, uint32_t uppercase, uint16_t *out,
                         uint64_t capacity, uint64_t *out_len) {
    (void)units; (void)count; (void)locale; (void)locale_len; (void)uppercase;
    (void)out; (void)capacity; (void)out_len;
    return KILN_C99_INVALID_ARGUMENT;
}
int kiln_intl_datetime_format(const uint8_t *spec, uint64_t spec_len, double epoch,
                            uint32_t operation, const uint8_t **out, uint64_t *out_len) {
    (void)spec; (void)spec_len; (void)epoch; (void)operation; (void)out; (void)out_len;
    return KILN_C99_INVALID_ARGUMENT;
}
int kiln_intl_collate(const uint16_t *left, uint64_t left_len, const uint16_t *right,
                      uint64_t right_len, const uint8_t *spec, uint64_t spec_len,
                      int32_t *out_order) {
    (void)left; (void)left_len; (void)right; (void)right_len; (void)spec; (void)spec_len;
    (void)out_order;
    return KILN_C99_INVALID_ARGUMENT;
}
int kiln_url_parse(const uint8_t *input, uint64_t input_len, const uint8_t *base,
                   uint64_t base_len, uint8_t *out, uint64_t capacity, uint64_t *out_len) {
    (void)input; (void)input_len; (void)base; (void)base_len; (void)out; (void)capacity;
    if (out_len == NULL) return KILN_C99_INVALID_ARGUMENT;
    *out_len = 0;
    return KILN_C99_INVALID_ARGUMENT;
}
int kiln_url_set(const uint8_t *href, uint64_t href_len, uint32_t field, const uint8_t *value,
                 uint64_t value_len, uint8_t *out, uint64_t capacity, uint64_t *out_len) {
    (void)href; (void)href_len; (void)field; (void)value; (void)value_len; (void)out;
    (void)capacity;
    if (out_len == NULL) return KILN_C99_INVALID_ARGUMENT;
    *out_len = 0;
    return KILN_C99_INVALID_ARGUMENT;
}
int kiln_text_decoder_label(const uint8_t *label, uint64_t label_len, uint8_t *out,
                            uint64_t capacity, uint64_t *out_len) {
    (void)label; (void)label_len; (void)out; (void)capacity;
    if (out_len == NULL) return KILN_C99_INVALID_ARGUMENT;
    *out_len = 0;
    return KILN_C99_INVALID_ARGUMENT;
}
int kiln_text_decode(const uint8_t *name, uint64_t name_len, const uint8_t *bytes,
                     uint64_t bytes_len, uint32_t flags, uint8_t *out, uint64_t capacity,
                     uint64_t *out_len) {
    (void)name; (void)name_len; (void)bytes; (void)bytes_len; (void)flags; (void)out;
    (void)capacity;
    if (out_len == NULL) return KILN_C99_INVALID_ARGUMENT;
    *out_len = 0;
    return KILN_C99_INVALID_ARGUMENT;
}
int kiln_text_decoder_state_create(const uint8_t *label, uint64_t label_len, uint32_t flags,
                                   void **out_state, uint8_t *out, uint64_t capacity,
                                   uint64_t *out_len) {
    (void)label; (void)label_len; (void)flags; (void)out_state; (void)out; (void)capacity;
    if (out_len == NULL) return KILN_C99_INVALID_ARGUMENT;
    *out_len = 0;
    return KILN_C99_INVALID_ARGUMENT;
}
int kiln_text_decoder_state_decode(void *state, const uint8_t *bytes, uint64_t bytes_len,
                                   uint32_t stream, uint8_t *out, uint64_t capacity,
                                   uint64_t *out_len) {
    (void)state; (void)bytes; (void)bytes_len; (void)stream; (void)out; (void)capacity;
    if (out_len == NULL) return KILN_C99_INVALID_ARGUMENT;
    *out_len = 0;
    return KILN_C99_INVALID_ARGUMENT;
}
void kiln_text_decoder_state_free(void *state) { (void)state; }
int kiln_node_path_relative(const uint8_t *from, uint64_t from_len, const uint8_t *to,
                            uint64_t to_len, uint8_t *out, uint64_t capacity, uint64_t *out_len) {
    (void)from; (void)from_len; (void)to; (void)to_len; (void)out; (void)capacity;
    if (out_len == NULL) return KILN_C99_INVALID_ARGUMENT;
    *out_len = 0;
    return KILN_C99_INVALID_ARGUMENT;
}
int kiln_ffi_open(const uint8_t *path, uint64_t path_len, uint64_t *out_library) {
    (void)path; (void)path_len; (void)out_library; return KILN_C99_INVALID_ARGUMENT;
}
int kiln_ffi_close(uint64_t library) { (void)library; return KILN_C99_INVALID_ARGUMENT; }
int kiln_ffi_prepare(uint64_t library, const uint8_t *symbol, uint64_t symbol_len,
                     const uint32_t *argument_types, uint64_t argument_count,
                     uint32_t result_type, uint64_t *out_function) {
    (void)library; (void)symbol; (void)symbol_len; (void)argument_types;
    (void)argument_count; (void)result_type; (void)out_function;
    return KILN_C99_INVALID_ARGUMENT;
}
int kiln_ffi_call(uint64_t function, const kiln_ffi_value *arguments,
                  uint64_t argument_count, kiln_ffi_value *out_result) {
    (void)function; (void)arguments; (void)argument_count; (void)out_result;
    return KILN_C99_INVALID_ARGUMENT;
}
int kiln_ffi_register_callback(const uint32_t *argument_types, uint64_t argument_count,
                               uint32_t result_type, kiln_ffi_dispatch_fn dispatch,
                               void *context, uint64_t *out_pointer) {
    (void)argument_types; (void)argument_count; (void)result_type; (void)dispatch;
    (void)context; (void)out_pointer; return KILN_C99_INVALID_ARGUMENT;
}
int kiln_ffi_unregister_callback(uint64_t pointer) {
    (void)pointer; return KILN_C99_INVALID_ARGUMENT;
}
int kiln_ffi_last_error(const uint8_t **out_bytes, uint64_t *out_len) {
    if (out_bytes != NULL) *out_bytes = NULL;
    if (out_len != NULL) *out_len = 0;
    return KILN_C99_OK;
}
/* The registry answers in the same headed units the heap uses: identity is compared unit
 * by unit, so a Latin1 registration and its UTF-16 spelling are the same key. Units are
 * stored widened; every test key here is ASCII, where the spelling is unambiguous. */
int kiln_realm_symbol_for(const kiln_string *key, uint64_t *out_id) {
    size_t index;
    int same;
    if (!kiln_test_realm_active || out_id == NULL || key == NULL ||
        key->unit_len > sizeof kiln_test_symbol_key / sizeof *kiln_test_symbol_key) {
        return KILN_C99_INVALID_ARGUMENT;
    }
    if (kiln_test_symbol_key_len == 0) {
        for (index = 0; index < key->unit_len; ++index) {
            kiln_test_symbol_key[index] = kiln_string_unit_at(key, index);
        }
        kiln_test_symbol_key_len = key->unit_len;
    } else {
        same = kiln_test_symbol_key_len == key->unit_len;
        for (index = 0; same && index < key->unit_len; ++index) {
            if (kiln_test_symbol_key[index] != kiln_string_unit_at(key, index)) {
                same = 0;
            }
        }
        if (!same) {
            ++kiln_test_symbol_id;
            for (index = 0; index < key->unit_len; ++index) {
                kiln_test_symbol_key[index] = kiln_string_unit_at(key, index);
            }
            kiln_test_symbol_key_len = key->unit_len;
        }
    }
    *out_id = kiln_test_symbol_id;
    return KILN_C99_OK;
}
int kiln_realm_symbol_key_for(uint64_t id, kiln_string *out_key) {
    if (!kiln_test_realm_active || id != kiln_test_symbol_id || out_key == NULL) {
        return KILN_C99_INVALID_ARGUMENT;
    }
    out_key->unit_len = (uint32_t)kiln_test_symbol_key_len;
    out_key->flags = KILN_STRING_UTF16;
    out_key->data = kiln_test_symbol_key;
    return KILN_C99_OK;
}
int kiln_realm_callable_store(uint32_t tag, uint64_t code, void *candidate,
                              kiln_drop_opaque_fn drop_candidate, void **out_store) {
    size_t index;
    if (!kiln_test_realm_active || out_store == NULL ||
        ((candidate == NULL) != (drop_candidate == NULL))) {
        return KILN_C99_INVALID_ARGUMENT;
    }
    if (candidate == NULL) {
        *out_store = NULL;
        return KILN_C99_OK;
    }
    for (index = 0; index < kiln_test_callable_store_count; ++index) {
        if (kiln_test_callable_tags[index] == tag && kiln_test_callable_codes[index] == code) {
            *out_store = kiln_test_callable_stores[index];
            return KILN_C99_OK;
        }
    }
    if (kiln_test_callable_store_count == 256) return KILN_C99_OUT_OF_MEMORY;
    index = kiln_test_callable_store_count++;
    kiln_test_callable_tags[index] = tag;
    kiln_test_callable_codes[index] = code;
    kiln_test_callable_stores[index] = candidate;
    *out_store = candidate;
    return KILN_C99_OK;
}
int kiln_realm_value_slot(uint32_t key, void **out_slot) {
    if (!kiln_test_realm_active || key == 0 || key > 13 || out_slot == NULL)
        return KILN_C99_INVALID_ARGUMENT;
    *out_slot = &kiln_test_value_slots[key - 1];
    return KILN_C99_OK;
}
int kiln_realm_entropy_word(uint64_t seed, uint64_t *out_word) {
    uint64_t mixed;
    if (!kiln_test_realm_active || out_word == NULL) return KILN_C99_INVALID_ARGUMENT;
    if (!kiln_test_entropy_initialized) {
        kiln_test_entropy_seed = seed;
        kiln_test_entropy_state = seed;
        kiln_test_entropy_initialized = 1;
    } else if (kiln_test_entropy_seed != seed) {
        return KILN_C99_INVALID_ARGUMENT;
    }
    kiln_test_entropy_state += UINT64_C(0x9E3779B97F4A7C15);
    mixed = kiln_test_entropy_state;
    mixed = (mixed ^ (mixed >> 30)) * UINT64_C(0xBF58476D1CE4E5B9);
    mixed = (mixed ^ (mixed >> 27)) * UINT64_C(0x94D049BB133111EB);
    *out_word = mixed ^ (mixed >> 31);
    return KILN_C99_OK;
}
/* The fixture has no Tokio runtime and no HTTP engine, so it reports nothing. The dump this
 * feeds is unreachable here anyway: the fixture refuses to attach a heap, and the introspect
 * hook declines on a fallback heap. */
int kiln_realm_runtime_report(uint64_t *out_fields, uint64_t capacity, uint64_t *out_count) {
    (void)out_fields;
    (void)capacity;
    if (out_count == NULL) return KILN_C99_INVALID_ARGUMENT;
    *out_count = 0;
    return KILN_C99_INVALID_ARGUMENT;
}
/* A test may host heaps of its own (`kiln_test_heap`), switching between them by bumping
 * `kiln_test_generation` the way a realm entry does; by default there is none. */
static void *kiln_test_heap;
static uint64_t kiln_test_generation = 1;
void *kiln_realm_gc_current_heap(void) { return kiln_test_heap; }
const uint64_t *kiln_realm_generation_slot(void) { return &kiln_test_generation; }
/* The backend fixture links no Rust provider. Its registry mock enforces the same exact
 * identities and phase gate; the real provider registry has its own Rust tests. */
static uint64_t graph_active_realm;
static uint64_t graph_staged_reservation;
static uint64_t graph_staged_request;
static uint64_t graph_staged_receiver;
static void *graph_staged_capsule;
static int graph_staged_phase;
int kiln_realm_graph_capsule_stage(uint64_t reservation, uint64_t receiver,
                                   uint64_t request, void *capsule) {
    if (graph_active_realm != 1 || receiver != 2 || reservation == 0 || request == 0 ||
        capsule == NULL || graph_staged_phase != 0) return -1;
    graph_staged_reservation = reservation;
    graph_staged_receiver = receiver;
    graph_staged_request = request;
    graph_staged_capsule = capsule;
    graph_staged_phase = 1;
    return 0;
}
int kiln_realm_graph_capsule_peek(uint64_t reservation, uint64_t sender,
                                  uint64_t request, void **out) {
    *out = NULL;
    if (sender != 1 || (graph_active_realm != sender &&
        graph_active_realm != graph_staged_receiver) || graph_staged_phase != 1 ||
        graph_staged_reservation != reservation || graph_staged_request != request) return -1;
    *out = graph_staged_capsule;
    return 0;
}
int kiln_realm_graph_capsule_detached(uint64_t reservation, uint64_t request) {
    if (graph_active_realm != 1 || graph_staged_phase != 1 ||
        reservation != graph_staged_reservation || request != graph_staged_request) return -1;
    graph_staged_phase = 2;
    return 0;
}
int kiln_realm_graph_capsule_cancel(uint64_t reservation, uint64_t request) {
    if (graph_active_realm != 1 || graph_staged_phase != 1 ||
        reservation != graph_staged_reservation || request != graph_staged_request) return -1;
    graph_staged_phase = 0;
    graph_staged_capsule = NULL;
    return 0;
}
int kiln_realm_graph_capsule_take(uint64_t reservation, uint64_t sender,
                                  uint64_t request, void **out) {
    *out = NULL;
    if (graph_active_realm != 2 || sender != 1 || graph_staged_phase != 2 ||
        reservation != graph_staged_reservation || request != graph_staged_request) return -1;
    *out = graph_staged_capsule;
    graph_staged_capsule = NULL;
    graph_staged_phase = 0;
    return 0;
}
void *kiln_realm_process_shared(uint64_t size) {
    static union { uint64_t align; unsigned char bytes[4096]; } block;
    return size <= sizeof block.bytes ? (void *)block.bytes : NULL;
}
/* Reclamation (the provider's `reclaim.rs`). By default the fixture is a process with one thread,
 * so nothing retired is ever under another thread's reader. A test standing for other threads
 * running realms sets `kiln_test_other_threads`, and moves `kiln_test_safe_below` past a tag to
 * say they have all passed a turn boundary since it was taken. */
static int kiln_test_other_threads;
static uint64_t kiln_test_epoch = 1;
static uint64_t kiln_test_safe_below = 1;
uint64_t kiln_realm_retire_tag(void) {
    uint64_t tag = kiln_test_epoch++;
    return kiln_test_other_threads ? tag : 0;
}
int kiln_realm_retired_safe(uint64_t tag) { return tag == 0 || tag < kiln_test_safe_below; }
int kiln_realm_retire(void *record) { free(record); return 0; }
static uint64_t kiln_test_shared_retains;
static uint64_t kiln_test_shared_releases;
uint64_t kiln_shared_graph_retain(uint32_t graph, uint32_t generation, uint32_t node) {
    if (graph != 17 || generation != 1 || node != 0) return 0;
    return ++kiln_test_shared_retains;
}
int kiln_shared_graph_release(uint64_t lease) {
    if (lease == 0) return -1;
    ++kiln_test_shared_releases;
    return 0;
}
int kiln_shared_graph_view(uint64_t lease, kiln_shared_arena_view *out) {
    static struct {
        kiln_shared_node_record node;
        kiln_shared_field_record field;
        uint16_t key;
    } arena = {
        .node = { .kind = KILN_SHARED_ARENA_RECORD,
                  .fields = sizeof(kiln_shared_node_record), .field_count = 1 },
        .field = { .key_offset = sizeof(kiln_shared_node_record) +
                                 sizeof(kiln_shared_field_record),
                   .key_length = 1, .kind = KILN_SHARED_FIELD_NUMBER, .enumerable = 1 },
        .key = 'x',
    };
    double number = 7.0;
    if (lease == 0 || out == NULL) return -1;
    memcpy(&arena.field.payload, &number, sizeof number);
    out->base = (const uint8_t *)&arena;
    out->length = sizeof arena;
    out->node_count = 1;
    out->layout_version = KILN_SHARED_ARENA_LAYOUT_VERSION;
    return 0;
}
int kiln_shared_graph_info(uint64_t lease, kiln_shared_info *out) {
    if (lease == 0 || out == NULL) return -1;
    out->array = 0;
    out->null_prototype = 0;
    out->length = 0;
    out->field_count = 1;
    return 0;
}
int kiln_shared_graph_find_field(uint64_t lease, const uint16_t *key,
                                  uint64_t key_length, uint64_t *out_index) {
    if (lease == 0 || key == NULL || out_index == NULL) return -1;
    if (key_length != 1 || key[0] != 'x') return 1;
    *out_index = 0;
    return 0;
}
int kiln_shared_graph_field(uint64_t lease, uint64_t index, kiln_shared_field_info *out,
                            uint16_t *key_out, uint64_t key_capacity,
                            uint16_t *value_out, uint64_t value_capacity) {
    double number = 7.0;
    (void)value_out; (void)value_capacity;
    if (lease == 0 || index != 0 || out == NULL) return -1;
    out->kind = KILN_SHARED_FIELD_NUMBER;
    out->enumerable = 1;
    out->key_length = 1;
    out->value_length = 0;
    memcpy(&out->bits, &number, sizeof number);
    out->generation = 0;
    out->node = 0;
    if (key_out != NULL) {
        if (key_capacity < 1) return -2;
        key_out[0] = 'x';
    }
    return 0;
}
int kiln_shared_graph_build(uint64_t root, kiln_shared_source_inspect_fn inspect,
                            kiln_shared_source_unit_fn unit, void *context,
                            kiln_shared_build_result *out) {
    (void)root; (void)inspect; (void)unit; (void)context; (void)out;
    return -2;
}
int kiln_shared_graph_refusal(uint32_t kind, const char **text) {
    (void)kind;
    *text = "cannot share a function";
    return 0;
}
int kiln_realm_gc_attach(const kiln_gc_hooks *hooks, void *candidate, void **out_heap) {
    /* The fixture has no realm heap: every unit collects nothing and keeps its fallback heap. */
    (void)hooks;
    (void)candidate;
    if (out_heap != NULL) *out_heap = NULL;
    return KILN_C99_INVALID_ARGUMENT;
}
int kiln_realm_gc_register_roots(kiln_gc_roots_fn roots, void *context, uint32_t flags) {
    (void)roots;
    (void)context;
    (void)flags;
    kiln_test_legacy_roots = 1;
    return KILN_C99_OK;
}
int kiln_realm_gc_register_roots_with_visitor(kiln_gc_roots_fn roots,
                                               kiln_gc_visit_roots_fn visit_roots,
                                               void *context, uint32_t flags) {
    (void)roots;
    (void)flags;
    if (kiln_test_fail_root_registration) return KILN_C99_HOST_ERROR;
    if (!kiln_test_realm_active || visit_roots == NULL || kiln_test_registered_count == 32)
        return KILN_C99_INVALID_ARGUMENT;
    kiln_test_registered_visitors[kiln_test_registered_count] = visit_roots;
    kiln_test_registered_contexts[kiln_test_registered_count++] = context;
    return KILN_C99_OK;
}
int kiln_realm_gc_has_root_visitor(void *heap, kiln_gc_visit_roots_fn visit_roots,
                                   void *context) {
    size_t index;
    if (heap == NULL || !kiln_test_realm_active || visit_roots == NULL)
        return KILN_C99_INVALID_ARGUMENT;
    for (index = 0; index < kiln_test_registered_count; ++index)
        if (kiln_test_registered_visitors[index] == visit_roots &&
            kiln_test_registered_contexts[index] == context) return 1;
    return 0;
}
static kiln_value kiln_test_gc_root;
int kiln_realm_gc_mark_roots(void *heap) {
    kiln_c99_gc_mark_value((kiln_gc_heap *)heap, &kiln_test_gc_root);
    return KILN_C99_OK;
}
/* This shim enumerates only the provider state it models. The real archive's visitor also
 * covers queued work, continuations and host handles; its fixture tests that contract. */
int kiln_realm_gc_visit_provider_roots(void *heap, kiln_gc_visit_root_fn visit, void *context) {
    size_t index;
    (void)heap;
    if (!kiln_test_realm_active || visit == NULL) return KILN_C99_INVALID_ARGUMENT;
    for (index = 0; index < 13; ++index)
        if (visit(context, &kiln_test_value_slots[index], KILN_GC_VISIT_VALUE) != 0)
            return KILN_C99_HOST_ERROR;
    for (index = 0; index < kiln_test_callable_store_count; ++index)
        if (visit(context, kiln_test_callable_stores[index], KILN_GC_VISIT_BLOCK) != 0)
            return KILN_C99_HOST_ERROR;
    for (index = 0; index < kiln_test_registered_count; ++index)
        if (kiln_test_registered_visitors[index](kiln_test_registered_contexts[index],
                                                 visit, context) != 0)
            return KILN_C99_HOST_ERROR;
    return kiln_test_legacy_roots ? 1 : KILN_C99_OK;
}
int kiln_realm_now(double *out_milliseconds) {
    if (!kiln_test_realm_active || out_milliseconds == NULL) {
        return KILN_C99_INVALID_ARGUMENT;
    }
    *out_milliseconds = 0.0;
    return KILN_C99_OK;
}
int kiln_realm_performance_now(double *out_milliseconds) {
    return kiln_realm_now(out_milliseconds);
}
int kiln_realm_now_nanoseconds(int64_t *out_high, uint64_t *out_low) {
    if (!kiln_test_realm_active || out_high == NULL || out_low == NULL) {
        return KILN_C99_INVALID_ARGUMENT;
    }
    *out_high = 0;
    *out_low = 0;
    return KILN_C99_OK;
}
int kiln_realm_queue_microtask(kiln_realm_work_fn run,
                               kiln_realm_drop_fn drop,
                               void *work) {
    (void)run;
    if (drop != NULL) drop(work);
    return KILN_C99_OK;
}
/* The fixture drives nothing, so a registered lane is never drained: the prelude's ring holds
 * what a unit test queues until the process exits, exactly as this fixture's microtask queue
 * dropped it before. */
int kiln_realm_set_promise_lane(kiln_realm_lane_run_fn run,
                                kiln_realm_lane_pending_fn pending,
                                void *context) {
    (void)run;
    (void)pending;
    (void)context;
    return KILN_C99_OK;
}
int kiln_realm_queue_next_tick(kiln_realm_work_fn run,
                               kiln_realm_drop_fn drop,
                               void *work) {
    (void)run;
    if (drop != NULL) drop(work);
    return KILN_C99_OK;
}
/* No launcher ran, so there is no command line. The prelude reads zero here and leaves
 * `process.argv` empty, which is what a harness-hosted realm should report. */
int kiln_realm_argv_count(uint64_t *out_count) {
    if (!kiln_test_realm_active || out_count == NULL) return KILN_C99_INVALID_ARGUMENT;
    *out_count = 0;
    return KILN_C99_OK;
}
int kiln_realm_string_code_generation_allowed(int *out_allowed) {
    if (!kiln_test_realm_active || out_allowed == NULL) return KILN_C99_INVALID_ARGUMENT;
    *out_allowed = kiln_test_string_code_generation_allowed;
    return KILN_C99_OK;
}
int kiln_realm_argv_at(uint64_t index, const uint8_t **out_bytes, uint64_t *out_len) {
    (void)index;
    (void)out_bytes;
    (void)out_len;
    return KILN_C99_INVALID_ARGUMENT;
}
static int kiln_test_set_timer(double delay_ms,
                               kiln_realm_work_fn run,
                               kiln_realm_drop_fn drop,
                               void *work,
                               uint64_t *out_id) {
    (void)delay_ms;
    (void)run;
    if (out_id == NULL) return KILN_C99_INVALID_ARGUMENT;
    *out_id = kiln_test_next_timer_id++;
    if (drop != NULL) drop(work);
    return KILN_C99_OK;
}
int kiln_realm_set_timeout(double delay_ms,
                           kiln_realm_work_fn run,
                           kiln_realm_drop_fn drop,
                           void *work,
                           uint64_t *out_id) {
    return kiln_test_set_timer(delay_ms, run, drop, work, out_id);
}
int kiln_realm_set_interval(double delay_ms,
                            kiln_realm_work_fn run,
                            kiln_realm_drop_fn drop,
                            void *work,
                            uint64_t *out_id) {
    return kiln_test_set_timer(delay_ms, run, drop, work, out_id);
}
int kiln_realm_set_immediate(kiln_realm_work_fn run,
                             kiln_realm_drop_fn drop,
                             void *work,
                             uint64_t *out_id) {
    return kiln_test_set_timer(0.0, run, drop, work, out_id);
}
int kiln_realm_queue_io(kiln_realm_work_fn run,
                        kiln_realm_drop_fn drop,
                        void *work,
                        uint64_t *out_id) {
    return kiln_test_set_timer(0.0, run, drop, work, out_id);
}
int kiln_realm_clear_timer(uint64_t id) {
    (void)id;
    return KILN_C99_OK;
}
int kiln_realm_refresh_timer(uint64_t id) {
    (void)id;
    return 0;
}
int kiln_realm_rearm_timeout(uint64_t id, double delay_ms, kiln_realm_work_fn run,
                             kiln_realm_drop_fn drop, void *work) {
    (void)id;
    (void)delay_ms;
    (void)run;
    (void)drop;
    (void)work;
    return 1;
}
int kiln_realm_set_timer_ref(uint64_t id, int referenced) {
    (void)id;
    (void)referenced;
    return KILN_C99_OK;
}
int kiln_realm_start_active_host_op(const kiln_async_host_op *operation,
                                    kiln_realm_completion_fn run,
                                    kiln_realm_drop_fn drop,
                                    void *work,
                                    uint64_t *out_request) {
    (void)operation;
    (void)run;
    if (out_request != NULL) *out_request = 1;
    if (drop != NULL) drop(work);
    return KILN_C99_INVALID_ARGUMENT;
}
int kiln_realm_cancel_active_request(uint64_t request) {
    (void)request;
    return KILN_REALM_ASYNC_NOT_FOUND;
}
int kiln_realm_cancel_active_request_if_pending(uint64_t request) {
    (void)request;
    return 0;
}

/* Scalar backend fixtures never exercise Node HTTP; production artifacts link these symbols
 * from the Rust realm provider. */
int kiln_realm_http_server_attach_active(uint64_t resource, kiln_realm_http_event_fn run,
                                         kiln_realm_drop_fn drop, void *work) {
    (void)resource; (void)run; if (drop != NULL) drop(work); return KILN_C99_INVALID_ARGUMENT;
}
int kiln_realm_http_server_attach_with_options_active(
    uint64_t resource, int allow_half_close, kiln_realm_http_event_fn run,
    kiln_realm_drop_fn drop, void *work) {
    (void)allow_half_close;
    return kiln_realm_http_server_attach_active(resource, run, drop, work);
}
int kiln_realm_http_server_close_active(uint64_t resource, int mode) {
    (void)resource; (void)mode;
    return KILN_C99_INVALID_ARGUMENT;
}
int kiln_realm_http_client_attach_active(uint64_t resource, kiln_realm_http_event_fn run,
                                         kiln_realm_drop_fn drop, void *work) {
    (void)resource; (void)run; if (drop != NULL) drop(work); return KILN_C99_INVALID_ARGUMENT;
}
int kiln_realm_http_client_request_id_active(uint64_t *out_id) {
    if (out_id == NULL) return KILN_C99_INVALID_ARGUMENT;
    *out_id = 1;
    return KILN_C99_OK;
}
int kiln_realm_http_agent_acquire_active(const uint8_t *host, uint64_t host_len,
                                         uint16_t port, uint64_t request,
                                         uint64_t *out_resource) {
    (void)host; (void)host_len; (void)port; (void)request;
    if (out_resource == NULL) return KILN_C99_INVALID_ARGUMENT;
    *out_resource = 0;
    return KILN_REALM_HTTP_AGENT_CONNECT;
}
int kiln_realm_http_client_owner_active(uint64_t resource, void **out_owner) {
    (void)resource;
    if (out_owner == NULL) return KILN_C99_INVALID_ARGUMENT;
    *out_owner = NULL;
    return KILN_C99_INVALID_ARGUMENT;
}
/* Provider seams retained by the current prelude. These fixtures do not execute transports. */
int kiln_realm_http_agent_connect_failed_active(const uint8_t *host, uint64_t host_len,
                                                uint16_t port, uint64_t request,
                                                uint64_t *out_request) {
    (void)host; (void)host_len; (void)port; (void)request;
    if (out_request != NULL) *out_request = 0;
    return KILN_C99_INVALID_ARGUMENT;
}
int kiln_realm_http_agent_cancel_connect_active(const uint8_t *host, uint64_t host_len,
                                                uint16_t port, uint64_t request,
                                                uint64_t *out_request) {
    return kiln_realm_http_agent_connect_failed_active(host, host_len, port, request, out_request);
}
int kiln_realm_http_upgrade_write_active_with_fence(uint64_t resource, uint64_t id,
                                                    const uint8_t *data, uint64_t data_len,
                                                    uint64_t *out_fence) {
    (void)resource; (void)id; (void)data; (void)data_len;
    if (out_fence != NULL) *out_fence = 0;
    return KILN_C99_INVALID_ARGUMENT;
}
int kiln_crypto_key_import(const uint8_t *data, size_t length, uint32_t kind,
                           uint64_t *out_handle) {
    (void)data; (void)length; (void)kind;
    if (out_handle != NULL) *out_handle = 0;
    return -3;
}
int kiln_crypto_key_public(uint64_t handle, uint64_t *out_handle) {
    (void)handle;
    if (out_handle != NULL) *out_handle = 0;
    return -3;
}
int kiln_crypto_key_info(uint64_t handle, uint32_t *out_type, uint32_t *out_bits) {
    (void)handle;
    if (out_type != NULL) *out_type = 0;
    if (out_bits != NULL) *out_bits = 0;
    return -3;
}
int kiln_crypto_key_export(uint64_t handle, uint32_t format, uint32_t encoding,
                           uint8_t *output, size_t capacity, size_t *out_length) {
    (void)handle; (void)format; (void)encoding; (void)output; (void)capacity;
    if (out_length != NULL) *out_length = 0;
    return -3;
}
int kiln_crypto_key_free(uint64_t handle) {
    (void)handle;
    return -3;
}
int kiln_realm_http_agent_connected_active(const uint8_t *host, uint64_t host_len,
                                           uint16_t port, uint64_t resource,
                                           uint64_t request) {
    (void)host; (void)host_len; (void)port; (void)resource; (void)request;
    return 1;
}
int kiln_realm_http_agent_can_retain_active(uint64_t resource, int reusable) {
    (void)resource;
    return reusable != 0;
}
int kiln_realm_http_agent_release_active(uint64_t resource, int reusable,
                                         uint64_t *out_request) {
    (void)resource; (void)reusable;
    if (out_request == NULL) return KILN_C99_INVALID_ARGUMENT;
    *out_request = 0;
    return KILN_REALM_HTTP_AGENT_IDLE;
}
int kiln_realm_http_client_start_active(uint64_t resource,
                                        uint64_t id,
                                        const kiln_realm_http_request *request,
                                        uint64_t high_water_mark) {
    (void)resource; (void)id; (void)request; (void)high_water_mark;
    return KILN_C99_INVALID_ARGUMENT;
}
int kiln_realm_http_client_write_active(uint64_t resource,
                                        uint64_t id,
                                        const uint8_t *data, uint64_t data_len) {
    (void)resource; (void)id; (void)data; (void)data_len; return KILN_C99_INVALID_ARGUMENT;
}
int kiln_realm_http_client_end_active(uint64_t resource,
                                      uint64_t id,
                                      const uint8_t *data, uint64_t data_len) {
    (void)resource; (void)id; (void)data; (void)data_len; return KILN_C99_INVALID_ARGUMENT;
}
int kiln_realm_http_response_start_active(uint64_t resource, uint64_t id,
                                          const kiln_realm_http_response *response,
                                          uint64_t high_water_mark) {
    (void)resource; (void)id; (void)response; (void)high_water_mark;
    return KILN_C99_INVALID_ARGUMENT;
}
int kiln_realm_http_response_write_active(uint64_t resource, uint64_t id,
                                          const uint8_t *data, uint64_t data_len) {
    (void)resource; (void)id; (void)data; (void)data_len; return KILN_C99_INVALID_ARGUMENT;
}
int kiln_realm_http_response_end_active(uint64_t resource, uint64_t id,
                                        const uint8_t *data, uint64_t data_len) {
    (void)resource; (void)id; (void)data; (void)data_len; return KILN_C99_INVALID_ARGUMENT;
}
int kiln_node_fs_read_file_sync(const uint8_t *path, uint64_t path_len,
                               uint8_t **out_bytes, uint64_t *out_len, int32_t *out_err) {
    (void)path; (void)path_len;
    if (out_bytes != NULL) *out_bytes = NULL;
    if (out_len != NULL) *out_len = 0;
    if (out_err != NULL) *out_err = 1;
    return 0;
}
void kiln_node_fs_free_bytes(uint8_t *bytes, uint64_t len) {
    (void)bytes; (void)len;
}
int kiln_node_fs_write_file_sync(const uint8_t *path, uint64_t path_len,
                                const uint8_t *bytes, uint64_t len, int32_t *out_err) {
    (void)path; (void)path_len; (void)bytes; (void)len;
    if (out_err != NULL) *out_err = 1;
    return 0;
}
int kiln_node_fs_mkdir_sync(const uint8_t *path, uint64_t path_len,
                           int32_t recursive, int32_t *out_err) {
    (void)path; (void)path_len; (void)recursive;
    if (out_err != NULL) *out_err = 1;
    return 0;
}
int kiln_node_fs_stat_sync(const uint8_t *path, uint64_t path_len,
                          int32_t follow, uint64_t *out_size, uint32_t *out_flags,
                          double *out_mtime_ms, uint32_t *out_mode, int32_t *out_err) {
    (void)path; (void)path_len; (void)follow;
    if (out_size != NULL) *out_size = 0;
    if (out_flags != NULL) *out_flags = 0;
    if (out_mtime_ms != NULL) *out_mtime_ms = 0.0;
    if (out_mode != NULL) *out_mode = 0;
    if (out_err != NULL) *out_err = 1;
    return 0;
}
int kiln_node_fs_readdir_sync(const uint8_t *path, uint64_t path_len,
                             uint8_t **out_entries, uint64_t *out_len,
                             uint64_t *out_count, int32_t *out_err) {
    (void)path; (void)path_len;
    if (out_entries != NULL) *out_entries = NULL;
    if (out_len != NULL) *out_len = 0;
    if (out_count != NULL) *out_count = 0;
    if (out_err != NULL) *out_err = 1;
    return 0;
}
int kiln_node_fs_realpath_sync(const uint8_t *path, uint64_t path_len,
                              uint8_t **out_bytes, uint64_t *out_len, int32_t *out_err) {
    (void)path; (void)path_len;
    if (out_bytes != NULL) *out_bytes = NULL;
    if (out_len != NULL) *out_len = 0;
    if (out_err != NULL) *out_err = 1;
    return 0;
}
int kiln_node_fs_unlink_sync(const uint8_t *path, uint64_t path_len, int32_t *out_err) {
    (void)path; (void)path_len;
    if (out_err != NULL) *out_err = 1;
    return 0;
}
int kiln_node_fs_rmdir_sync(const uint8_t *path, uint64_t path_len, int32_t *out_err) {
    (void)path; (void)path_len;
    if (out_err != NULL) *out_err = 1;
    return 0;
}
"#;

/// A real build compiles the vendored number runtime and each emitted unit as separate
/// translation units, so every unit starts with `HEADER` and asks for POSIX visibility before
/// its first `#include`. These tests concatenate them instead, which puts the number
/// runtime's includes first and makes `HEADER`'s request arrive too late to be honoured. The
/// prologue is therefore repeated here, ahead of everything, so the one TU the tests build
/// sees what the many TUs a build produces each see.
///
/// The provider fixtures follow the module rather than preceding it: their signatures name
/// the headed `kiln_string` ABI, whose definition lives in `HEADER`'s prefix, while the calls
/// in the prelude see `foundation.c`'s forward declarations. A definition may follow its
/// calls; a use of an undeclared type may not, so this order is the only one that compiles.
pub(crate) fn linked_source(module_source: &str, wrapper: &str) -> String {
    format!(
        "#if !defined(_WIN32) && !defined(__APPLE__) && !defined(_POSIX_C_SOURCE)\n\
             #define _POSIX_C_SOURCE 200809L\n\
             #endif\n{}\n{}\n{}\n{}\n{}\n{}",
        kiln_native_provider_contract::number::runtime_source(),
        module_source,
        REGEXP_FIXTURE,
        REALM_FIXTURE,
        SEMANTIC_PROVIDER_FIXTURE,
        wrapper
    )
}

/// Link-only symbols for executable backend fixtures, which do not link the packaged Rust
/// semantic provider. Production compression and WebCrypto always resolve to their real archive.
const SEMANTIC_PROVIDER_FIXTURE: &str = r#"
int kiln_process_exec_file_sync(const kiln_string *file, const kiln_string *argv,
                                size_t argc, void **out) {
    (void)file; (void)argv; (void)argc;
    if (out != NULL) *out = NULL;
    return KILN_C99_INVALID_ARGUMENT;
}
int kiln_process_output_view(const void *result, const uint8_t **stdout_bytes,
                             size_t *stdout_len, const uint8_t **stderr_bytes,
                             size_t *stderr_len, int32_t *status, uint32_t *pid,
                             const uint8_t **error_bytes, size_t *error_len,
                             const uint8_t **signal_bytes, size_t *signal_len) {
    (void)result; (void)stdout_bytes; (void)stdout_len; (void)stderr_bytes; (void)stderr_len;
    (void)status; (void)pid; (void)error_bytes; (void)error_len;
    (void)signal_bytes; (void)signal_len;
    return KILN_C99_INVALID_ARGUMENT;
}
void kiln_process_output_free(void *result) { (void)result; }

int kiln_webcrypto_execute(uint32_t op, const uint8_t *key, size_t key_length,
                           const uint8_t *data, size_t data_length,
                           const uint8_t *iv, size_t iv_length,
                           const uint8_t *additional_data, size_t additional_data_length,
                           size_t output_length, uint8_t *output, size_t output_capacity,
                           size_t *required) {
    (void)op; (void)key; (void)key_length; (void)data; (void)data_length;
    (void)iv; (void)iv_length; (void)additional_data; (void)additional_data_length;
    (void)output_length; (void)output; (void)output_capacity;
    if (required != NULL) *required = 0;
    return -6;
}
void *kiln_compression_state_create(uint32_t format, uint32_t direction) {
    (void)format; (void)direction; return NULL;
}
void kiln_compression_state_free(void *state) { (void)state; }
int kiln_compression_state_push(void *state, const uint8_t *input, size_t input_length,
                                uint32_t finish, uint8_t **output, size_t *output_length) {
    (void)state; (void)input; (void)input_length; (void)finish; (void)output; (void)output_length;
    return -1;
}
void kiln_compression_output_free(uint8_t *output, size_t length) {
    (void)output; (void)length;
}
"#;

fn function(code: Vec<Instruction>) -> FunctionProto {
    FunctionProto {
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
        code,
        upvalues: Vec::new(),
        type_annotations: None,
    }
}

/// Compile one emitted unit with a `main` of the caller's choosing and report its exit code.
fn run_with_main(generated: &CModule, wrapper: &str, _label: &str) -> std::process::Output {
    fixture_cache::run(&linked_source(&generated.source, wrapper))
}

#[cfg(any())]
mod pre_typed_array_tests {

    #[test]
    fn executable_array_string_iterator_and_named_enumeration_are_distinct() {
        let generated = emit(&program(
            vec![Constant::String("@@iterator".into())],
            vec![
                Instruction::NewArray { count: 0 },
                Instruction::Const(0),
                Instruction::GetComputed,
                Instruction::Return,
            ],
            0,
        ))
        .expect("array string iterator probe emits");
        let wrapper = format!(
            r#"
int main(void) {{
    kiln_value result;
    if ({symbol}(NULL, &result) != KILN_C99_OK) return 2;
    return result.tag == KILN_VALUE_UNDEFINED ? 0 : 1;
}}
"#,
            symbol = generated.symbol
        );
        let run = run_with_main(&generated, &wrapper, "array-string-iterator");
        assert!(
            run.status.success(),
            "an absent ordinary @@iterator key must not expose Symbol.iterator"
        );

        let constants = vec![
            Constant::Number(0.0f64.to_bits()),
            Constant::Number(1.0f64.to_bits()),
            Constant::Number(2.0f64.to_bits()),
            Constant::Number(3.0f64.to_bits()),
            Constant::String("a".into()),
            Constant::String("b".into()),
            Constant::String("keys".into()),
            Constant::String("values".into()),
            Constant::String("entries".into()),
        ];
        let array = |code: &mut Vec<Instruction>| {
            code.extend([
                Instruction::Const(1),
                Instruction::NewArray { count: 1 },
                Instruction::Dup,
                Instruction::Const(5),
                Instruction::Const(2),
                Instruction::SetComputed,
                Instruction::Pop,
                Instruction::Dup,
                Instruction::Const(4),
                Instruction::Const(3),
                Instruction::SetComputed,
                Instruction::Pop,
            ]);
        };
        let probes = [
            (6, 2, 4, "keys"),    // Object.keys(array)[2] == "a".
            (7, 2, 3, "values"),  // Object.values(array)[2] == 3.
            (8, 1, 5, "entries"), // Object.entries(array)[1][0] == "b".
        ];
        for (method, index, expected, label) in probes {
            let mut code = vec![Instruction::LoadBuiltin(kiln_ir::Builtin::Object)];
            array(&mut code);
            code.push(Instruction::CallMethod {
                name: method,
                argc: 1,
            });
            code.push(Instruction::Const(index));
            code.push(Instruction::GetComputed);
            if method == 8 {
                code.push(Instruction::Const(0));
                code.push(Instruction::GetComputed);
                code.push(Instruction::Const(5));
            } else {
                code.push(Instruction::Const(expected));
            }
            code.extend([
                Instruction::Binary(BinaryOp::StrictEqual),
                Instruction::Return,
            ]);
            let generated =
                emit(&program(constants.clone(), code, 0)).expect("array enumeration probe emits");
            let wrapper = format!(
                "int main(void) {{ kiln_value result; if ({}(NULL, &result) != KILN_C99_OK) return 2; return result.tag == KILN_VALUE_BOOLEAN && result.bits ? 0 : 1; }}\n",
                generated.symbol
            );
            let run = run_with_main(&generated, &wrapper, label);
            assert!(
                run.status.success(),
                "array {label} omitted named string properties"
            );
        }

        let mut code = Vec::new();
        array(&mut code);
        code.push(Instruction::EnumerateKeys);
        code.push(Instruction::Const(2));
        code.push(Instruction::GetComputed);
        code.push(Instruction::Const(4));
        code.extend([
            Instruction::Binary(BinaryOp::StrictEqual),
            Instruction::Return,
        ]);
        let generated = emit(&program(constants, code, 0)).expect("for-in probe emits");
        let wrapper = format!(
            "int main(void) {{ kiln_value result; if ({}(NULL, &result) != KILN_C99_OK) return 2; return result.tag == KILN_VALUE_BOOLEAN && result.bits ? 0 : 1; }}\n",
            generated.symbol
        );
        let run = run_with_main(&generated, &wrapper, "array-for-in");
        assert!(
            run.status.success(),
            "for-in omitted named string properties"
        );
    }

    #[test]
    fn executable_inspection_skips_symbol_properties_without_separators() {
        let generated = emit(&program(
            vec![
                Constant::String("iterator".into()),
                Constant::Number(1.0f64.to_bits()),
                Constant::String("a".into()),
                Constant::Number(2.0f64.to_bits()),
            ],
            vec![
                Instruction::LoadBuiltin(kiln_ir::Builtin::Symbol),
                Instruction::GetProperty(0),
                Instruction::Const(1),
                Instruction::Const(2),
                Instruction::Const(3),
                Instruction::NewObject { count: 2 },
                Instruction::HostCall {
                    op: HostOp::WriteStdout,
                    argc: 1,
                },
                Instruction::Return,
            ],
            0,
        ))
        .expect("symbol inspection probe emits");
        let wrapper = format!(
            r#"
static int host_call(void *context, uint16_t operation, uint16_t argc,
                     const kiln_value *arguments, kiln_value *result) {{
    (void)context;
    if (operation != UINT16_C(1) || argc != 1 || arguments == NULL || result == NULL ||
        arguments[0].tag != KILN_VALUE_STRING ||
        !kiln_test_units_equal(arguments[0], "{{ a: 2 }}", 8)) return 1;
    *result = kiln_c99_undefined();
    return 0;
}}
int main(void) {{
    kiln_host host = {{host_call, NULL, NULL}};
    kiln_value result;
    return {symbol}(&host, &result) == KILN_C99_OK ? 0 : 2;
}}
"#,
            symbol = generated.symbol
        );
        let run = run_with_main(&generated, &wrapper, "symbol-inspection");
        assert!(
            run.status.success(),
            "symbol-only properties must not leave malformed inspection separators"
        );
    }
}

// `URL` was a C constructor here and had a test that built one in this host; it is the `url`
// crate through the provider and a class in the runtime bootstrap now (full-runtime/04), and
// neither is linked into this host -- the same reason the `node:path` shim above refuses.
// `conformance/native/url-surface.mjs` and `app-http-client-url.mjs` hold both engines to node.

mod analysis;
mod emission;
mod intrinsics;
mod objects;
mod reference_stores;
mod runtime;
