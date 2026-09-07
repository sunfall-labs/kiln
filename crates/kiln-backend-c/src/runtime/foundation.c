
int kiln_realm_http_client_attach_active(uint64_t resource, kiln_realm_http_event_fn run,
                                         kiln_realm_drop_fn drop, void *work);
int kiln_realm_http_client_request_id_active(uint64_t *out_id);
#define KILN_REALM_HTTP_AGENT_CONNECT 1
#define KILN_REALM_HTTP_AGENT_REUSE 2
#define KILN_REALM_HTTP_AGENT_QUEUED 3
#define KILN_REALM_HTTP_AGENT_IDLE 4
#define KILN_REALM_HTTP_AGENT_DISPATCH 5
#define KILN_REALM_HTTP_AGENT_RETIRE 6
int kiln_realm_http_agent_acquire_active(const uint8_t *host, uint64_t host_len,
                                         uint16_t port, uint64_t request,
                                         uint64_t *out_resource);
int kiln_realm_http_client_owner_active(uint64_t resource, void **out_owner);
int kiln_realm_http_agent_connected_active(const uint8_t *host, uint64_t host_len,
                                           uint16_t port, uint64_t resource,
                                           uint64_t request);
int kiln_realm_http_agent_release_active(uint64_t resource, int reusable,
                                         uint64_t *out_request);
int kiln_realm_http_client_start_active(uint64_t resource,
                                        uint64_t id,
                                        const kiln_realm_http_request *request,
                                        uint64_t high_water_mark);
int kiln_realm_http_client_write_active(uint64_t resource,
                                        uint64_t id,
                                        const uint8_t *data, uint64_t data_len);
int kiln_realm_http_client_end_active(uint64_t resource,
                                      uint64_t id,
                                      const uint8_t *data, uint64_t data_len);
int kiln_realm_http_response_start_active(uint64_t resource, uint64_t id,
                                          const kiln_realm_http_response *response,
                                          uint64_t high_water_mark);
int kiln_realm_http_response_write_active(uint64_t resource, uint64_t id,
                                          const uint8_t *data, uint64_t data_len);
int kiln_realm_http_response_end_active(uint64_t resource, uint64_t id,
                                        const uint8_t *data, uint64_t data_len);

static int kiln_c99_node_http_event(void *opaque, const kiln_realm_http_event *event,
                                    void *host, void *thrown);
static void kiln_c99_node_drop_http_connection(void *opaque);

static int kiln_c99_node_client_attach(kiln_node_client_request *request,
                                       kiln_node_socket *socket,
                                       int attach_runtime,
                                       kiln_value *thrown) {
    int status;
    if (request == NULL || socket == NULL) return KILN_C99_INVALID_ARGUMENT;
    request->socket = socket;
    request->next = socket->client_requests;
    socket->client_requests = request;
    if (attach_runtime) {
        status = kiln_c99_node_socket_object(socket, thrown);
        if (status != KILN_C99_OK) return status;
        status = kiln_realm_http_client_attach_active(socket->resource,
                                                      kiln_c99_node_http_event,
                                                      kiln_c99_node_drop_http_connection,
                                                      socket);
        if (status != KILN_C99_OK) return status;
    }
    return kiln_c99_node_client_try_start(request);
}
int32_t kiln_regexp_compile(const unsigned char *pattern, size_t pattern_len,
                            const unsigned char *flags, size_t flags_len, void **out_handle);
void kiln_regexp_free(void *handle);
int32_t kiln_regexp_search(void *handle, const uint16_t *subject, size_t subject_len, size_t start,
                           int64_t *out_ranges, size_t out_capacity);
size_t kiln_regexp_group_count(const void *handle);
int32_t kiln_regexp_group_name(const void *handle, size_t index, unsigned char *buffer,
                               size_t capacity);
/* Arbitrary-precision integers. The handles are opaque: everything that knows what one contains
 * is on the other side of this boundary, which is the point (ADR 0025). */
int32_t kiln_bigint_parse(const unsigned char *text, size_t len, uint32_t radix, void **out);
int32_t kiln_bigint_from_double(double value, void **out);
void kiln_bigint_free(void *handle);
double kiln_bigint_to_double(const void *handle);
int32_t kiln_bigint_to_string(const void *handle, uint32_t radix, unsigned char *buffer,
                              size_t capacity);
int32_t kiln_bigint_binary(uint32_t operation, const void *left, const void *right, void **out);
int32_t kiln_bigint_negate(const void *handle, void **out);
int32_t kiln_bigint_compare(const void *left, const void *right);
int32_t kiln_bigint_is_zero(const void *handle);
/* The exact 128-bit value, split into a signed top half and an unsigned bottom half because C99
 * has no portable 128-bit integer. Non-zero when it does not fit, never a truncated answer:
 * `Temporal.Instant.fromEpochNanoseconds` is the caller, and an instant off by 2^128 nanoseconds
 * would be a wrong answer that prints rather than the RangeError the language specifies. */
int32_t kiln_bigint_to_i128(const void *handle, int64_t *out_high, uint64_t *out_low);
/* `Temporal`, over the same `kiln-temporal` adapter the interpreter links (ADR 0025, ADR 0029 section 4).
 *
 * The handles are opaque and every readback is a scalar, exactly as the BigInt block above. The
 * field reader is keyed by *name* because the adapter's own reader is: one call answers `year`,
 * `daysInMonth` and `blank` alike, for all eight types, so there is no per-field C at all. */
int32_t kiln_temporal_from_fields(uint32_t kind, const double *fields, size_t field_count,
                                  const unsigned char *text, const int64_t *text_lengths,
                                  size_t text_count, const unsigned char *option_text,
                                  const int64_t *option_text_lengths, size_t option_text_count,
                                  void **out_handle);
int32_t kiln_temporal_duration_from_fields(const double *fields, size_t field_count,
                                           void **out_handle);
/* `new Temporal.X(...)`, which is not `from` with a bag: a constructor takes ISO components and
 * rejects one out of range where `from` resolves it through the calendar and constrains. Both
 * reach the same linked engine; which of its constructors runs is the kind. */
int32_t kiln_temporal_construct(uint32_t kind, const double *fields, size_t field_count,
                                const unsigned char *text, const int64_t *text_lengths,
                                size_t text_count, int64_t epoch_high, uint64_t epoch_low,
                                void **out_handle);
/* `Temporal.Instant`'s two epoch constructors. The nanosecond count arrives in the same two
 * halves `kiln_temporal_now` takes, read out of the BigInt provider: the two providers are
 * independent features of one archive and only scalars pass between them. */
int32_t kiln_temporal_instant_from_epoch_milliseconds(double milliseconds, void **out_handle);
int32_t kiln_temporal_instant_from_epoch_nanoseconds(int64_t high, uint64_t low,
                                                     void **out_handle);
int32_t kiln_temporal_parse(uint32_t kind, const unsigned char *bytes, size_t len,
                            const unsigned char *option_text,
                            const int64_t *option_text_lengths, size_t option_text_count,
                            void **out_handle);
int32_t kiln_temporal_now(uint32_t kind, int64_t high, uint64_t low, const unsigned char *zone,
                          size_t zone_len, void **out_handle);
void kiln_temporal_free(void *handle);
int32_t kiln_temporal_kind(const void *handle);
int32_t kiln_temporal_get(const void *handle, const unsigned char *name, size_t name_len,
                          double *out_number, unsigned char *buffer, size_t capacity,
                          int32_t *out_length);
int32_t kiln_temporal_to_string(const void *handle, unsigned char *buffer, size_t capacity);
/* Two durations take the options: whether one month is longer than thirty days depends on which
 * month, so `Temporal.Duration.compare` needs a `relativeTo` and refuses without one. */
int32_t kiln_temporal_compare(const void *left, const void *right, const void *relative_to,
                              const unsigned char *option_text,
                              const int64_t *option_text_lengths, size_t option_text_count,
                              int32_t *out_order);
/* Everything one arithmetic or conversion operation needs, in one struct.
 *
 * The whole surface is name-keyed for the same reason the readers are: `kiln-temporal` answers
 * `add`, `until`, `round`, `with`, `equals` and the eleven conversions through names and scalars,
 * so one entry point carries all of it. A struct rather than twenty parameters, because the
 * alternative is a signature nobody can call correctly twice. */
typedef struct kiln_temporal_call {
    const void *receiver;
    const unsigned char *name;
    size_t name_len;
    const void *operand;
    const void *relative_to;
    const unsigned char *argument;
    size_t argument_len;
    const double *fields;
    size_t field_count;
    const unsigned char *field_text;
    const int64_t *field_text_lengths;
    size_t field_text_count;
    const double *option_numbers;
    size_t option_number_count;
    const unsigned char *option_text;
    const int64_t *option_text_lengths;
    size_t option_text_count;
    void **out_handle;
    double *out_number;
    unsigned char *out_buffer;
    size_t out_capacity;
    int32_t *out_length;
} kiln_temporal_call;
/* 0 a new handle, 1 a number, 2 text under the retry protocol, 3 a flag, 6 no such operation on
 * this type -- which is a refusal by name here, not a failure. Negative is an error class. */
int32_t kiln_temporal_invoke(const kiln_temporal_call *call);
int32_t kiln_temporal_to_kind(const void *handle, uint32_t kind, void **out_handle);
int32_t kiln_temporal_last_error(int32_t *out_kind, unsigned char *buffer, size_t capacity);


/* Forward declarations.
 *
 * The runtime below is one long sequence of static functions, and the order they happen to be
 * written in is not the order they call each other in: a property read reaches string conversion,
 * string conversion reaches the array join, the join reaches the read again. Declaring them all
 * here once removes the ordering constraint entirely, rather than reshuffling the file every time
 * a new call crosses an old boundary. */
static kiln_value kiln_c99_value(uint32_t tag, uint64_t bits, const char *ptr, size_t len);
static kiln_value kiln_c99_undefined(void);
static kiln_value kiln_c99_null(void);
static kiln_value kiln_c99_boolean(int value);
static kiln_value kiln_c99_number(double value);
static kiln_value kiln_c99_uninitialized(void);
static kiln_value kiln_c99_builtin(uint64_t code);
static int kiln_c99_global_binding(kiln_value *out, const char *name, size_t name_len, int *found);
static kiln_value kiln_c99_temporal_value(void *handle);
static void *kiln_c99_temporal_of(kiln_value value);
static int kiln_c99_temporal_raise(kiln_value *thrown, int32_t code);
static int kiln_c99_temporal_string(kiln_value *out, kiln_value value, kiln_value *thrown);
static const char *kiln_c99_temporal_tag(kiln_value value);
static const char *kiln_c99_temporal_builtin_name(uint64_t builtin);
static int32_t kiln_c99_temporal_kind_of_builtin(uint64_t builtin);
static uint64_t kiln_c99_temporal_builtin_of(kiln_value value);
static int kiln_c99_temporal_unsupported(kiln_value *thrown, const char *owner, const char *member,
                                         size_t member_len);
static const char *kiln_c99_temporal_op_name(uint64_t code);
static const char *kiln_c99_temporal_now_name(uint64_t code);
static uint64_t kiln_c99_temporal_op_code(const char *key, size_t key_len);
static int kiln_c99_temporal_method(kiln_host *host, kiln_value *result, uint64_t code,
                                    kiln_value receiver, const kiln_value *args, size_t argc,
                                    int depth, kiln_value *thrown);
static kiln_value kiln_c99_method(uint64_t code);
static kiln_value kiln_c99_function(kiln_closure *closure);
static kiln_closure *kiln_c99_closure_of(kiln_value value);
static kiln_val *kiln_c99_cell_of(kiln_value value);
static kiln_object *kiln_c99_object_of(kiln_value value);
static kiln_proxy *kiln_c99_proxy_of(kiln_value value);
static kiln_value kiln_c99_proxy_inner(kiln_value value);
static int kiln_c99_new_proxy(kiln_value *out, kiln_value target, kiln_value handler);
static kiln_array *kiln_c99_array_of(kiln_value value);
static kiln_typed_buffer *kiln_c99_typed_buffer_of(kiln_value value);
static kiln_typed_array *kiln_c99_typed_array_of(kiln_value value);
static kiln_data_view *kiln_c99_data_view_of(kiln_value value);
static double kiln_c99_as_number(kiln_value value);
static int kiln_c99_to_number_checked(kiln_host *host, kiln_value value, double *out, int depth,
                                      kiln_value *thrown);
static int kiln_c99_truthy(kiln_value value);
static int kiln_c99_signbit(double value);
static int kiln_c99_strict_equal(kiln_value left, kiln_value right);
static int kiln_c99_is_callable(kiln_value value);
static int kiln_c99_is_reference(kiln_value value);
static int kiln_c99_has_intrinsic_text(kiln_value value);
static int kiln_c99_crosses_boundary(kiln_value value);
static kiln_value kiln_c99_text(kiln_value value);
static kiln_value kiln_c99_copy_text(const char *text, size_t len);
static kiln_value kiln_c99_inspect(kiln_value value);
static int kiln_c99_reserve(void **items, size_t *capacity, size_t needed, size_t item_size);
static int kiln_c99_text_append(kiln_text *text, const char *bytes, size_t len);
static int kiln_c99_text_append_c(kiln_text *text, const char *literal);
static kiln_value kiln_c99_text_value(kiln_text *text);
static int kiln_c99_same_name(const char *key, size_t key_len, const char *name);
static int kiln_c99_same_key(const kiln_property *property, const char *key, size_t key_len);
static int kiln_c99_new_object(kiln_value *out);
static int kiln_c99_new_object_with_capacity(kiln_value *out, size_t capacity);
static int kiln_c99_new_object_tenured(kiln_value *out);
static int kiln_c99_new_object_with_capacity_tenured(kiln_value *out, size_t capacity);
static int kiln_c99_new_array(kiln_value *out);
static int kiln_c99_new_abort_signal(kiln_value *out);
static int kiln_c99_new_abort_controller(kiln_value *out);
static int kiln_c99_new_text_encoder(kiln_value *out);
static int kiln_c99_new_text_decoder(kiln_value *out, const kiln_value *args, size_t argc,
                                     kiln_value *thrown);
static int kiln_c99_new_async_local_storage(kiln_value *out);
static int kiln_c99_abort(kiln_host *host, kiln_value signal, kiln_value reason, int depth,
                          kiln_value *thrown);
static int kiln_c99_new_typed_buffer(kiln_value *out, size_t byte_length);
static int kiln_c99_new_typed_array(kiln_value *out, kiln_typed_kind kind,
                                    kiln_value source, size_t byte_offset, size_t length,
                                    int has_length, kiln_value *thrown);
static int kiln_c99_new_data_view(kiln_value *out, kiln_value source, size_t byte_offset,
                                  size_t byte_length, int has_length, kiln_value *thrown);
static int kiln_c99_to_index(kiln_value value, size_t *out, kiln_value *thrown);
static size_t kiln_c99_typed_width(kiln_typed_kind kind);
static double kiln_c99_typed_get(kiln_typed_array *view, size_t index);
static void kiln_c99_typed_set(kiln_typed_array *view, size_t index, double value);
static int kiln_c99_new_typed_iterator(kiln_value *out, kiln_value source, int part);
static int kiln_c99_own_slot(kiln_value target, const char *key, size_t key_len,
                             kiln_property **out);
static int kiln_c99_own_slot_ikey(kiln_value target, const char *key, size_t key_len,
                                  const char *ikey, kiln_property **out);
static int kiln_c99_own_data(kiln_value target, const char *key, size_t key_len,
                             kiln_value value);
static int kiln_c99_own_data_hot_key(kiln_value target, int id, kiln_value value);
static int kiln_c99_put_hot_key(kiln_host *host, kiln_value target, int id, kiln_value value,
                                int depth, kiln_value *thrown);
static int kiln_c99_put_refused(kiln_value target, const char *key, size_t key_len);
static int kiln_c99_put(kiln_host *host, kiln_value target, const char *key, size_t key_len,
                        kiln_value value, int depth, kiln_value *thrown);
static int kiln_c99_put_mode(kiln_host *host, kiln_value target, const char *key, size_t key_len,
                             kiln_value value, int strict, int depth, kiln_value *thrown);
static int kiln_c99_put_mode_ikey(kiln_host *host, kiln_value target, const char *key,
                                  size_t key_len, const char *ikey, kiln_value value, int strict,
                                  int depth, kiln_value *thrown);
static int kiln_c99_get(kiln_host *host, kiln_value target, const char *key, size_t key_len,
                        kiln_value *out, int depth, kiln_value *thrown);
static int kiln_c99_get_ikey(kiln_host *host, kiln_value target, const char *key, size_t key_len,
                             const char *ikey, kiln_value *out, int depth, kiln_value *thrown);
static kiln_value *kiln_c99_exotic_store(kiln_value target);
static int kiln_c99_proxy_trap(kiln_host *host, kiln_value proxy, const char *name,
                               kiln_value *trap, int depth, kiln_value *thrown);
static int kiln_c99_proxy_get(kiln_host *host, kiln_value proxy, kiln_value key,
                              kiln_value receiver, kiln_value *out, int depth,
                              kiln_value *thrown);
static int kiln_c99_get_with_receiver(kiln_host *host, kiln_value target, kiln_value key,
                                      kiln_value receiver, kiln_value *out, int depth,
                                      kiln_value *thrown);
static int kiln_c99_proxy_set(kiln_host *host, kiln_value proxy, kiln_value key, kiln_value value,
                              int strict, int depth, kiln_value *thrown);
static int kiln_c99_set_computed(kiln_host *host, kiln_value target, kiln_value key,
                                 kiln_value value, int depth, kiln_value *thrown);
static int kiln_c99_set_computed_mode(kiln_host *host, kiln_value target, kiln_value key,
                                      kiln_value value, int strict, int depth,
                                      kiln_value *thrown);
static int kiln_c99_reflect_set(kiln_host *host, kiln_value target, kiln_value key,
                                kiln_value value, int *accepted, int depth,
                                kiln_value *thrown);
static int kiln_c99_get_computed(kiln_host *host, kiln_value target, kiln_value key,
                                 kiln_value *out, int depth, kiln_value *thrown);
static int kiln_c99_definition_store(kiln_value target, kiln_value *out);
static int kiln_c99_array_constructor(kiln_value *out, const kiln_value *args, size_t argc,
                                      kiln_value *thrown);
static int kiln_c99_object_constructor(kiln_value *out, const kiln_value *args, size_t argc);
static int kiln_c99_define_property(kiln_host *host, kiln_value target, const char *key,
                                    size_t key_len, kiln_value descriptor, int depth,
                                    kiln_value *thrown);
static int kiln_c99_define_properties(kiln_host *host, kiln_value target, kiln_value descriptors,
                                      int depth, kiln_value *thrown);
static int kiln_c99_describe_property(kiln_host *host, kiln_value target, const char *key,
                                      size_t key_len, kiln_value *out, int *found, int depth,
                                      kiln_value *thrown);
static int kiln_c99_own_property_present(kiln_host *host, kiln_value target, kiln_value key,
                                         int *found, int depth, kiln_value *thrown);
static int kiln_c99_proxy_own_keys(kiln_host *host, kiln_value proxy, kiln_value *out, int depth,
                                   kiln_value *thrown);
static int kiln_c99_proxy_describe(kiln_host *host, kiln_value proxy, kiln_value key,
                                   kiln_value *out, int *found, int depth, kiln_value *thrown);
static int kiln_c99_proxy_define(kiln_host *host, kiln_value proxy, kiln_value key,
                                 kiln_value descriptor, int depth, kiln_value *thrown);
static int kiln_c99_proxy_all_keys(kiln_host *host, kiln_value proxy, kiln_value *out, int depth,
                                   kiln_value *thrown);
static int kiln_c99_all_own_keys(kiln_host *host, kiln_value target, kiln_value *out, int depth,
                                 kiln_value *thrown);
static int kiln_c99_construct(kiln_host *host, kiln_value *result, kiln_value callee,
                              const kiln_value *args, size_t argc, int depth, kiln_value *thrown);
static int kiln_c99_call_val(kiln_host *host, kiln_val *result, kiln_val callee,
                             kiln_val receiver, const kiln_val *args, size_t argc,
                             int depth, kiln_value *thrown);
static int kiln_c99_construct_val(kiln_host *host, kiln_val *result, kiln_val callee,
                                  const kiln_val *args, size_t argc,
                                  int depth, kiln_value *thrown);
static int kiln_c99_super_call_val(kiln_host *host, kiln_val *produced, kiln_val parent,
                                   kiln_val receiver, const kiln_val *args, size_t argc,
                                   int depth, kiln_value *thrown);
static int kiln_c99_store_get(kiln_host *host, kiln_value target, const char *key, size_t key_len,
                              kiln_value *out, int depth, kiln_value *thrown, int *found);
static int kiln_c99_get_own_kind(kiln_host *host, kiln_value target, const char *key,
                                 size_t key_len, const char *ikey, kiln_value *out, int depth,
                                 kiln_value *thrown);
static int kiln_c99_array_reserve(kiln_array *array, size_t needed);
static int kiln_c99_array_push(kiln_value target, kiln_value value);
static int kiln_c99_array_push_val(kiln_value target, kiln_val value);
static int kiln_c99_prototype_object(kiln_value function, kiln_value *out);
static int kiln_c99_callable_prototype(kiln_value callable, kiln_value *out);
static int kiln_c99_install_event_emitter_methods(kiln_value target);
static int kiln_c99_install_readable_methods(kiln_value target);
static int kiln_c99_install_writable_methods(kiln_value target);
static int kiln_c99_install_server_response_methods(kiln_value target);
static int kiln_c99_to_units(kiln_value value, uint16_t **out, size_t *count);
static kiln_value kiln_c99_from_units(const uint16_t *units, size_t count);
static const char *kiln_c99_method_name(uint64_t code);
static const char *kiln_c99_builtin_name(uint64_t builtin);
static int kiln_c99_is_namespace(uint64_t builtin);
static int kiln_c99_changes_length(uint64_t code);
static int kiln_c99_reads_only(uint64_t code);
static uint64_t kiln_c99_callable_method(const char *key, size_t key_len);
static kiln_value kiln_c99_property_key(kiln_value key);
static uint32_t kiln_c99_to_uint32(double value);
static kiln_value kiln_c99_bigint_text(kiln_value value, uint32_t radix);
static kiln_value kiln_c99_bigint_value(void *handle);
static const void *kiln_c99_bigint_one(void);
static int kiln_c99_bigint_constant(void **cache, const char *digits, size_t len,
                                    kiln_value *out);
static int kiln_c99_expand_replacement(kiln_text *out, const uint16_t *pattern,
                                       size_t pattern_count, const uint16_t *subject,
                                       size_t subject_count, const int64_t *ranges, size_t groups);
static kiln_regexp *kiln_c99_regexp_of(kiln_value value);
static int kiln_c99_global_object(kiln_value *out);
static int kiln_c99_global_object_val(kiln_val *out);
static int kiln_c99_new_regexp(kiln_value *out, const char *source, size_t source_len,
                               const char *flags, size_t flags_len, kiln_value *thrown);
static int kiln_c99_string_compare(kiln_value left, kiln_value right);
static uint64_t kiln_c99_array_method(const char *key, size_t key_len);
static uint64_t kiln_c99_string_method(const char *key, size_t key_len);
static int kiln_c99_number_in_radix(kiln_value *out, double value, int radix);
static int kiln_c99_to_fixed(kiln_value *out, double value, int places);
static size_t kiln_c99_exact_digits(double value, unsigned char *digits, size_t capacity,
                                    size_t *fractional);
static void kiln_c99_significant_digits(double value, size_t count, char *mantissa,
                                        int *exponent);
static int kiln_c99_shortest_digits(double value, char *mantissa, size_t capacity, int *exponent);
static int kiln_c99_exponential_text(kiln_value *out, int negative, const char *mantissa,
                                     int exponent);
static int kiln_c99_precision_text(kiln_value *out, int negative, const char *mantissa,
                                   size_t digits, int exponent);
static int kiln_c99_hex_digit(char character);
static int kiln_c99_append_hex(kiln_text *text, unsigned value, int width);
static int kiln_c99_valid_utf8(const char *bytes, size_t len);
static int kiln_c99_url_construct(kiln_value *out, const kiln_value *args, size_t argc,
                                  kiln_value *thrown);
static int kiln_c99_url_search_params_get(kiln_value *out, kiln_value receiver,
                                          const kiln_value *args, size_t argc,
                                          kiln_value *thrown);
static int kiln_c99_form_decode(const char *bytes, size_t len, kiln_value *out);
static int kiln_c99_node_querystring_parse(kiln_value *out, const kiln_value *args, size_t argc);
static int kiln_c99_typed_alloc(kiln_value *out, kiln_typed_kind kind, size_t count);
static int kiln_c99_new_typed_array(kiln_value *out, kiln_typed_kind kind,
                                    kiln_value source, size_t byte_offset, size_t length,
                                    int has_length, kiln_value *thrown);
static int kiln_c99_buffer_from(kiln_value *out, kiln_value source, kiln_value *thrown);
static int kiln_c99_buffer_method(kiln_value *out, uint64_t code, kiln_value receiver,
                                  const kiln_value *args, size_t argc, kiln_value *thrown);
static int kiln_c99_node_util(kiln_host *host, kiln_value *out, uint64_t code,
                              const kiln_value *args, size_t argc, int depth,
                              kiln_value *thrown);
static int kiln_c99_node_diagnostics(kiln_host *host, kiln_value *out, uint64_t code,
                                     const kiln_value *args, size_t argc, int depth,
                                     kiln_value *thrown);
static int kiln_c99_node_dns_lookup(kiln_host *host, kiln_value *out,
                                    const kiln_value *args, size_t argc, int depth,
                                    kiln_value *thrown);
static int kiln_c99_node_path(kiln_value *out, uint64_t code,
                              const kiln_value *args, size_t argc, kiln_value *thrown);
static int kiln_c99_node_stream(kiln_host *host, kiln_value *out, uint64_t code,
                                kiln_value receiver, const kiln_value *args, size_t argc,
                                int depth, kiln_value *thrown);
static kiln_value kiln_c99_copy_text(const char *bytes, size_t len);
static kiln_value kiln_c99_static_text(const char *text, size_t len);
static kiln_value kiln_c99_control_diagnostic(void);
static int64_t kiln_c99_days_from_civil(int64_t year, int64_t month, int64_t day);
static kiln_date_parts kiln_c99_date_parts(double time);
static double kiln_c99_date_time(kiln_value value);
static int kiln_c99_new_date(kiln_value *out, double time);
static int kiln_c99_iso_string(kiln_value *out, double time);
static int kiln_c99_date_to_string(kiln_value *out, double time);
static double kiln_c99_instant_from_parts(const double parts[7]);
static double kiln_c99_parse_date(const char *bytes, size_t len);
static int kiln_c99_pad_number(kiln_text *text, int64_t value, int width);
static int kiln_c99_is_hole(kiln_value value);
static kiln_collection *kiln_c99_collection_of(kiln_value value);
static int kiln_c99_new_collection(kiln_value *out, int is_map, int is_weak);
static const char *kiln_c99_error_name(uint64_t builtin);
/* Defined beside the surface table it reads, which the assembled prelude places after this file. */
static const char *kiln_c99_surface_method_name(uint64_t code);
static uint64_t kiln_c99_error_kind(kiln_value value);
static int kiln_c99_error_to_string(kiln_host *host, kiln_value *result, kiln_value receiver,
                                    int depth, kiln_value *thrown);
static int kiln_c99_new_error(kiln_value *out, uint64_t kind, kiln_value message);
static int kiln_c99_aggregate_error(kiln_value *out, kiln_value reasons);
static int kiln_c99_aggregate_error_from_args(kiln_host *host, kiln_value *out,
                                               const kiln_value *args, size_t argc, int depth,
                                               kiln_value *thrown);
static kiln_property *kiln_c99_own_private(kiln_value target, const char *key, size_t key_len);
static int kiln_c99_private_error(kiln_value *thrown, const char *key, size_t key_len,
                                  int mode);
static int kiln_c99_define_private(kiln_value target, const char *key, size_t key_len,
                                   kiln_value function, int kind);
static int kiln_c99_is_node_hidden_key(const char *key, size_t key_len);
static int kiln_c99_set_prototype(kiln_value target, kiln_value prototype);
static int kiln_c99_can_be_prototype(kiln_value value);
static int kiln_c99_new_promise(kiln_value *out);
static kiln_promise *kiln_c99_promise_of(kiln_value value);
static int kiln_c99_new_settler(kiln_value *out, kiln_value promise, int rejects);
static int kiln_c99_bind(kiln_value *out, kiln_value target, kiln_value receiver);
static int kiln_c99_bind_with(kiln_value *out, kiln_value target, kiln_value receiver,
                              const kiln_value *arguments, size_t count);
static int kiln_c99_settle(kiln_value promise, kiln_value value, int rejected);
static int kiln_c99_then(kiln_value promise, kiln_value on_fulfilled, kiln_value on_rejected,
                         kiln_value *out);
static int kiln_c99_enqueue(kiln_value handler, kiln_value argument, kiln_value derived,
                            int rejected, int continuation);
static int kiln_c99_enqueue_next_tick(kiln_value callback, const kiln_value *args, size_t argc);
static int kiln_c99_run_microtask(void *work, void *host, void *thrown);
static void kiln_c99_drop_microtask(void *work);
static int kiln_c99_run_next_tick(void *work, void *host, void *thrown);
static void kiln_c99_drop_next_tick(void *work);
static int kiln_c99_run_timer(void *work, void *host, void *thrown);
static void kiln_c99_drop_timer(void *work);
static int kiln_c99_schedule(kiln_promise *promise, const kiln_reaction *reaction);
static int kiln_c99_async_step(kiln_host *host, kiln_value generator, kiln_value sent,
                               int rejected, int depth, kiln_value *thrown);
static int kiln_c99_async_generator_step(kiln_host *host, kiln_value generator, kiln_value sent,
                                         int rejected, int depth, kiln_value *thrown);
static int kiln_c99_async_generator_settle(kiln_host *host, kiln_value step, kiln_value value,
                                           int finished, int depth, kiln_value *thrown);
static int kiln_c99_new_generator(kiln_value *out, kiln_value function, kiln_value receiver,
                                  const kiln_value *args, size_t argc);
static kiln_generator *kiln_c99_generator_of(kiln_value value);
static int kiln_c99_function_store(kiln_value function, kiln_value *out);
static int kiln_c99_callable_store(kiln_value callable, kiln_value *out, int create);
static int kiln_c99_throw_error(kiln_value *thrown, uint64_t kind, const char *message);
static int kiln_c99_throw_node_invalid_arg_type(kiln_value *thrown, const char *message);
static int kiln_c99_throw_named(kiln_value *thrown, uint64_t kind, const char *sentence,
                                const char *key, size_t key_len);
static int kiln_c99_throw_not_a(kiln_value *thrown, kiln_value value, const char *role);
static int kiln_c99_throw_not_iterable(kiln_value *thrown, kiln_value value);
static int kiln_c99_throw_cannot_set(kiln_value *thrown, kiln_value target);
static int kiln_c99_throw_read_only(kiln_value *thrown, const char *key, size_t key_len);
static int kiln_c99_throw_module_namespace_write(kiln_value *thrown, const char *key,
                                                 size_t key_len);
static int kiln_c99_throw_module_namespace_delete(kiln_value *thrown, const char *key,
                                                  size_t key_len);
static int kiln_c99_throw_module_namespace_define(kiln_value *thrown, const char *key,
                                                  size_t key_len);
static int kiln_c99_throw_module_namespace_freeze(kiln_value *thrown);
static int kiln_c99_throw_cannot_delete(kiln_value *thrown, const char *key, size_t key_len);
static int kiln_c99_binary_checked(uint16_t operation, kiln_host *host, kiln_value left,
                                   kiln_value right, kiln_value *out, int depth,
                                   kiln_value *thrown);
static int kiln_c99_to_primitive(kiln_host *host, kiln_value value, int string_hint,
                                 kiln_value *out, int depth, kiln_value *thrown);
static int kiln_c99_unary_checked(uint16_t operation, kiln_host *host, kiln_value value,
                                  kiln_value *out, int depth, kiln_value *thrown);
static int kiln_c99_new_iterator(kiln_value *out, kiln_value items);
static double kiln_c99_collection_find(kiln_collection *collection, kiln_value key);
static int kiln_c99_collection_put(kiln_value target, kiln_value key, kiln_value value);
static void kiln_c99_collection_hash_remove(kiln_collection *collection, size_t entry_idx, kiln_value key);
static int kiln_c99_same_value_zero(kiln_value left, kiln_value right);
static int kiln_c99_iterate(kiln_host *host, kiln_value target, kiln_value source, int depth,
                            kiln_value *thrown);
static kiln_value kiln_c99_hole(void);
static int kiln_c99_has_property(kiln_host *host, kiln_value target, const char *key,
                                 size_t key_len, int *answer, int depth, kiln_value *thrown);
static int kiln_c99_has_computed(kiln_host *host, kiln_value target, kiln_value key, int *answer,
                                 int depth, kiln_value *thrown);
static int kiln_c99_delete_computed(kiln_host *host, kiln_value target, kiln_value key,
                                    int *answer, int depth, kiln_value *thrown);
static int kiln_c99_delete(kiln_host *host, kiln_value target, const char *key, size_t key_len,
                           int *answer, int depth, kiln_value *thrown);
static int kiln_c99_delete_or_throw(kiln_host *host, kiln_value target, const char *key,
                                    size_t key_len, int strict, int *answer, int depth,
                                    kiln_value *thrown);
static int kiln_c99_delete_computed_or_throw(kiln_host *host, kiln_value target, kiln_value key,
                                             int strict, int *answer, int depth,
                                             kiln_value *thrown);
static void kiln_c99_typed_own(kiln_value target, const char *key, size_t key_len,
                               int *found, int *enumerable);
static int kiln_c99_typed_prototype_own(kiln_value target, const char *key, size_t key_len,
                                        int *found, int *enumerable);
static int kiln_c99_own_keys(kiln_host *host, kiln_value target, kiln_value *out, int depth,
                             kiln_value *thrown);
static int kiln_c99_enumerate_keys(kiln_host *host, kiln_value target, kiln_value *out,
                                   int depth, kiln_value *thrown);
static int kiln_c99_define_method(kiln_value target, const char *key, size_t key_len,
                                  kiln_value method);
static int kiln_c99_define_accessor(kiln_value target, const char *key, size_t key_len,
                                    kiln_value getter, kiln_value setter, int enumerable);
static int kiln_c99_set_function_name(kiln_value function, kiln_value key,
                                     const char *prefix);
static double kiln_c99_key_index(const char *key, size_t key_len);
static uint64_t kiln_c99_array_method(const char *key, size_t key_len);
/* The containers currently being written, innermost first.
 *
 * `JSON.stringify` is defined to fail on a cycle rather than recurse forever, and the only way to
 * know a value is a cycle is to ask whether it is one of its own ancestors. The recursion already
 * holds that chain in its stack frames, so linking them answers the question without allocating
 * and without a depth limit standing in for it -- a limit would reject a deep document that is
 * perfectly finite. */
typedef struct kiln_json_frame {
    const struct kiln_json_frame *parent;
    const void *container;
} kiln_json_frame;

static int kiln_c99_json_value(kiln_host *host, kiln_text *text, kiln_value value,
                               const char *indent, size_t indent_len, const char *current,
                               size_t current_len, int *serialised, int depth,
                               const kiln_json_frame *parents, kiln_value *thrown);
static int kiln_c99_json_read(kiln_json_reader *reader, kiln_value *out);
static int kiln_c99_json_document(const char *bytes, size_t len, kiln_value *out);
static void kiln_c99_json_space(kiln_json_reader *reader);
static int kiln_c99_call(kiln_host *host, kiln_value *result, kiln_value callee,
                         kiln_value receiver, const kiln_value *args, size_t argc, int depth,
                         kiln_value *thrown);
static int kiln_c99_invoke_method(kiln_host *host, kiln_value *result, uint64_t code,
                                  kiln_value receiver, const kiln_value *args, size_t argc,
                                  int depth, kiln_value *thrown);
static int kiln_c99_invoke_method_dispatch(kiln_host *host, kiln_value *result, uint64_t code,
                                           kiln_value receiver, const kiln_value *args,
                                           size_t argc, int depth, kiln_value *thrown);
static int kiln_c99_node_event(kiln_host *host, kiln_value *result, uint64_t code,
                               kiln_value receiver, const kiln_value *args, size_t argc,
                               int depth, kiln_value *thrown);
static int kiln_c99_node_net(kiln_host *host, kiln_value *result, uint64_t code,
                             kiln_value receiver, const kiln_value *args, size_t argc,
                             int depth, kiln_value *thrown);
static int kiln_c99_node_http(kiln_host *host, kiln_value *result, uint64_t code,
                              kiln_value receiver, const kiln_value *args, size_t argc,
                              int depth, kiln_value *thrown);
static int kiln_c99_node_fs(kiln_host *host, kiln_value *result, uint64_t code,
                            kiln_value receiver, const kiln_value *args, size_t argc,
                            int depth, kiln_value *thrown);
static int kiln_c99_iterator_method(kiln_host *host, kiln_value *result, uint64_t code,
                                    kiln_value receiver, const kiln_value *args, size_t argc,
                                    int depth, kiln_value *thrown);

static kiln_value kiln_c99_value(uint32_t tag, uint64_t bits, const char *ptr, size_t len) {
    kiln_value value;
    value.tag = tag;
    value.len = (uint32_t)len;
    /* `bits` and `ptr` share one word: a value is a payload or a pointer, never both. */
    if (ptr != NULL) { value.ptr = ptr; } else { value.bits = bits; }
    return value;
}

/* Realm stream semantics. The transport adapters only feed data and lifecycle events into these
 * ordinary objects; no socket, reactor, clock, or scheduler state is duplicated here. */
static int kiln_c99_node_stream(kiln_host *host, kiln_value *result, uint64_t code,
                                kiln_value receiver, const kiln_value *args, size_t argc,
                                int depth, kiln_value *thrown) {
    kiln_value object = receiver;
    kiln_value ignored;
    kiln_property *buffer;
    int status;
    if (code == KILN_C99_METHOD_NODE_STREAM_ADD_ABORT_SIGNAL) {
        *result = argc > 1 ? args[1] : kiln_c99_undefined();
        return KILN_C99_OK;
    }
    if (code == KILN_C99_METHOD_NODE_STREAM_FINISHED) {
        kiln_value stream = argc > 0 ? args[0] : kiln_c99_undefined();
        kiln_value callback = argc > 1 ? args[argc - 1] : kiln_c99_undefined();
        kiln_value ended = kiln_c99_undefined();
        kiln_value event;
        kiln_value once_args[2];
        if (!kiln_c99_is_callable(callback)) return kiln_c99_throw_not_a(thrown, callback, "function");
        if (stream.tag != KILN_VALUE_OBJECT) return kiln_c99_throw_not_a(thrown, stream, "stream");
        status = kiln_c99_get(host, stream, "writableEnded", 13, &ended, depth, thrown);
        if (status != KILN_C99_OK) return status;
        if (!kiln_c99_truthy(ended)) {
            status = kiln_c99_get(host, stream, "readableEnded", 13, &ended, depth, thrown);
            if (status != KILN_C99_OK) return status;
            event = kiln_c99_copy_text(kiln_c99_truthy(ended) ? "end" : "finish",
                                       kiln_c99_truthy(ended) ? 3 : 6);
            once_args[0] = event;
            once_args[1] = callback;
            status = kiln_c99_node_event(host, &ignored, KILN_C99_METHOD_NODE_EVENT_ONCE,
                                         stream, once_args, 2, depth, thrown);
            if (status != KILN_C99_OK) return status;
        } else {
            status = kiln_c99_call(host, &ignored, callback, kiln_c99_undefined(), NULL, 0,
                                   depth, thrown);
            if (status != KILN_C99_OK) return status;
        }
        *result = kiln_c99_method(KILN_C99_METHOD_NODE_UTIL_NOOP);
        return KILN_C99_OK;
    }
    if (code == KILN_C99_METHOD_NODE_STREAM || code == KILN_C99_METHOD_NODE_READABLE ||
        code == KILN_C99_METHOD_NODE_WRITABLE) {
        if (object.tag != KILN_VALUE_OBJECT) {
            status = kiln_c99_new_object(&object);
            if (status != KILN_C99_OK) return status;
        }
        status = kiln_c99_node_event(host, &ignored, KILN_C99_METHOD_NODE_EVENT_EMITTER,
                                     object, NULL, 0, depth, thrown);
        if (status != KILN_C99_OK) return status;
        if (code == KILN_C99_METHOD_NODE_READABLE) {
            kiln_value queue;
            status = kiln_c99_new_array(&queue);
            if (status == KILN_C99_OK) status = kiln_c99_put(NULL, object, "@@node:readable", 15, queue, 0, NULL);
            if (status == KILN_C99_OK) status = kiln_c99_put(NULL, object, "readableEnded", 13, kiln_c99_boolean(0), 0, NULL);
            if (status == KILN_C99_OK) status = kiln_c99_put(NULL, object, "destroyed", 9, kiln_c99_boolean(0), 0, NULL);
        } else if (code == KILN_C99_METHOD_NODE_WRITABLE) {
            status = kiln_c99_put(NULL, object, "writableEnded", 13, kiln_c99_boolean(0), 0, NULL);
            if (status == KILN_C99_OK) status = kiln_c99_put(NULL, object, "writableFinished", 16, kiln_c99_boolean(0), 0, NULL);
            if (status == KILN_C99_OK) status = kiln_c99_put(NULL, object, "destroyed", 9, kiln_c99_boolean(0), 0, NULL);
        }
        *result = object;
        return status;
    }
    if (receiver.tag != KILN_VALUE_OBJECT) return kiln_c99_throw_not_a(thrown, receiver, "stream");
    buffer = kiln_c99_own_private(receiver, "@@node:readable", 15);
    if (code == KILN_C99_METHOD_NODE_READABLE_PUSH) {
        kiln_value chunk = argc > 0 ? args[0] : kiln_c99_undefined();
        kiln_value emit_args[2];
        if (buffer == NULL || buffer->value.tag != KILN_VALUE_ARRAY) return kiln_c99_throw_not_a(thrown, receiver, "Readable");
        if (chunk.tag == KILN_VALUE_NULL) {
            status = kiln_c99_put(NULL, receiver, "readableEnded", 13, kiln_c99_boolean(1), 0, NULL);
            emit_args[0] = kiln_c99_copy_text("end", 3);
            status = status == KILN_C99_OK ? kiln_c99_node_event(host, &ignored, KILN_C99_METHOD_NODE_EVENT_EMIT, receiver, emit_args, 1, depth, thrown) : status;
            *result = kiln_c99_boolean(0);
            return status;
        }
        status = kiln_c99_array_push(buffer->value, chunk);
        emit_args[0] = kiln_c99_copy_text("readable", 8);
        emit_args[1] = chunk;
        if (status == KILN_C99_OK) status = kiln_c99_node_event(host, &ignored, KILN_C99_METHOD_NODE_EVENT_EMIT, receiver, emit_args, 1, depth, thrown);
        *result = kiln_c99_boolean(1);
        return status;
    }
    if (code == KILN_C99_METHOD_NODE_READABLE_READ) {
        kiln_array *queue;
        size_t index;
        if (buffer == NULL || buffer->value.tag != KILN_VALUE_ARRAY) return kiln_c99_throw_not_a(thrown, receiver, "Readable");
        queue = kiln_c99_array_of(buffer->value);
        if (queue->count == 0) { *result = kiln_c99_null(); return KILN_C99_OK; }
        *result = kiln_val_to_boundary(queue->elements[0]);
        for (index = 1; index < queue->count; ++index) queue->elements[index - 1] = queue->elements[index];
        queue->count -= 1;
        return KILN_C99_OK;
    }
    if (code == KILN_C99_METHOD_NODE_READABLE_PAUSE || code == KILN_C99_METHOD_NODE_READABLE_RESUME ||
        code == KILN_C99_METHOD_NODE_READABLE_SET_ENCODING || code == KILN_C99_METHOD_NODE_READABLE_UNPIPE) {
        *result = receiver; return KILN_C99_OK;
    }
    if (code == KILN_C99_METHOD_NODE_READABLE_PIPE) { *result = argc > 0 ? args[0] : kiln_c99_undefined(); return KILN_C99_OK; }
    if (code == KILN_C99_METHOD_NODE_WRITABLE_WRITE) {
        kiln_value hook;
        status = kiln_c99_get(host, receiver, "_write", 6, &hook, depth, thrown);
        if (status != KILN_C99_OK) return status;
        if (kiln_c99_is_callable(hook)) {
            kiln_value callback = kiln_c99_method(KILN_C99_METHOD_NODE_UTIL_NOOP);
            kiln_value write_args[3] = { argc > 0 ? args[0] : kiln_c99_undefined(), kiln_c99_copy_text("buffer", 6), callback };
            status = kiln_c99_call(host, &ignored, hook, receiver, write_args, 3, depth, thrown);
            if (status != KILN_C99_OK) return status;
        }
        *result = kiln_c99_boolean(1); return KILN_C99_OK;
    }
    if (code == KILN_C99_METHOD_NODE_WRITABLE_END) {
        if (argc > 0 && args[0].tag != KILN_VALUE_UNDEFINED) {
            status = kiln_c99_node_stream(host, &ignored, KILN_C99_METHOD_NODE_WRITABLE_WRITE, receiver, args, 1, depth, thrown);
            if (status != KILN_C99_OK) return status;
        }
        status = kiln_c99_put(NULL, receiver, "writableEnded", 13, kiln_c99_boolean(1), 0, NULL);
        if (status == KILN_C99_OK) status = kiln_c99_put(NULL, receiver, "writableFinished", 16, kiln_c99_boolean(1), 0, NULL);
        if (status == KILN_C99_OK) { kiln_value emit_args[1] = { kiln_c99_copy_text("finish", 6) }; status = kiln_c99_node_event(host, &ignored, KILN_C99_METHOD_NODE_EVENT_EMIT, receiver, emit_args, 1, depth, thrown); }
        *result = receiver; return status;
    }
    if (code == KILN_C99_METHOD_NODE_READABLE_DESTROY || code == KILN_C99_METHOD_NODE_WRITABLE_DESTROY) {
        kiln_value emit_args[1] = { kiln_c99_copy_text("close", 5) };
        status = kiln_c99_put(NULL, receiver, "destroyed", 9, kiln_c99_boolean(1), 0, NULL);
        if (status == KILN_C99_OK) status = kiln_c99_node_event(host, &ignored, KILN_C99_METHOD_NODE_EVENT_EMIT, receiver, emit_args, 1, depth, thrown);
        *result = receiver; return status;
    }
    KILN_C99_REFUSE("a stream method this backend has not implemented");
}

/* `node:path`'s algorithms live in Rust now, in the crate the interpreter links too: `dirname`
 * and friends are fiddlier than they look -- `dirname("a//b")` is "a/", `extname("..")` is "" but
 * `extname("...")` is "." -- and the two hand-written copies had drifted from node in seven and
 * five places respectively, in different places. One implementation cannot disagree with itself.
 *
 * The answer is a slice of the caller's own path for everything but `normalize`, so the provider
 * allocates nothing and this copies once. The caller passes a bound it can prove, which makes this
 * a single call rather than the usual ask-the-length-then-ask-again pair; the stack covers every
 * ordinary path and only an absurd one reaches the heap. */
#define KILN_C99_NODE_PATH_NORMALIZE 3u
#define KILN_C99_NODE_PATH_RELATIVE 4u
static int kiln_c99_node_path_answer(uint32_t part, const uint8_t *first, uint64_t first_len,
                                     const uint8_t *second, uint64_t second_len, size_t bound,
                                     kiln_value *result, kiln_value *thrown) {
    uint8_t inline_buffer[512];
    uint8_t *buffer = inline_buffer;
    uint8_t *spilled = NULL;
    uint64_t written = 0;
    int status;
    if (bound > sizeof inline_buffer) {
        spilled = (uint8_t *)malloc(bound);
        if (spilled == NULL)
            return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_RANGE_ERROR,
                                        "out of memory answering node:path");
        buffer = spilled;
    }
    status = part == KILN_C99_NODE_PATH_RELATIVE
                 ? kiln_node_path_relative(first, first_len, second, second_len, buffer,
                                           (uint64_t)bound, &written)
                 : kiln_node_path_part(part, first, first_len, second, second_len, buffer,
                                       (uint64_t)bound, &written);
    if (status == KILN_C99_OK)
        *result = kiln_c99_copy_text((const char *)buffer, (size_t)written);
    free(spilled);
    if (status != KILN_C99_OK)
        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_ERROR,
                                    "node:path provider refused the request");
    return KILN_C99_OK;
}

static int kiln_c99_node_path(kiln_value *result, uint64_t code,
                              const kiln_value *args, size_t argc, kiln_value *thrown) {
    kiln_text joined = {0};
    size_t index;
    int absolute = code == KILN_C99_METHOD_NODE_PATH_RESOLVE;
    int status = KILN_C99_OK;
    if (code == KILN_C99_METHOD_NODE_PATH_IS_ABSOLUTE) {
        kiln_value path = argc > 0 ? args[0] : kiln_c99_undefined();
        if (path.tag != KILN_VALUE_STRING)
            return kiln_c99_throw_not_a(thrown, path, "string path");
        *result = kiln_c99_boolean(path.len > 0 && path.ptr[0] == '/');
        return KILN_C99_OK;
    }
    if (code == KILN_C99_METHOD_NODE_PATH_DIRNAME ||
        code == KILN_C99_METHOD_NODE_PATH_BASENAME ||
        code == KILN_C99_METHOD_NODE_PATH_EXTNAME) {
        kiln_value path = argc > 0 ? args[0] : kiln_c99_undefined();
        const uint8_t *suffix_ptr = NULL;
        uint64_t suffix_len = 0;
        uint32_t part = code == KILN_C99_METHOD_NODE_PATH_DIRNAME    ? 0u
                        : code == KILN_C99_METHOD_NODE_PATH_BASENAME ? 1u
                                                                     : 2u;
        if (path.tag != KILN_VALUE_STRING)
            return kiln_c99_throw_not_a(thrown, path, "string path");
        if (part == 1u && argc > 1 && args[1].tag == KILN_VALUE_STRING) {
            suffix_ptr = (const uint8_t *)args[1].ptr;
            suffix_len = (uint64_t)args[1].len;
        }
        /* No decomposition is longer than the path it came from; the empty path answers ".". */
        return kiln_c99_node_path_answer(part, (const uint8_t *)path.ptr, (uint64_t)path.len,
                                         suffix_ptr, suffix_len, path.len + 1, result, thrown);
    }
    if (code == KILN_C99_METHOD_NODE_PATH_RELATIVE) {
        kiln_value from = argc > 0 ? args[0] : kiln_c99_undefined();
        kiln_value to = argc > 1 ? args[1] : kiln_c99_undefined();
        if (from.tag != KILN_VALUE_STRING)
            return kiln_c99_throw_not_a(thrown, from, "string path");
        if (to.tag != KILN_VALUE_STRING)
            return kiln_c99_throw_not_a(thrown, to, "string path");
        /* Unlike the decompositions this can outgrow both inputs: every component of `from` that
         * `to` does not share becomes "../", and a component can be a single byte. */
        return kiln_c99_node_path_answer(KILN_C99_NODE_PATH_RELATIVE, (const uint8_t *)from.ptr,
                                         (uint64_t)from.len, (const uint8_t *)to.ptr,
                                         (uint64_t)to.len, from.len * 3 + to.len + 1, result,
                                         thrown);
    }
    /* `join` concatenates every argument with a separator and normalizes the result -- the empty
     * ones included, which is the interpreter's arithmetic exactly. Skipping an empty argument
     * here instead is what made `join("a", "")` answer "a" where the interpreter answers "a/",
     * and dropping the leading separator with it made `join("/a", "b/")` answer "a/b" -- a
     * relative path where an absolute one was asked for, which is the worst shape this can take.
     * `resolve` does skip the empty ones, and restarts at any argument that is itself absolute. */
    for (index = 0; index < argc; ++index) {
        kiln_value part;
        if (args[index].tag != KILN_VALUE_STRING)
            return kiln_c99_throw_not_a(thrown, args[index], "string path");
        part = args[index];
        if (code == KILN_C99_METHOD_NODE_PATH_RESOLVE) {
            if (part.len == 0) continue;
            if (part.ptr[0] == '/') {
                joined.len = 0;
                absolute = 1;
            }
            if (joined.len != 0 && joined.bytes[joined.len - 1] != '/')
                status = kiln_c99_text_append_c(&joined, "/");
        } else if (index > 0) {
            status = kiln_c99_text_append_c(&joined, "/");
        }
        if (status == KILN_C99_OK) status = kiln_c99_text_append(&joined, part.ptr, part.len);
        if (status != KILN_C99_OK) goto done;
    }
    if (absolute && (joined.len == 0 || joined.bytes[0] != '/')) {
        kiln_text prefixed = {0};
        status = kiln_c99_text_append_c(&prefixed, "/");
        if (status == KILN_C99_OK)
            status = kiln_c99_text_append(&prefixed, joined.bytes, joined.len);
        free(joined.bytes);
        joined = prefixed;
        if (status != KILN_C99_OK) goto done;
    }
    /* ...and then the same normalizer the interpreter calls, across the provider boundary. Both
     * of these are "assemble a string, then normalize it" in the interpreter, and the assembly is
     * concatenation rather than path arithmetic -- so this crosses the ABI exactly once, for the
     * half that is an algorithm, and keeps ADR 0048 true for the whole of `node:path` rather than
     * for four of its members. The bound is the same one the decompositions use: normalizing
     * never adds more than the "." an empty path answers with. */
    status = kiln_c99_node_path_answer(KILN_C99_NODE_PATH_NORMALIZE, (const uint8_t *)joined.bytes,
                                       (uint64_t)joined.len, NULL, 0, joined.len + 1, result,
                                       thrown);
done:
    free(joined.bytes);
    return status;
}

static kiln_value kiln_c99_undefined(void) {
    return kiln_c99_value(KILN_VALUE_UNDEFINED, 0, NULL, 0);
}

static kiln_object *kiln_c99_object_of(kiln_value value) {
    return (kiln_object *)(uintptr_t)value.bits;
}

static kiln_array *kiln_c99_array_of(kiln_value value) {
    return (kiln_array *)(uintptr_t)value.bits;
}

static kiln_typed_buffer *kiln_c99_typed_buffer_of(kiln_value value) {
    return (kiln_typed_buffer *)(uintptr_t)value.bits;
}

static kiln_typed_array *kiln_c99_typed_array_of(kiln_value value) {
    return (kiln_typed_array *)(uintptr_t)value.bits;
}

static kiln_data_view *kiln_c99_data_view_of(kiln_value value) {
    return (kiln_data_view *)(uintptr_t)value.bits;
}
