/* Kiln runtime prelude declarations for decoupled module units. */
#ifndef KILN_PRELUDE_DECLARATIONS_H
#define KILN_PRELUDE_DECLARATIONS_H

typedef struct kiln_data_view kiln_data_view;
#define KILN_GC_PHASE_MARK 0
#define KILN_GC_PHASE_RESET 2
#define KILN_GC_PHASE_LANE_RESET 3
/* Opaque: a unit marks its module globals through the prelude and never looks inside the heap.
 * Declared as the bare struct rather than the typedef so this header cannot collide with the
 * definition in `heap.c` (formerly part of `prelude_runtime.c`; see `runtime/README.md`). */
struct kiln_gc_heap;
extern void kiln_c99_gc_mark_values(struct kiln_gc_heap *heap, const kiln_value *values,
                                    size_t count);
#define KILN_GC_ROOT_SURVIVES_TEARDOWN 1
typedef int (*kiln_native_entry)(void *heap, void *context, kiln_value *result, size_t depth, kiln_value *thrown);
extern void kiln_c99_gc_release(void *ptr);
extern void *kiln_c99_gc_splice_sublist(struct kiln_gc_heap *from_heap, struct kiln_gc_heap *to_heap, void *root_ptr);
extern void *__kiln_transfer_region(void *ptr);
extern void kiln_c99_gc_result_roots(void *opaque, void *context, int phase);
extern char *kiln_c99_gc_alloc_text(size_t len);
extern kiln_val kiln_c99_val_from_text(const char *bytes, size_t len);
extern kiln_val kiln_c99_candidate_literal(const kiln_string *source);
extern kiln_val kiln_c99_val_from_ascii(const char *bytes, size_t len);
extern kiln_val kiln_c99_val_from_unit(uint16_t unit);
extern kiln_val kiln_c99_val_slice_string(const kiln_string *source, size_t start, size_t len);
extern int kiln_c99_split_string_ascii_byte(kiln_value *result, const kiln_string *source,
                                            char separator, size_t limit);
extern int kiln_c99_split_ascii_byte(kiln_value *result, const char *bytes, size_t byte_len,
                                     char separator, size_t limit);
extern const kiln_string *kiln_c99_intern_static(const char *bytes, size_t len);
extern const kiln_string *kiln_c99_intern(const char *bytes, size_t len);
extern const kiln_string *kiln_c99_intern_key(const kiln_string *view);
extern const kiln_string *kiln_c99_intern_static_key(const kiln_string *view);
extern int kiln_c99_register_static_keys(const kiln_string *const *keys, const kiln_string **out,
                                         size_t count);
extern int kiln_c99_unit_keys_claim(uint64_t *owner);
extern void kiln_c99_unit_keys_release(uint64_t *owner, void *heap);
extern const kiln_string *kiln_c99_interned_key_in(struct kiln_gc_heap *heap,
                                                   const kiln_string *view);
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

extern kiln_value kiln_c99_number_bits(uint64_t bits);
extern kiln_value kiln_c99_concat(kiln_value left, kiln_value right);
extern int kiln_c99_define_computed(kiln_value target, kiln_value key, kiln_value value);
extern KILN_COLD void kiln_c99_note_refusal(const char *site, size_t instruction, const char *operation, size_t depth);
extern KILN_COLD void kiln_c99_publish_diagnostic(void);
extern void kiln_c99_gc_unit_roots(void *opaque, void *context, int phase);
extern KILN_COLD int kiln_c99_refuse(const char *lacked);
extern KILN_COLD int kiln_c99_refuse_named(const char *lacked, const char *subject, size_t subject_len);
extern KILN_COLD int kiln_c99_internal_status(const char *converted_by);
static int kiln_c99_json_document(const kiln_string *source, kiln_val *out);

#define KILN_C99_REFUSE(lacked) return kiln_c99_refuse(lacked)
#define KILN_C99_REFUSE_NAMED(lacked, subject, subject_len) \
    return kiln_c99_refuse_named((lacked), (subject), (subject_len))
#define KILN_C99_REFUSE_INTERNAL(converted_by) return kiln_c99_internal_status(converted_by)

/* Emitted variable declarations */
extern int kiln_c99_keys_check_mode;
/* The frame registers are per thread (see `builtins.c`). */
extern _Thread_local kiln_value kiln_c99_new_target;
extern _Thread_local int kiln_c99_inheriting_new_target __attribute__((visibility("hidden")));
extern _Thread_local int kiln_c99_typed_status;
extern _Thread_local kiln_value kiln_c99_typed_thrown;
extern void kiln_c99_typed_overflow(void);
extern _Thread_local kiln_value kiln_c99_adopted_this;
extern _Thread_local const kiln_val *kiln_c99_frame_args;
extern _Thread_local size_t kiln_c99_frame_argc;
extern _Thread_local kiln_value kiln_c99_captured_arguments;
extern _Thread_local int kiln_c99_frame_is_arrow;
extern _Thread_local kiln_value kiln_c99_frame_arguments_object;
extern _Thread_local int kiln_c99_frame_mapped_arguments;
extern _Thread_local kiln_value kiln_c99_frame_callee;
extern const uint16_t kiln_c99_method_codes[];

/* Emitted function prototypes */
extern kiln_value kiln_c99_value(uint32_t tag, uint64_t bits, const char *ptr, size_t len);
extern kiln_value kiln_c99_undefined(void);
extern kiln_object *kiln_c99_object_of(kiln_value value);
extern kiln_array *kiln_c99_array_of(kiln_value value);
extern kiln_typed_array *kiln_c99_typed_array_of(kiln_value value);
extern const kiln_string *kiln_c99_intern(const char *bytes, size_t len);
extern const kiln_string *kiln_c99_intern_static(const char *bytes, size_t len);
extern const kiln_string *kiln_c99_intern_key(const kiln_string *view);
extern const kiln_string *kiln_c99_intern_static_key(const kiln_string *view);
extern int kiln_c99_register_static_keys(const kiln_string *const *keys, const kiln_string **out,
                                         size_t count);
extern int kiln_c99_unit_keys_claim(uint64_t *owner);
extern void kiln_c99_unit_keys_release(uint64_t *owner, void *heap);
extern void kiln_c99_gc_release(void *payload);
extern void kiln_c99_gc_result_roots(void *opaque, void *context, int phase);
extern int kiln_c99_new_object(kiln_value *out);
extern int kiln_c99_new_object_with_capacity(kiln_value *out, size_t capacity);
extern int kiln_c99_new_array(kiln_value *out);
extern const char *kiln_c99_typed_name(kiln_typed_kind kind);
extern int kiln_c99_new_typed_buffer(kiln_value *out, size_t byte_length);
extern int kiln_c99_to_index(kiln_value value, size_t *out, kiln_value *thrown);
extern int kiln_c99_to_number_checked(kiln_host *host, kiln_value value, double *out, int depth, kiln_value *thrown);
extern int kiln_c99_typed_alloc(kiln_value *out, kiln_typed_kind kind, size_t count);
extern int kiln_c99_typed_alloc_ex(kiln_value *out, kiln_typed_kind kind, size_t count, int zeroed);
extern double kiln_c99_typed_get(kiln_typed_array *view, size_t index);
extern void kiln_c99_typed_set(kiln_typed_array *view, size_t index, double value);
extern int kiln_c99_new_typed_array(kiln_value *out, kiln_typed_kind kind, kiln_value source, size_t byte_offset, size_t length, int has_length, kiln_value *thrown);
extern int kiln_c99_reserve(void **items, size_t *capacity, size_t needed, size_t item_size);
extern int kiln_c99_same_name(const char *key, size_t key_len, const char *name);
extern int kiln_c99_put_mode(kiln_host *host, kiln_value target, const char *key, size_t key_len, kiln_value value, int strict, int depth, kiln_value *thrown);
extern int kiln_c99_put_mode_key(kiln_host *host, kiln_value target, const kiln_string *view,
                                 const kiln_string *canonical, kiln_value value, int strict,
                                 int depth, kiln_value *thrown);
extern int kiln_c99_put(kiln_host *host, kiln_value target, const char *key, size_t key_len, kiln_value value, int depth, kiln_value *thrown);
extern uint32_t kiln_c99_shape_slot_named(const kiln_shape *shape, const char *bytes, size_t len);
extern int kiln_c99_define_named_fast(kiln_val target, kiln_val key, kiln_val value, int *status,
                                      kiln_c99_site *site);
extern int kiln_c99_spread_object_fast(kiln_val target, kiln_val source, int *status);
extern int kiln_c99_val_promise_resolve(kiln_val value, kiln_val *out);
/* Whether this process installs kiln's own members (ADR 0068); `parallelMap`'s lowerings ask. */
extern int kiln_c99_kiln_extensions_on(void);
extern int kiln_c99_own_slot(kiln_value target, const char *key, size_t key_len, kiln_property **out);
extern int kiln_c99_own_slot_key(kiln_value target, const kiln_string *key,
                                 const kiln_string *ikey, kiln_property **out);
extern kiln_value kiln_c99_method(uint64_t code);
extern int kiln_c99_commonjs_require_value(kiln_value *out, kiln_commonjs_require_fn call, void *context);
extern int kiln_c99_default_require(void *context, kiln_host *host, kiln_value specifier,
                                    kiln_value *result, kiln_value *thrown);
extern const char *kiln_c99_method_name(uint64_t code);
extern const char *kiln_c99_builtin_name(uint64_t builtin);
extern int kiln_c99_builtin_length(uint64_t builtin, double *out);
extern int kiln_c99_visible_arguments(kiln_value *out);
extern int kiln_c99_atomics_object(kiln_value *out);
extern int kiln_c99_process_object(kiln_value *out);
extern int kiln_c99_url_class(kiln_value *out);
extern int kiln_c99_bootstrap_global(kiln_value *out, const char *name, size_t name_len);
extern int kiln_c99_node_process_raw(kiln_host *host, kiln_value *result, uint64_t code,
                                     const kiln_value *args, size_t argc, int depth,
                                     kiln_value *thrown);
extern int kiln_c99_global_object(kiln_value *out);
extern int kiln_c99_global_binding(kiln_value *out, const char *name, size_t name_len, int *found);
extern int kiln_c99_global_binding_key(kiln_value *out, const kiln_string *name, int *found);
extern int kiln_c99_declare_global(const char *name, size_t name_len, int configurable);
extern int kiln_c99_declare_global_key(const kiln_string *name, int configurable);
extern int kiln_c99_bigint_constant(void **cache, const char *digits, size_t len, kiln_value *out);
extern int kiln_c99_bigint_constant_key(void **cache, const kiln_string *digits, kiln_value *out);
extern int kiln_c99_new_regexp(kiln_value *out, const kiln_string *source, const kiln_string *flags, kiln_value *thrown);
extern int kiln_c99_new_regexp_matcher(kiln_value *out, const kiln_string *source, const kiln_string *flags, int (*matcher)(const uint16_t *units, size_t count, size_t start, int64_t *ranges, size_t capacity, size_t *groups, int *matched), int (*matcher_ascii)(const uint8_t *bytes, size_t count, size_t start, int64_t *ranges, size_t capacity, size_t *groups, int *matched), kiln_value *thrown);
extern void kiln_c99_materialize_match_properties(kiln_array *arr);
extern kiln_value kiln_c99_property_key(kiln_value key);
extern int kiln_c99_new_symbol(kiln_value *out, kiln_value description);
extern int kiln_c99_symbol_for(kiln_value *out, kiln_value key);
extern int kiln_c99_symbol_of_key(const char *key, size_t key_len, kiln_value *out);
extern kiln_value kiln_c99_symbol_key_for(kiln_value value);
extern int kiln_c99_is_namespace(uint64_t builtin);
extern uint64_t kiln_c99_namespace_method(uint64_t builtin, const char *key, size_t key_len);
extern kiln_value kiln_c99_builtin(uint64_t code);
extern int kiln_c99_is_callable(kiln_value value);
extern int kiln_c99_is_reference(kiln_value value);
/* The three hints `ToPrimitive` can be asked for. The specification has three, not two, and the
 * third is not decoration: a `Date` is the one built-in whose `@@toPrimitive` answers a string for
 * "default" and a number for "number", which is the whole of why `date + date` concatenates while
 * `date - date` subtracts. For every other value the three coincide, which is why two of them
 * sufficed here for so long. NUMBER and STRING keep the 0 and 1 the two-valued flag used, so every
 * call site that already said what it wanted still says it. */
#ifndef KILN_C99_HINT_NUMBER
#define KILN_C99_HINT_NUMBER 0
#define KILN_C99_HINT_STRING 1
#define KILN_C99_HINT_DEFAULT 2
#endif

extern int kiln_c99_to_primitive(kiln_host *host, kiln_value value, int hint, kiln_value *out, int depth, kiln_value *thrown);
extern int kiln_c99_reduce_key(kiln_host *host, kiln_value target, kiln_value key, kiln_value *out, int depth, kiln_value *thrown);
extern int kiln_c99_has_intrinsic_text(kiln_value value);
extern int kiln_c99_crosses_boundary(kiln_value value);
extern kiln_value kiln_c99_hole(void);
extern kiln_value kiln_c99_uninitialized(void);
extern kiln_value kiln_c99_null(void);
extern kiln_value kiln_c99_boolean(int value);
extern kiln_closure *kiln_c99_closure_of(kiln_value value);
extern kiln_val *kiln_c99_cell_of(kiln_value value);
extern void kiln_c99_heap_record_pointer(const void *owner_payload, const void *pointer);
static KILN_ALWAYS_INLINE inline void kiln_c99_heap_store_val(
    const void *owner_payload, kiln_val *slot, kiln_val value) {
    if (owner_payload != NULL && !KILN_VAL_IS_NUMBER(value) && value >= KILN_TAG_STRING_PREFIX)
        kiln_c99_heap_record_pointer(owner_payload, KILN_VAL_PTR(value));
    *slot = value;
}
static KILN_ALWAYS_INLINE inline void kiln_c99_heap_store_value(
    const void *owner_payload, kiln_value *slot, kiln_value value) {
    if (owner_payload != NULL && value.tag >= KILN_VALUE_STRING)
        kiln_c99_heap_record_pointer(owner_payload, (const void *)(uintptr_t)value.bits);
    *slot = value;
}
static KILN_ALWAYS_INLINE inline void kiln_c99_val_store_cell(kiln_val *cell, kiln_val value) {
    kiln_c99_heap_store_val(cell, cell, value);
}

extern int kiln_c99_global_object_val(kiln_val *out);
extern int kiln_c99_make_cell(kiln_value *slot);
extern void kiln_c99_mark_uninitialized(kiln_value *slot);
extern int kiln_c99_make_closure(kiln_value *out, kiln_body_fn body, const kiln_string *name, const char *source, size_t source_len, const char *diagnostic_source, uint32_t proto, uint32_t arity, int generator, int asynchronous, int async_generator, int is_method, int arrow, int strict, kiln_val receiver, kiln_val new_target, uint32_t local_count, uint32_t stack_size, uint32_t handler_depth, uint32_t prologue_resume, const kiln_val *captured, uint32_t count);
extern int kiln_c99_region_closure(kiln_value *out, kiln_closure **site, kiln_body_fn body, const kiln_string *name, const char *source, size_t source_len, const char *diagnostic_source, uint32_t proto, uint32_t arity, int generator, int asynchronous, int async_generator, int is_method, int arrow, int strict, kiln_val receiver, uint32_t local_count, uint32_t stack_size, uint32_t handler_depth, uint32_t prologue_resume, int publishes_frame);
extern int kiln_c99_module_completion(kiln_host *host, kiln_value input_promise, uint32_t module_index, kiln_value *out_promise, int depth, kiln_value *thrown);
extern int kiln_c99_finish_module_checkpoint(kiln_host *host, kiln_value *thrown);
extern int kiln_c99_buffer_prototype(kiln_host *host, kiln_value *out, int depth, kiln_value *thrown);
extern int kiln_c99_tail_define_accessor(kiln_val *stack, size_t *sp, int setter_half, int enumerable, const char *site, size_t pc, const char *operation, size_t diagnostic_depth);
extern int kiln_c99_tail_define_method(kiln_val *stack, size_t *sp, const char *site, size_t pc, const char *operation, size_t diagnostic_depth);
extern int kiln_c99_tail_apply(kiln_host *host, kiln_val *stack, size_t *sp, int depth, kiln_value *thrown, const char *site, size_t pc, const char *operation, size_t diagnostic_depth);
extern int kiln_c99_val_new_array_literal(kiln_val *out, const kiln_val *elements, size_t count, int is_numeric);
extern int kiln_c99_val_new_object_literal(kiln_val *out, const kiln_val *pairs, size_t count);
extern int kiln_c99_invoke_closure_body(kiln_host *host, kiln_val *result, kiln_closure *closure, kiln_val receiver, const kiln_val *args, size_t argc, int depth, kiln_value *thrown, kiln_generator *frame, int inherit_new_target);
extern int kiln_c99_invoke_closure(kiln_host *host, kiln_val *result, kiln_closure *closure, kiln_val receiver, const kiln_val *args, size_t argc, int depth, kiln_value *thrown, kiln_generator *frame, int inherit_new_target);
extern int kiln_c99_call(kiln_host *host, kiln_value *result, kiln_value callee, kiln_value receiver, const kiln_value *args, size_t argc, int depth, kiln_value *thrown);
extern kiln_value kiln_c99_number(double value);
extern kiln_value kiln_c99_number_bits(uint64_t bits);
extern double kiln_c99_as_number(kiln_value value);
extern int kiln_c99_truthy(kiln_value value);
extern kiln_value kiln_c99_copy_text(const char *text, size_t len);
extern kiln_val kiln_c99_val_from_text(const char *bytes, size_t len);
extern int kiln_c99_new_array_with_capacity(kiln_value *out, size_t capacity, int is_numeric);
extern KILN_COLD kiln_value kiln_c99_control_diagnostic(void);
extern int kiln_c99_diagnostic_available(void);
extern void kiln_c99_forget_handled_throw(void);
extern KILN_COLD void kiln_c99_note_refusal(const char *site, size_t instruction, const char *operation, size_t depth);
extern kiln_value kiln_c99_text(kiln_value value);
extern kiln_value kiln_c99_concat(kiln_value left, kiln_value right);
extern int kiln_c99_strict_equal(kiln_value left, kiln_value right);
extern int kiln_c99_callable_prototype(kiln_value callable, kiln_value *out);
extern int kiln_c99_get_array_unscopables(kiln_value *out);
extern int kiln_c99_get_with_receiver(kiln_host *host, kiln_value target, kiln_value key, kiln_value receiver, kiln_value *out, int depth, kiln_value *thrown);
extern int kiln_c99_set_with_receiver(kiln_host *host, kiln_value target, kiln_value key, kiln_value value, kiln_value receiver, int strict, int depth, kiln_value *thrown);
extern int kiln_c99_has_computed(kiln_host *host, kiln_value target, kiln_value key, int *answer, int depth, kiln_value *thrown);
extern int kiln_c99_delete_computed(kiln_host *host, kiln_value target, kiln_value key, int *answer, int depth, kiln_value *thrown);
extern int kiln_c99_get(kiln_host *host, kiln_value target, const char *key, size_t key_len, kiln_value *out, int depth, kiln_value *thrown);
extern int kiln_c99_get_site(kiln_host *host, kiln_value target, const kiln_string *view,
                             const kiln_string *canonical, kiln_value *out, int depth,
                             kiln_value *thrown, kiln_c99_site *site);
extern int kiln_c99_put_site_key(kiln_host *host, kiln_value target, const kiln_string *view,
                                 const kiln_string *canonical, kiln_value value, int strict,
                                 int depth, kiln_value *thrown, kiln_c99_site *site);
extern int kiln_c99_get_computed(kiln_host *host, kiln_value target, kiln_value key, kiln_value *out, int depth, kiln_value *thrown);
extern int kiln_c99_set_computed_mode(kiln_host *host, kiln_value target, kiln_value key, kiln_value value, int strict, int depth, kiln_value *thrown);
extern int kiln_c99_write_would_accept(kiln_value target, const char *key, size_t key_len);
extern int kiln_c99_set_computed(kiln_host *host, kiln_value target, kiln_value key, kiln_value value, int depth, kiln_value *thrown);
extern int kiln_c99_define_computed(kiln_value target, kiln_value key, kiln_value value);
extern int kiln_c99_array_push(kiln_value target, kiln_value value);
extern int kiln_c99_array_push_val(kiln_value target, kiln_val value);
extern kiln_value kiln_c99_inspect(kiln_value value);
extern kiln_value kiln_c99_console_inspect(kiln_host *host, kiln_value value);
extern int kiln_c99_node_http_event(void *opaque, const kiln_realm_http_event *event, void *host, void *thrown);
extern int kiln_c99_invoke_method(kiln_host *host, kiln_value *result, uint64_t code, kiln_value receiver, const kiln_value *args, size_t argc, int depth, kiln_value *thrown);
extern int kiln_c99_iterate(kiln_host *host, kiln_value target, kiln_value source, int depth, kiln_value *thrown);
extern void kiln_c99_typed_own(kiln_value target, const char *key, size_t key_len, int *found, int *enumerable);
extern int kiln_c99_delete(kiln_host *host, kiln_value target, const char *key, size_t key_len, int *answer, int depth, kiln_value *thrown);
extern int kiln_c99_delete_key(kiln_host *host, kiln_value target, const kiln_string *view,
                               const kiln_string *canonical, int *answer, int depth,
                               kiln_value *thrown);
extern int kiln_c99_delete_or_throw(kiln_host *host, kiln_value target, const char *key, size_t key_len, int strict, int *answer, int depth, kiln_value *thrown);
extern int kiln_c99_delete_or_throw_key(kiln_host *host, kiln_value target, const kiln_string *key,
                                        int strict, int *answer, int depth,
                                        kiln_value *thrown);
extern int kiln_c99_delete_computed_or_throw(kiln_host *host, kiln_value target, kiln_value key, int strict, int *answer, int depth, kiln_value *thrown);
extern int kiln_c99_own_keys(kiln_host *host, kiln_value target, kiln_value *out, int depth, kiln_value *thrown);
extern int kiln_c99_enumerate_keys(kiln_host *host, kiln_value target, kiln_value *out, int depth, kiln_value *thrown);
extern int kiln_c99_validate_dynamic_import_options(kiln_host *host, kiln_value options, int depth, kiln_value *thrown, int *attribute_type);
extern int kiln_c99_spread_object(kiln_host *host, kiln_value target, kiln_value source, int depth, kiln_value *thrown);
extern int kiln_c99_define_accessor(kiln_value target, const char *key, size_t key_len, kiln_value getter, kiln_value setter, int enumerable);
extern int kiln_c99_define_accessor_key(kiln_value target, const kiln_string *key,
                                        kiln_value getter, kiln_value setter, int enumerable);
extern int kiln_c99_describe_property(kiln_host *host, kiln_value target, const char *key, size_t key_len, kiln_value *out, int *found, int depth, kiln_value *thrown);
extern int kiln_c99_set_function_name(kiln_value function, kiln_value key, const char *prefix);
extern int kiln_c99_define_private(kiln_value target, const char *key, size_t key_len, kiln_value function, int kind);
extern int kiln_c99_define_private_key(kiln_value target, const kiln_string *key,
                                       kiln_value function, int kind);
extern int kiln_c99_define_method(kiln_value target, const char *key, size_t key_len, kiln_value method);
extern int kiln_c99_define_method_key(kiln_value target, const kiln_string *key, kiln_value method);
extern int kiln_c99_new_typed_iterator(kiln_value *out, kiln_value source, int part);
extern int kiln_c99_iterator_method(kiln_host *host, kiln_value *result, uint64_t code, kiln_value receiver, const kiln_value *args, size_t argc, int depth, kiln_value *thrown);
extern kiln_property *kiln_c99_own_private(kiln_value target, const char *key, size_t key_len);
extern kiln_property *kiln_c99_own_private_key(kiln_value target, const kiln_string *key);
extern int kiln_c99_private_error(kiln_value *thrown, const char *key, size_t key_len, int mode);
extern int kiln_c99_private_error_key(kiln_value *thrown, const kiln_string *key, int mode);
extern int kiln_c99_throw_error(kiln_value *thrown, uint64_t kind, const char *message);
extern int kiln_c99_new_type_error(kiln_value *out, const char *message);
extern kiln_promise *kiln_c99_promise_of(kiln_value value);
extern int kiln_c99_bind_with(kiln_value *out, kiln_value target, kiln_value receiver, const kiln_value *arguments, size_t count);
extern int kiln_c99_construct(kiln_host *host, kiln_value *result, kiln_value callee, const kiln_value *args, size_t argc, int depth, kiln_value *thrown);
extern int kiln_c99_construct_val(kiln_host *host, kiln_val *result, kiln_val callee, const kiln_val *args, size_t argc, int depth, kiln_value *thrown);
extern int kiln_c99_call_val(kiln_host *host, kiln_val *result, kiln_val callee, kiln_val receiver, const kiln_val *args, size_t argc, int depth, kiln_value *thrown);
extern int kiln_c99_super_call(kiln_host *host, kiln_value *produced, kiln_value parent, kiln_value receiver, const kiln_value *args, size_t argc, int depth, kiln_value *thrown);
extern int kiln_c99_adopt_super(kiln_value *receiver, kiln_value produced);
extern int kiln_c99_super_call_val(kiln_host *host, kiln_val *produced, kiln_val parent, kiln_val receiver, const kiln_val *args, size_t argc, int depth, kiln_value *thrown);
extern int kiln_c99_adopt_super_val(kiln_val *receiver, kiln_val produced);
extern int kiln_c99_set_prototype(kiln_value target, kiln_value prototype);
extern int kiln_c99_set_prototype_checked(kiln_value target, kiln_value prototype, kiln_value *thrown);
extern int kiln_c99_instance_of(kiln_value value, kiln_value constructor, int *answer);
extern kiln_value kiln_c99_typeof(kiln_value value);
extern kiln_value kiln_c99_unary(uint16_t operation, kiln_value value);
extern kiln_value kiln_c99_binary(uint16_t operation, kiln_value left, kiln_value right);
extern int kiln_c99_unary_checked(uint16_t operation, kiln_host *host, kiln_value value, kiln_value *out, int depth, kiln_value *thrown);
extern int kiln_c99_binary_checked(uint16_t operation, kiln_host *host, kiln_value left, kiln_value right, kiln_value *out, int depth, kiln_value *thrown);
extern void kiln_c99_gc_unit_roots(void *opaque, void *context, int phase);
extern uint64_t kiln_c99_array_method(const char *key, size_t key_len);
extern uint64_t kiln_c99_string_method(const char *key, size_t key_len);
extern int kiln_c99_read_property(kiln_host *host, kiln_value target, const char *key, size_t key_len, int depth, kiln_value *thrown, kiln_c99_site *site, kiln_value *out);
extern int kiln_c99_write_property(kiln_host *host, kiln_value target, const char *key, size_t key_len, kiln_value value, int strict, int depth, kiln_value *thrown, kiln_c99_site *site);
extern KILN_COLD void kiln_c99_note_get_property_refusal(kiln_value target);
extern KILN_COLD void kiln_c99_note_get_computed_refusal(kiln_value target, kiln_value key);
extern int kiln_c99_read_element(kiln_host *host, kiln_value target, kiln_value key, int depth, kiln_value *thrown, kiln_value *out);
extern int kiln_c99_write_element(kiln_host *host, kiln_value target, kiln_value key, kiln_value value, int strict, int depth, kiln_value *thrown);
extern uint32_t kiln_c99_gc_kind_of_ptr(const void *ptr);
extern kiln_value kiln_val_to_boundary(kiln_val v);
extern kiln_value kiln_val_binding_to_boundary(kiln_val v);

#ifndef KILN_C99_REALM_GENERATION_DEFINED
#define KILN_C99_REALM_GENERATION_DEFINED
static inline uint64_t kiln_c99_realm_generation(void) {
    static _Thread_local const uint64_t *slot = NULL;
    if (KILN_UNLIKELY(slot == NULL)) slot = kiln_realm_generation_slot();
    return slot != NULL ? *slot : 0;
}
#endif

/* A unit's realm block (see `kiln_c99_unit` in `prelude_types.h`). */
extern kiln_val *kiln_c99_unit_vals_slow(kiln_c99_unit *unit, kiln_c99_unit_cache *cache);
#ifndef KILN_C99_UNIT_VALS_DEFINED
#define KILN_C99_UNIT_VALS_DEFINED
static inline kiln_val *kiln_c99_unit_vals(kiln_c99_unit *unit, kiln_c99_unit_cache *cache) {
    const uint64_t *slot = cache->generation_slot;
    if (KILN_LIKELY(slot != NULL && *slot == cache->generation)) return cache->vals;
    return kiln_c99_unit_vals_slow(unit, cache);
}
#endif

#ifndef KILN_C99_CACHED_METHOD_MATCHES_DEFINED
#define KILN_C99_CACHED_METHOD_MATCHES_DEFINED
static inline int kiln_c99_cached_method_matches(const kiln_c99_site *site, kiln_val target) {
    if (site == NULL || !kiln_c99_site_has_method(site)) return 0;
    uint8_t ctag = site->cached_tag;
    /* Issue 30: a kind's remembered method is its table's, which a member the program wrote onto
     * the kind's prototype shadows. One compare while no realm has written onto a built-in. */
    if (ctag != KILN_VALUE_BUILTIN && KILN_UNLIKELY(!kiln_c99_tag_method_pristine(ctag, site->cached_method))) return 0;
    if (ctag == KILN_VALUE_COLLECTION && (KILN_VAL_IS_OBJECT(target) || ((target & KILN_TAG_MASK) == KILN_TAG_BUFFER_PREFIX))) {
        void *ptr = KILN_VAL_PTR(target);
        return kiln_c99_gc_kind_of_ptr(ptr) == KILN_GC_COLLECTION;
    } else if (ctag == KILN_VALUE_STRING && KILN_VAL_IS_STRING(target)) {
        return 1;
    } else if (ctag == KILN_VALUE_NUMBER && KILN_VAL_IS_NUMBER(target)) {
        return 1;
    } else if (ctag == KILN_VALUE_BOOLEAN && KILN_VAL_IS_BOOLEAN(target)) {
        return 1;
    } else if (ctag == KILN_VALUE_BUILTIN) {
        return KILN_VAL_IS_BUILTIN(target) && KILN_VAL_BUILTIN_ID(target) == site->cached_builtin &&
               (KILN_LIKELY(kiln_c99_builtin_store_epoch == 0) ||
                !kiln_c99_realm_builtin_modified(site->cached_builtin));
    } else if (ctag == KILN_VALUE_ARRAY && ((target & KILN_TAG_MASK) == KILN_TAG_BUFFER_PREFIX)) {
        void *ptr = KILN_VAL_PTR(target);
        if (kiln_c99_gc_kind_of_ptr(ptr) == KILN_GC_ARRAY) {
            const kiln_array *arr = (const kiln_array *)ptr;
            return arr->properties.tag == KILN_VALUE_UNDEFINED;
        }
    } else if (ctag == KILN_VALUE_TYPED_ARRAY && ((target & KILN_TAG_MASK) == KILN_TAG_BUFFER_PREFIX)) {
        void *ptr = KILN_VAL_PTR(target);
        return kiln_c99_gc_kind_of_ptr(ptr) == KILN_GC_TYPED_ARRAY;
    } else if (ctag == KILN_VALUE_REGEXP && (KILN_VAL_IS_OBJECT(target) || ((target & KILN_TAG_MASK) == KILN_TAG_BUFFER_PREFIX))) {
        void *ptr = KILN_VAL_PTR(target);
        return kiln_c99_gc_kind_of_ptr(ptr) == KILN_GC_REGEXP;
    }
    return 0;
}
#endif
extern int kiln_c99_val_compare(uint16_t operation, kiln_val left, kiln_val right, int *result);
extern kiln_val kiln_c99_val_binary(uint16_t operation, kiln_host *host, kiln_val left, kiln_val right, int *status, int depth, kiln_value *thrown);
extern kiln_val kiln_c99_val_binary_fast(uint16_t operation, kiln_val left, kiln_val right);
extern kiln_val kiln_c99_val_unary_fast(uint16_t operation, kiln_val value);
extern int kiln_c99_val_read_property(kiln_host *host, kiln_val target, const char *key, size_t key_len, int depth, kiln_value *thrown, kiln_c99_site *site, kiln_val *out);
extern int kiln_c99_val_write_property(kiln_host *host, kiln_val target, const char *key, size_t key_len, kiln_val value, int strict, int depth, kiln_value *thrown, kiln_c99_site *site);
extern int kiln_c99_val_read_property_key(kiln_host *host, kiln_val target, const kiln_string *key, int depth, kiln_value *thrown, kiln_c99_site *site, kiln_val *out);
extern int kiln_c99_val_write_property_key(kiln_host *host, kiln_val target, const kiln_string *key, kiln_val value, int strict, int depth, kiln_value *thrown, kiln_c99_site *site);
#ifndef KILN_C99_VAL_PRIVATE_CACHED_DEFINED
#define KILN_C99_VAL_PRIVATE_CACHED_DEFINED
static inline int kiln_c99_val_get_private_cached(kiln_val *out, kiln_val receiver, kiln_val brand,
                                                 const kiln_c99_private_site *site) {
    if (KILN_LIKELY(site != NULL && KILN_VAL_IS_OBJECT(receiver))) {
        kiln_object *obj = (kiln_object *)KILN_VAL_PTR(receiver);
        if (KILN_LIKELY(obj != NULL && obj->shape != NULL && obj->shape == kiln_c99_slot_key(&site->shape) &&
                        kiln_c99_gc_kind_of_ptr(obj) == KILN_GC_OBJECT)) {
            if (brand == KILN_VAL_UNDEFINED ||
                (site->brand_slot < obj->slot_capacity && obj->slots[site->brand_slot] == brand)) {
                if (KILN_LIKELY(site->member_slot < obj->slot_capacity)) {
                    *out = obj->slots[site->member_slot];
                    return 1;
                }
            }
        }
    }
    return 0;
}

static inline int kiln_c99_val_set_private_cached(kiln_val receiver, kiln_val brand, kiln_val member,
                                                 const kiln_c99_private_site *site) {
    if (KILN_LIKELY(site != NULL && KILN_VAL_IS_OBJECT(receiver))) {
        kiln_object *obj = (kiln_object *)KILN_VAL_PTR(receiver);
        if (KILN_LIKELY(obj != NULL && obj->shape != NULL && obj->shape == kiln_c99_slot_key(&site->shape) &&
                        !obj->frozen && kiln_c99_gc_kind_of_ptr(obj) == KILN_GC_OBJECT)) {
            if (brand == KILN_VAL_UNDEFINED ||
                (site->brand_slot < obj->slot_capacity && obj->slots[site->brand_slot] == brand)) {
                if (KILN_LIKELY(site->member_slot < obj->slot_capacity)) {
                    if ((obj->shape->double_slots_mask & (1u << site->member_slot)) == 0 || KILN_VAL_IS_NUMBER(member)) {
                        kiln_c99_prototype_touched(obj);
                        kiln_c99_heap_store_val(obj, &obj->slots[site->member_slot], member);
                        return 1;
                    }
                }
            }
        }
    }
    return 0;
}
#endif
extern int kiln_c99_val_get_private_fast(kiln_val *out, kiln_val receiver, kiln_val brand,
                                         const char *brand_key, size_t brand_len,
                                         const char *member_key, size_t member_len,
                                         kiln_c99_private_site *site);
extern int kiln_c99_val_set_private_fast(kiln_val receiver, kiln_val brand, kiln_val member,
                                         const char *brand_key, size_t brand_len,
                                         const char *member_key, size_t member_len,
                                         kiln_c99_private_site *site);
extern int kiln_c99_val_read_element(kiln_host *host, kiln_val target, kiln_val key, int depth, kiln_value *thrown, kiln_val *out);
extern int kiln_c99_val_write_element(kiln_host *host, kiln_val target, kiln_val key, kiln_val value, int strict, int depth, kiln_value *thrown);
extern int kiln_c99_val_read_symbol(kiln_host *host, kiln_val target, kiln_val key, int depth, kiln_value *thrown, kiln_c99_symbol_cell *cell, kiln_val *out);
extern int kiln_c99_val_write_symbol(kiln_host *host, kiln_val target, kiln_val key, kiln_val value, int strict, int depth, kiln_value *thrown, kiln_c99_symbol_cell *cell);
extern int kiln_c99_val_call_method(kiln_host *host, kiln_val receiver, const kiln_val *args, size_t argc, const char *key, size_t key_len, int depth, kiln_value *thrown, kiln_c99_site *site, const char *inst_site, size_t inst_idx, const char *inst_op, kiln_val *out);
extern int kiln_c99_val_call_method_key(kiln_host *host, kiln_val receiver, const kiln_val *args, size_t argc, const kiln_string *key, int depth, kiln_value *thrown, kiln_c99_site *site, const char *inst_site, size_t inst_idx, const char *inst_op, kiln_val *out);
extern int kiln_c99_val_invoke_cached_method(kiln_host *host, uint64_t method_code, kiln_val receiver, const kiln_val *args, size_t argc, int depth, kiln_value *thrown, kiln_val *out);
extern int kiln_c99_val_call(kiln_host *host, kiln_val callee, const kiln_val *args, size_t argc, int depth, kiln_value *thrown, const char *inst_site, size_t inst_idx, const char *inst_op, kiln_val *out);
extern int kiln_c99_val_new(kiln_host *host, kiln_val callee, const kiln_val *args, size_t argc, int depth, kiln_value *thrown, kiln_val *out);
extern int kiln_c99_val_to_property_key(kiln_host *host, kiln_val target, kiln_val key, int depth, kiln_value *thrown, kiln_val *out);
extern int kiln_c99_val_check_tdz(kiln_val val, kiln_value *thrown);
extern int kiln_c99_val_make_cell(kiln_val *slot);
extern void kiln_c99_val_make_unit_cell(kiln_val *slot, kiln_val *storage);
extern kiln_val kiln_c99_val_from_double(double d);
extern size_t kiln_c99_val_string_length(kiln_val v);
extern int kiln_c99_val_array_push_fast(kiln_val receiver, kiln_val arg, kiln_val *out);
extern int kiln_c99_val_regexp_test_fast(kiln_val receiver, kiln_val arg, kiln_val *out);
extern int kiln_c99_val_regexp_exec_fast(kiln_val receiver, kiln_val arg, kiln_val *out);
extern kiln_collection *kiln_c99_collection_alloc_fast(int is_map, int is_weak);
extern int kiln_c99_collection_put_val_fast(kiln_collection *collection, kiln_val key, kiln_val value);
extern kiln_val kiln_c99_collection_get_val_fast(kiln_collection *collection, kiln_val key);
extern kiln_val kiln_c99_collection_has_val_fast(kiln_collection *collection, kiln_val key);
extern kiln_val kiln_c99_collection_delete_val_fast(kiln_collection *collection, kiln_val key);
extern int kiln_c99_val_json_stringify_fast(kiln_host *host, kiln_val val, kiln_val *out, int depth, kiln_value *thrown);
extern int kiln_c99_val_json_parse_fast(kiln_host *host, kiln_val val, kiln_val *out, int depth, kiln_value *thrown);
extern int kiln_c99_val_array_sort_fast(kiln_host *host, kiln_array *array, kiln_val comparator, int depth, kiln_value *thrown);
extern int kiln_c99_val_iterator_next_fast(kiln_host *host, kiln_val receiver, kiln_val *out, int depth, kiln_value *thrown);
extern int kiln_c99_val_same_value_zero(kiln_val a, kiln_val b);
extern kiln_val kiln_c99_string_constant(void **cached, const char *bytes, size_t len);
extern void kiln_c99_gc_mark_vals(struct kiln_gc_heap *heap, const kiln_val *values, size_t count);

#endif /* KILN_PRELUDE_DECLARATIONS_H */
