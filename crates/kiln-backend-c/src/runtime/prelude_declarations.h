/* Kiln runtime prelude declarations for decoupled module units. */
#ifndef KILN_PRELUDE_DECLARATIONS_H
#define KILN_PRELUDE_DECLARATIONS_H

typedef struct kiln_data_view kiln_data_view;
#define KILN_GC_PHASE_RESET 2
#define KILN_GC_ROOT_SURVIVES_TEARDOWN 1
typedef int (*kiln_native_entry)(void *heap, void *context, kiln_value *result, size_t depth, kiln_value *thrown);
extern void kiln_c99_gc_release(void *ptr);
extern void kiln_c99_gc_result_roots(void *opaque, void *context, int phase);
extern const char *kiln_c99_intern_static(const char *bytes, size_t len);
extern const char *kiln_c99_intern(const char *bytes, size_t len);
extern kiln_value kiln_c99_number_bits(uint64_t bits);
extern kiln_value kiln_c99_concat(kiln_value left, kiln_value right);
extern int kiln_c99_define_computed(kiln_value target, kiln_value key, kiln_value value);
extern void kiln_c99_note_refusal(const char *site, size_t instruction, const char *operation, size_t depth);
extern void kiln_c99_gc_unit_roots(void *opaque, void *context, int phase);
extern int kiln_c99_refuse(const char *lacked);
extern int kiln_c99_refuse_named(const char *lacked, const char *subject, size_t subject_len);
extern int kiln_c99_internal_status(const char *converted_by);

#define KILN_C99_REFUSE(lacked) return kiln_c99_refuse(lacked)
#define KILN_C99_REFUSE_NAMED(lacked, subject, subject_len) \
    return kiln_c99_refuse_named((lacked), (subject), (subject_len))
#define KILN_C99_REFUSE_INTERNAL(converted_by) return kiln_c99_internal_status(converted_by)

/* Emitted variable declarations */
extern int kiln_c99_keys_check_mode;
extern kiln_value kiln_c99_new_target;
extern int kiln_c99_inheriting_new_target;
extern kiln_value kiln_c99_adopted_this;
extern const kiln_value *kiln_c99_frame_args;
extern size_t kiln_c99_frame_argc;
extern kiln_value kiln_c99_captured_arguments;
extern int kiln_c99_frame_is_arrow;
extern kiln_value kiln_c99_frame_arguments_object;
extern int kiln_c99_frame_mapped_arguments;
extern kiln_value kiln_c99_frame_callee;
extern kiln_value *kiln_c99_symbols;
extern size_t kiln_c99_symbol_count;
extern size_t kiln_c99_symbol_capacity;
extern const uint16_t kiln_c99_method_codes[];

/* Emitted function prototypes */
extern kiln_value kiln_c99_value(uint32_t tag, uint64_t bits, const char *ptr, size_t len);
extern kiln_value kiln_c99_undefined(void);
extern kiln_object *kiln_c99_object_of(kiln_value value);
extern kiln_array *kiln_c99_array_of(kiln_value value);
extern kiln_typed_array *kiln_c99_typed_array_of(kiln_value value);
extern const char *kiln_c99_intern(const char *bytes, size_t len);
extern const char *kiln_c99_intern_static(const char *bytes, size_t len);
extern void kiln_c99_gc_release(void *payload);
extern void kiln_c99_gc_result_roots(void *opaque, void *context, int phase);
extern int kiln_c99_new_object(kiln_value *out);
extern int kiln_c99_new_object_with_capacity(kiln_value *out, size_t capacity);
extern int kiln_c99_node_dns_lookup(kiln_host *host, kiln_value *out, const kiln_value *args, size_t argc, int depth, kiln_value *thrown);
extern int kiln_c99_node_diagnostics(kiln_host *host, kiln_value *out, uint64_t code, const kiln_value *args, size_t argc, int depth, kiln_value *thrown);
extern int kiln_c99_node_util(kiln_host *host, kiln_value *out, uint64_t code, const kiln_value *args, size_t argc, int depth, kiln_value *thrown);
extern int kiln_c99_new_array(kiln_value *out);
extern const char *kiln_c99_typed_name(kiln_typed_kind kind);
extern int kiln_c99_new_typed_buffer(kiln_value *out, size_t byte_length);
extern int kiln_c99_to_index(kiln_value value, size_t *out, kiln_value *thrown);
extern double kiln_c99_typed_get(kiln_typed_array *view, size_t index);
extern void kiln_c99_typed_set(kiln_typed_array *view, size_t index, double value);
extern int kiln_c99_new_typed_array(kiln_value *out, kiln_typed_kind kind, kiln_value source, size_t byte_offset, size_t length, int has_length, kiln_value *thrown);
extern int kiln_c99_reserve(void **items, size_t *capacity, size_t needed, size_t item_size);
extern int kiln_c99_same_name(const char *key, size_t key_len, const char *name);
extern int kiln_c99_put_mode(kiln_host *host, kiln_value target, const char *key, size_t key_len, kiln_value value, int strict, int depth, kiln_value *thrown);
extern int kiln_c99_put(kiln_host *host, kiln_value target, const char *key, size_t key_len, kiln_value value, int depth, kiln_value *thrown);
extern int kiln_c99_own_slot(kiln_value target, const char *key, size_t key_len, kiln_property **out);
extern kiln_value kiln_c99_method(uint64_t code);
extern int kiln_c99_commonjs_require_value(kiln_value *out, kiln_commonjs_require_fn call, void *context);
extern const char *kiln_c99_method_name(uint64_t code);
extern const char *kiln_c99_builtin_name(uint64_t builtin);
extern int kiln_c99_visible_arguments(kiln_value *out);
extern int kiln_c99_atomics_object(kiln_value *out);
extern int kiln_c99_process_object(kiln_value *out);
extern int kiln_c99_global_object(kiln_value *out);
extern int kiln_c99_global_binding(kiln_value *out, const char *name, size_t name_len, int *found);
extern int kiln_c99_declare_global(const char *name, size_t name_len, int configurable);
extern int kiln_c99_bigint_constant(void **cache, const char *digits, size_t len, kiln_value *out);
extern int kiln_c99_new_regexp(kiln_value *out, const char *source, size_t source_len, const char *flags, size_t flags_len, kiln_value *thrown);
extern kiln_value kiln_c99_property_key(kiln_value key);
extern int kiln_c99_new_symbol(kiln_value *out, kiln_value description);
extern int kiln_c99_symbol_for(kiln_value *out, kiln_value key);
extern kiln_value kiln_c99_symbol_key_for(kiln_value value);
extern int kiln_c99_is_namespace(uint64_t builtin);
extern uint64_t kiln_c99_namespace_method(uint64_t builtin, const char *key, size_t key_len);
extern kiln_value kiln_c99_builtin(uint64_t code);
extern int kiln_c99_is_callable(kiln_value value);
extern int kiln_c99_is_reference(kiln_value value);
extern int kiln_c99_has_intrinsic_text(kiln_value value);
extern int kiln_c99_crosses_boundary(kiln_value value);
extern kiln_value kiln_c99_hole(void);
extern kiln_value kiln_c99_uninitialized(void);
extern kiln_value kiln_c99_null(void);
extern kiln_value kiln_c99_boolean(int value);
extern kiln_closure *kiln_c99_closure_of(kiln_value value);
extern kiln_value *kiln_c99_cell_of(kiln_value value);
extern int kiln_c99_make_cell(kiln_value *slot);
extern void kiln_c99_mark_uninitialized(kiln_value *slot);
extern int kiln_c99_make_closure(kiln_value *out, kiln_body_fn body, const char *name, size_t name_len, const char *source, size_t source_len, const char *diagnostic_source, uint32_t proto, uint32_t arity, int generator, int asynchronous, int async_generator, int arrow, int strict, kiln_value receiver, kiln_value new_target, uint32_t local_count, uint32_t stack_size, uint32_t handler_depth, uint32_t prologue_resume, const kiln_value *captured, uint32_t count);
extern int kiln_c99_invoke_closure_body(kiln_host *host, kiln_value *result, kiln_closure *closure, kiln_value receiver, const kiln_value *args, size_t argc, int depth, kiln_value *thrown, kiln_generator *frame, int inherit_new_target);
extern int kiln_c99_invoke_closure(kiln_host *host, kiln_value *result, kiln_closure *closure, kiln_value receiver, const kiln_value *args, size_t argc, int depth, kiln_value *thrown, kiln_generator *frame, int inherit_new_target);
extern int kiln_c99_call(kiln_host *host, kiln_value *result, kiln_value callee, kiln_value receiver, const kiln_value *args, size_t argc, int depth, kiln_value *thrown);
extern kiln_value kiln_c99_number(double value);
extern kiln_value kiln_c99_number_bits(uint64_t bits);
extern double kiln_c99_as_number(kiln_value value);
extern int kiln_c99_truthy(kiln_value value);
extern kiln_value kiln_c99_copy_text(const char *text, size_t len);
extern kiln_value kiln_c99_control_diagnostic(void);
extern void kiln_c99_forget_handled_throw(void);
extern void kiln_c99_note_refusal(const char *site, size_t instruction, const char *operation, size_t depth);
extern kiln_value kiln_c99_text(kiln_value value);
extern kiln_value kiln_c99_concat(kiln_value left, kiln_value right);
extern int kiln_c99_strict_equal(kiln_value left, kiln_value right);
extern int kiln_c99_callable_prototype(kiln_value callable, kiln_value *out);
extern int kiln_c99_get_with_receiver(kiln_host *host, kiln_value target, kiln_value key, kiln_value receiver, kiln_value *out, int depth, kiln_value *thrown);
extern int kiln_c99_has_computed(kiln_host *host, kiln_value target, kiln_value key, int *answer, int depth, kiln_value *thrown);
extern int kiln_c99_delete_computed(kiln_host *host, kiln_value target, kiln_value key, int *answer, int depth, kiln_value *thrown);
extern int kiln_c99_get(kiln_host *host, kiln_value target, const char *key, size_t key_len, kiln_value *out, int depth, kiln_value *thrown);
extern int kiln_c99_get_site(kiln_host *host, kiln_value target, const char *key, size_t key_len, kiln_value *out, int depth, kiln_value *thrown, kiln_c99_site *site);
extern int kiln_c99_put_site(kiln_host *host, kiln_value target, const char *key, size_t key_len, kiln_value value, int strict, int depth, kiln_value *thrown, kiln_c99_site *site);
extern int kiln_c99_get_computed(kiln_host *host, kiln_value target, kiln_value key, kiln_value *out, int depth, kiln_value *thrown);
extern int kiln_c99_set_computed_mode(kiln_host *host, kiln_value target, kiln_value key, kiln_value value, int strict, int depth, kiln_value *thrown);
extern int kiln_c99_write_would_accept(kiln_value target, const char *key, size_t key_len);
extern int kiln_c99_set_computed(kiln_host *host, kiln_value target, kiln_value key, kiln_value value, int depth, kiln_value *thrown);
extern int kiln_c99_define_computed(kiln_value target, kiln_value key, kiln_value value);
extern int kiln_c99_array_push(kiln_value target, kiln_value value);
extern kiln_value kiln_c99_inspect(kiln_value value);
extern int kiln_c99_node_http_event(void *opaque, const kiln_realm_http_event *event, void *host, void *thrown);
extern int kiln_c99_invoke_method(kiln_host *host, kiln_value *result, uint64_t code, kiln_value receiver, const kiln_value *args, size_t argc, int depth, kiln_value *thrown);
extern int kiln_c99_collection_method_set(kiln_value receiver, kiln_value key, kiln_value val, kiln_value *result);
extern int kiln_c99_collection_method_get(kiln_value receiver, kiln_value key, kiln_value *result);
extern int kiln_c99_collection_method_has(kiln_value receiver, kiln_value key, kiln_value *result);
extern int kiln_c99_collection_method_add(kiln_value receiver, kiln_value key, kiln_value *result);
extern int kiln_c99_iterate(kiln_host *host, kiln_value target, kiln_value source, int depth, kiln_value *thrown);
extern void kiln_c99_typed_own(kiln_value target, const char *key, size_t key_len, int *found, int *enumerable);
extern int kiln_c99_delete(kiln_host *host, kiln_value target, const char *key, size_t key_len, int *answer, int depth, kiln_value *thrown);
extern int kiln_c99_delete_or_throw(kiln_host *host, kiln_value target, const char *key, size_t key_len, int strict, int *answer, int depth, kiln_value *thrown);
extern int kiln_c99_delete_computed_or_throw(kiln_host *host, kiln_value target, kiln_value key, int strict, int *answer, int depth, kiln_value *thrown);
extern int kiln_c99_own_keys(kiln_host *host, kiln_value target, kiln_value *out, int depth, kiln_value *thrown);
extern int kiln_c99_enumerate_keys(kiln_host *host, kiln_value target, kiln_value *out, int depth, kiln_value *thrown);
extern int kiln_c99_spread_object(kiln_host *host, kiln_value target, kiln_value source, int depth, kiln_value *thrown);
extern int kiln_c99_define_accessor(kiln_value target, const char *key, size_t key_len, kiln_value getter, kiln_value setter, int enumerable);
extern int kiln_c99_describe_property(kiln_host *host, kiln_value target, const char *key, size_t key_len, kiln_value *out, int *found, int depth, kiln_value *thrown);
extern int kiln_c99_set_function_name(kiln_value function, kiln_value key, const char *prefix);
extern int kiln_c99_define_private(kiln_value target, const char *key, size_t key_len, kiln_value function, int kind);
extern int kiln_c99_define_method(kiln_value target, const char *key, size_t key_len, kiln_value method);
extern int kiln_c99_new_typed_iterator(kiln_value *out, kiln_value source, int part);
extern int kiln_c99_iterator_method(kiln_host *host, kiln_value *result, uint64_t code, kiln_value receiver, const kiln_value *args, size_t argc, int depth, kiln_value *thrown);
extern kiln_property *kiln_c99_own_private(kiln_value target, const char *key, size_t key_len);
extern int kiln_c99_private_error(kiln_value *thrown, const char *key, size_t key_len, int mode);
extern int kiln_c99_throw_error(kiln_value *thrown, uint64_t kind, const char *message);
extern kiln_promise *kiln_c99_promise_of(kiln_value value);
extern int kiln_c99_bind_with(kiln_value *out, kiln_value target, kiln_value receiver, const kiln_value *arguments, size_t count);
extern int kiln_c99_construct(kiln_host *host, kiln_value *result, kiln_value callee, const kiln_value *args, size_t argc, int depth, kiln_value *thrown);
extern int kiln_c99_super_call(kiln_host *host, kiln_value *produced, kiln_value parent, kiln_value receiver, const kiln_value *args, size_t argc, int depth, kiln_value *thrown);
extern int kiln_c99_adopt_super(kiln_value *receiver, kiln_value produced);
extern int kiln_c99_set_prototype(kiln_value target, kiln_value prototype);
extern int kiln_c99_instance_of(kiln_value value, kiln_value constructor, int *answer);
extern kiln_value kiln_c99_typeof(kiln_value value);
extern kiln_value kiln_c99_unary(uint16_t operation, kiln_value value);
extern kiln_value kiln_c99_binary(uint16_t operation, kiln_value left, kiln_value right);
extern int kiln_c99_unary_checked(uint16_t operation, kiln_host *host, kiln_value value, kiln_value *out, int depth, kiln_value *thrown);
extern int kiln_c99_binary_checked(uint16_t operation, kiln_host *host, kiln_value left, kiln_value right, kiln_value *out, int depth, kiln_value *thrown);
extern void kiln_c99_gc_unit_roots(void *opaque, void *context, int phase);
extern uint64_t kiln_c99_array_method(const char *key, size_t key_len);
extern uint64_t kiln_c99_string_method(const char *key, size_t key_len);

#endif /* KILN_PRELUDE_DECLARATIONS_H */
