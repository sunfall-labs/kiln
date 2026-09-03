
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
static kiln_value *kiln_c99_cell_of(kiln_value value);
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
static int kiln_c99_new_array(kiln_value *out);
static int kiln_c99_new_abort_signal(kiln_value *out);
static int kiln_c99_new_abort_controller(kiln_value *out);
static int kiln_c99_new_text_encoder(kiln_value *out);
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
static int kiln_c99_store_get(kiln_host *host, kiln_value target, const char *key, size_t key_len,
                              kiln_value *out, int depth, kiln_value *thrown, int *found);
static int kiln_c99_get_own_kind(kiln_host *host, kiln_value target, const char *key,
                                 size_t key_len, const char *ikey, kiln_value *out, int depth,
                                 kiln_value *thrown);
static int kiln_c99_array_push(kiln_value target, kiln_value value);
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
                            int rejected);
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
        *result = queue->elements[0];
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

/* ---------------------------------------------------------------------------------------------
 * The collector (ADR 0041).
 *
 * Every allocation a JavaScript value can point at, or that holds one, comes from
 * `kiln_c99_gc_alloc`. It carries a header naming its kind, its mark epoch and its size, is
 * linked into one heap shared by every translation unit of the program, and is entered in a hash
 * set of block addresses. Marking is precise about edges and paranoid about targets: a candidate
 * pointer is marked only when the set knows it, so a string that points at a constant table, a
 * symbol that points at the well-known table, or an allocation site still on plain `malloc` is
 * ignored rather than freed.
 *
 * Collection runs only at the top of the realm's drive loop, where no generated frame is on the
 * stack (ADR 0041 says why that is enough for a server and not yet enough for a CPU-bound body).
 * The runtime archive calls `collect` through the hooks below; it never reads a C layout. It
 * enumerates the roots it owns by handing pointers and value slots back to `mark_block` and
 * `mark_value`, and each translation unit marks its own statics through a registered root
 * function.
 *
 * The heap struct's first member is the hooks pointer. The runtime reads exactly that one word
 * through the otherwise opaque heap pointer, so a root function outside the prelude (the graph
 * launcher) can mark through `kiln_realm_gc_mark_value`.
 * ------------------------------------------------------------------------------------------- */

typedef enum kiln_gc_kind {
    KILN_GC_NONE = 0,
    KILN_GC_STRING,
    KILN_GC_BYTES,
    KILN_GC_OBJECT,
    KILN_GC_CLOSURE,
    KILN_GC_CELL,
    KILN_GC_ARRAY,
    KILN_GC_GENERATOR,
    KILN_GC_PROMISE,
    KILN_GC_SETTLER,
    KILN_GC_BOUND,
    KILN_GC_COLLECTION,
    KILN_GC_PROXY,
    KILN_GC_DATE,
    KILN_GC_REGEXP,
    KILN_GC_SYMBOL,
    KILN_GC_TYPED_BUFFER,
    KILN_GC_TYPED_ARRAY,
    /* A DataView holds one edge -- the buffer it borrows -- and owns no growth buffer of its own,
     * so it is traced and never finalised. The bytes belong to KILN_GC_TYPED_BUFFER, which is the
     * one place they are released. */
    KILN_GC_DATA_VIEW,
    KILN_GC_COMMONJS_REQUIRE,
    KILN_GC_METHOD_STORE,
    KILN_GC_NODE_SERVER,
    KILN_GC_NODE_SOCKET,
    KILN_GC_NODE_CLIENT_REQUEST,
    KILN_GC_NODE_RESPONSE,
    KILN_GC_NODE_COMPLETION,
    KILN_GC_NODE_FS_COMPLETION,
    KILN_GC_MICROTASK,
    KILN_GC_NEXT_TICK,
    KILN_GC_TIMER,
    KILN_GC_KIND_COUNT
} kiln_gc_kind;

#define KILN_GC_PHASE_MARK 0
#define KILN_GC_PHASE_TEARDOWN 1
#define KILN_GC_PHASE_RESET 2
#define KILN_GC_ROOT_SURVIVES_TEARDOWN 1u
#define KILN_GC_HOOKS_VERSION 2u
#define KILN_GC_DEFAULT_FLOOR ((size_t)256 * 1024)
/* What the `introspect` hook reports back, as a bitmask. `ARMED` is what tells the drive to keep
 * its park short enough to notice a signal; `DUMPED` says a request was pending and was served. */
#define KILN_C99_INTROSPECT_ARMED 1
#define KILN_C99_INTROSPECT_DUMPED 2

/* `kind` carries `kiln_gc_kind` in its low bits and, for a `KILN_GC_STRING` block only, a cached
 * answer to "is this string all ASCII" (ADR 0022: ASCII is exactly the case where a code-unit
 * index equals a byte offset, which is what every position-taking string operation walks UTF-8 to
 * find) in its top two. `KILN_GC_KIND_COUNT` is nowhere near the mask's ~30 low bits, so the two
 * high bits are free; every reader of `block->kind` as a `kiln_gc_kind` must mask with
 * `KILN_GC_KIND_MASK` first. */
#define KILN_GC_KIND_MASK ((uint32_t)0x3FFFFFFFu)
#define KILN_GC_FLAG_ASCII_KNOWN ((uint32_t)0x80000000u)
#define KILN_GC_FLAG_ASCII ((uint32_t)0x40000000u)

typedef struct kiln_gc_block {
    struct kiln_gc_block *prev;
    struct kiln_gc_block *next;
    size_t size;
    uint32_t kind;
    uint32_t mark;
} kiln_gc_block;

/* Small-object slabs (perf/gc-slab-allocator). Most turns allocate a dozen blocks under 128
 * bytes; routing those through `malloc`/`free` plus a hash-table entry each was the allocator's
 * whole cost. A slab is one size-aligned allocation holding many same-size slots, so `kiln_c99_gc_lookup`
 * can answer membership by masking to the slab's base and reading its header instead of probing a
 * hash table sized by the live set. The block header above is unchanged and is what every slot
 * holds, so the tracer and finaliser never know which path a block came from. Alignment makes
 * "is this address in one of my slabs" a value comparison (`kiln_gc_ptrset_contains` on the slab
 * base) before anything derived from an untrusted candidate pointer is dereferenced -- marking
 * still never trusts a pointer (ADR 0041); the value check is the fail-safe the hash table gave
 * per block, done once per slab instead. */
#define KILN_GC_SLAB_SIZE ((size_t)64 * 1024)
#define KILN_GC_SLAB_CLASS_MIN ((size_t)48) /* smallest total slot (header + 1 byte payload) */
#define KILN_GC_SLAB_CLASS_MAX ((size_t)256)
#define KILN_GC_SLAB_CLASS_STEP ((size_t)16)
#define KILN_GC_SLOT_FREE ((size_t)-1) /* block->size sentinel: this slot is on the free list */

typedef struct kiln_gc_slab {
    struct kiln_gc_slab *all_prev;  /* heap-wide list of every slab, for sweep and teardown */
    struct kiln_gc_slab *all_next;
    struct kiln_gc_slab *free_prev; /* per-class list of slabs with >=1 free slot */
    struct kiln_gc_slab *free_next;
    kiln_gc_block *free_head;       /* free list threaded through freed slots' `next` field */
    uint32_t class_index;
    uint32_t slot_size;             /* bytes per slot: header + this class's payload, rounded */
    uint32_t slot_count;
    uint32_t free_count;
    uint32_t first_slot_offset;     /* bytes from the slab base to slot 0 */
} kiln_gc_slab;

/* A tiny open-addressed set of pointer values, shared by the large-block table (keys are block
 * headers) and the slab table (keys are slab base addresses). Lookup on a hit only ever compares
 * pointer values already in the set; nothing derived from an untrusted candidate is read until a
 * match confirms the memory is ours. */
typedef struct kiln_gc_ptrset {
    void **table;
    size_t capacity;
    size_t count;
} kiln_gc_ptrset;

/* Interned property keys (ADR 0047). One entry per distinct key spelling the realm has ever
 * stored or registered, mapping the bytes to the one pointer every property with that name
 * carries -- so `property->key == key` is the whole of a key comparison and a linear scan never
 * touches key bytes. The table is the heap's because the heap is the one struct every
 * translation unit shares: two units each keeping their own would have two canonical pointers
 * for one name, which is exactly the situation this exists to end.
 *
 * An entry is *pinned* when its bytes are immortal (a unit's constant table, a prelude literal,
 * a symbol's reserved key) or when the table itself has undertaken to keep a block alive because
 * a unit registered the same spelling after a computed key had already claimed it. Every other
 * entry is weak: its bytes are a string block that the properties holding the key keep marked,
 * and the collection that finds the block unmarked drops the entry before the sweep frees it. */
typedef struct kiln_key_entry {
    const char *bytes; /* NULL when the slot is empty */
    size_t len;
    uint64_t hash;
    int pinned;
} kiln_key_entry;

typedef struct kiln_key_table {
    kiln_key_entry *entries;
    size_t capacity; /* zero or a power of two */
    size_t count;
} kiln_key_table;

/* The literal keys one request costs, every time: the request and response objects, and the
 * `URL` a router builds from `req.url`.
 *
 * Interning is a hash of the bytes and a probe. These are the same twenty-odd strings on every
 * request, so ADR 0047's own "next step" applies: intern each once per heap and let the per-request
 * cost become an array index. The X-macro keeps the spelling and its length in one place, because
 * a key whose length disagrees with its text is a bug that hashes cleanly and looks up nothing.
 */
#define KILN_C99_HOT_KEYS(X)                     \
    X(METHOD, "method")                           \
    X(URL, "url")                                 \
    X(HTTP_VERSION, "httpVersion")                \
    X(HTTP_VERSION_MAJOR, "httpVersionMajor")     \
    X(HTTP_VERSION_MINOR, "httpVersionMinor")     \
    X(ABORTED, "aborted")                         \
    X(COMPLETE, "complete")                       \
    X(DESTROYED, "destroyed")                     \
    X(SOCKET, "socket")                           \
    X(CONNECTION, "connection")                   \
    X(HEADERS, "headers")                         \
    X(STATUS_CODE, "statusCode")                  \
    X(PATHNAME, "pathname")                       \
    X(GET, "get")                                 \
    X(SEARCH_PARAMS, "searchParams")

#define KILN_C99_HOT_KEY_ENUM(id, text) KILN_C99_HOT_KEY_##id,
enum kiln_c99_http_key {
    KILN_C99_HOT_KEYS(KILN_C99_HOT_KEY_ENUM) KILN_C99_HOT_KEY_COUNT
};
#undef KILN_C99_HOT_KEY_ENUM

#define KILN_C99_HOT_KEY_TEXT(id, text) text,
static const char *const kiln_c99_hot_key_text[KILN_C99_HOT_KEY_COUNT] = {
    KILN_C99_HOT_KEYS(KILN_C99_HOT_KEY_TEXT)};
#undef KILN_C99_HOT_KEY_TEXT

#define KILN_C99_HOT_KEY_LEN(id, text) sizeof(text) - 1,
static const size_t kiln_c99_hot_key_len[KILN_C99_HOT_KEY_COUNT] = {
    KILN_C99_HOT_KEYS(KILN_C99_HOT_KEY_LEN)};
#undef KILN_C99_HOT_KEY_LEN

typedef struct kiln_gc_heap {
    const kiln_gc_hooks *hooks;
    kiln_gc_block *head; /* large blocks only; slab blocks are found by walking slabs */
    size_t count;
    size_t bytes;
    size_t since_collection;
    size_t survived;
    size_t floor;
    uint32_t epoch;
    int enabled;
    int stress;
    int trace;
    int collecting;
    int mark_failed;
    int fallback;
    kiln_gc_ptrset large_table;
    kiln_gc_ptrset slab_table;
    kiln_gc_slab *all_slabs;
    kiln_gc_slab *class_free_head[(KILN_GC_SLAB_CLASS_MAX - KILN_GC_SLAB_CLASS_MIN) /
                                       KILN_GC_SLAB_CLASS_STEP +
                                   1];
    kiln_gc_block **stack;
    size_t stack_len;
    size_t stack_capacity;
    uint64_t collections;
    uint64_t freed_blocks;
    uint64_t freed_bytes;
    kiln_key_table keys;
    /* Canonical pointers for the keys the HTTP path writes on every request and response. See
     * KILN_C99_HOT_KEYS. NULL means "not interned in this heap yet"; the table is per-heap
     * because a canonical pointer from one realm means nothing in another. */
    const char *hot_keys[KILN_C99_HOT_KEY_COUNT];
} kiln_gc_heap;

static const kiln_gc_hooks kiln_c99_gc_hooks;
/* The heap a unit uses when no realm is active: the prelude's own tests. It never collects. */
static kiln_gc_heap kiln_c99_gc_fallback;

static void kiln_c99_gc_heap_init(kiln_gc_heap *heap) {
    const char *setting;
    memset(heap, 0, sizeof *heap);
    heap->hooks = &kiln_c99_gc_hooks;
    heap->floor = KILN_GC_DEFAULT_FLOOR;
    heap->enabled = 1;
    setting = getenv("KILN_GC");
    if (setting != NULL && setting[0] == '0' && setting[1] == '\0') heap->enabled = 0;
    setting = getenv("KILN_GC_STRESS");
    heap->stress = setting != NULL && setting[0] != '\0' && setting[0] != '0';
    setting = getenv("KILN_GC_TRACE");
    heap->trace = setting != NULL && setting[0] != '\0' && setting[0] != '0';
    setting = getenv("KILN_GC_FLOOR_KIB");
    if (setting != NULL && setting[0] >= '0' && setting[0] <= '9') {
        size_t kib = 0;
        while (*setting >= '0' && *setting <= '9' && kib < ((size_t)-1) / 10240) {
            kib = kib * 10 + (size_t)(*setting - '0');
            ++setting;
        }
        heap->floor = kib * 1024;
    }
}

/* Diagnostics stay off the locale-sensitive printf family (decision recorded beside
 * kiln_number_to_js_string): a count is spelled here by hand. */
static void kiln_c99_gc_trace_text(const char *text) { fputs(text, stderr); }

static void kiln_c99_gc_trace_count(uint64_t count) {
    char digits[24];
    size_t at = sizeof digits;
    digits[--at] = '\0';
    do {
        digits[--at] = (char)('0' + (int)(count % 10));
        count /= 10;
    } while (count != 0 && at != 0);
    fputs(&digits[at], stderr);
}

/* ---- SIGUSR1 runtime introspection ------------------------------------------------------
 *
 * A compiled server has no REPL, no inspector and no admin port, so `kill -USR1` is how a
 * production process is asked what it is doing. It answers with three lines on stderr and keeps
 * serving.
 *
 * The handler does the one thing a handler may do -- assign to a `volatile sig_atomic_t` -- and
 * the dump runs at the next turn boundary, the safepoint ADR 0041 already collects at, where no
 * generated frame is on the stack. Deferring is not merely stylish here: the kernel runs a
 * process-directed signal on whichever thread has not blocked it, Tokio's blocking pool included,
 * so the handler may be on a thread that has never touched this heap. Setting a process-global
 * flag is sound from there; reading the heap would not be. One realm per process (KILN_WORKERS
 * forks rather than threads) is what makes the flag unambiguous, but it does not make the
 * delivering thread predictable.
 *
 * The formatter below obeys the async-signal-safe rules anyway -- a stack buffer, hand-rolled
 * base-ten spelling, one write(2) per line, no malloc and no stdio. Only the `heap` line could
 * actually be produced from the handler; `runtime` and `connections` cross into the archive. The
 * discipline is kept because stdio is the wrong tool here for the locale reason recorded beside
 * kiln_number_to_js_string, and because one write per line means a dump racing another writer on
 * fd 2 interleaves lines rather than characters. */

/* Fields kiln_realm_runtime_report fills, in its documented order. */
#define KILN_C99_RUNTIME_REPORT_FIELDS 9

#if defined(__unix__) || defined(__APPLE__)

/* Set by the handler on an arbitrary thread, read and cleared at the turn boundary. Process
 * state, not realm state: it names a request that has arrived, and it outlives any one realm. */
static volatile sig_atomic_t kiln_c99_introspect_pending;
static int kiln_c99_introspect_armed;

static void kiln_c99_introspect_handler(int signal_number) {
    (void)signal_number;
    kiln_c99_introspect_pending = 1;
}

/* SA_RESTART so an interrupted accept or read resumes: a diagnostic must not be able to fail the
 * server it is diagnosing with EINTR. */
static int kiln_c99_introspect_arm(void) {
    struct sigaction action;
    const char *setting = getenv("KILN_INTROSPECT");
    if (setting != NULL && setting[0] == '0' && setting[1] == '\0') return 0;
    memset(&action, 0, sizeof action);
    action.sa_handler = kiln_c99_introspect_handler;
    sigemptyset(&action.sa_mask);
    action.sa_flags = SA_RESTART;
    return sigaction(SIGUSR1, &action, NULL) == 0;
}

typedef struct kiln_c99_introspect_line {
    char bytes[512];
    size_t len;
} kiln_c99_introspect_line;

static void kiln_c99_introspect_text(kiln_c99_introspect_line *line, const char *text) {
    while (*text != '\0' && line->len + 1 < sizeof line->bytes) line->bytes[line->len++] = *text++;
}

static void kiln_c99_introspect_number(kiln_c99_introspect_line *line, uint64_t value) {
    char digits[24];
    size_t at = sizeof digits;
    do {
        digits[--at] = (char)('0' + (int)(value % 10));
        value /= 10;
    } while (value != 0 && at != 0);
    while (at < sizeof digits && line->len + 1 < sizeof line->bytes)
        line->bytes[line->len++] = digits[at++];
}

static void kiln_c99_introspect_field(kiln_c99_introspect_line *line, const char *name,
                                      uint64_t value) {
    kiln_c99_introspect_text(line, " ");
    kiln_c99_introspect_text(line, name);
    kiln_c99_introspect_text(line, "=");
    kiln_c99_introspect_number(line, value);
}

/* The pid leads every line because KILN_WORKERS forks: a signal sent to the process group
 * produces one three-line group per worker, and they have to be separable. */
static void kiln_c99_introspect_begin(kiln_c99_introspect_line *line, const char *section) {
    line->len = 0;
    kiln_c99_introspect_text(line, "kiln introspect pid=");
    kiln_c99_introspect_number(line, (uint64_t)getpid());
    kiln_c99_introspect_text(line, " ");
    kiln_c99_introspect_text(line, section);
}

/* One write per line. A short write is retried a bounded number of times rather than by reading
 * errno, which a handler would have to save and restore. */
static void kiln_c99_introspect_flush(kiln_c99_introspect_line *line) {
    size_t at = 0;
    int retries = 0;
    if (line->len < sizeof line->bytes) line->bytes[line->len++] = '\n';
    else line->bytes[sizeof line->bytes - 1] = '\n';
    while (at < line->len) {
        ssize_t written = write(2, line->bytes + at, line->len - at);
        if (written < 0) {
            if (++retries > 8) break;
            continue;
        }
        if (written == 0) break;
        at += (size_t)written;
    }
}

static void kiln_c99_introspect_dump(kiln_gc_heap *heap) {
    kiln_c99_introspect_line line;
    uint64_t fields[KILN_C99_RUNTIME_REPORT_FIELDS];
    uint64_t reported = 0;
    size_t index;

    kiln_c99_introspect_begin(&line, "heap");
    kiln_c99_introspect_field(&line, "bytes", (uint64_t)heap->bytes);
    kiln_c99_introspect_field(&line, "count", (uint64_t)heap->count);
    kiln_c99_introspect_field(&line, "since_collection", (uint64_t)heap->since_collection);
    kiln_c99_introspect_field(&line, "survived", (uint64_t)heap->survived);
    kiln_c99_introspect_field(&line, "floor", (uint64_t)heap->floor);
    kiln_c99_introspect_field(&line, "epoch", (uint64_t)heap->epoch);
    kiln_c99_introspect_field(&line, "collections", heap->collections);
    kiln_c99_introspect_field(&line, "freed_bytes", heap->freed_bytes);
    kiln_c99_introspect_field(&line, "enabled", (uint64_t)(heap->enabled != 0));
    kiln_c99_introspect_field(&line, "collecting", (uint64_t)(heap->collecting != 0));
    kiln_c99_introspect_flush(&line);

    for (index = 0; index < KILN_C99_RUNTIME_REPORT_FIELDS; ++index) fields[index] = 0;
    if (kiln_realm_runtime_report(fields, (uint64_t)KILN_C99_RUNTIME_REPORT_FIELDS, &reported) !=
        KILN_C99_OK) {
        return;
    }
    /* Fields 0-5 are interval counters covering the time since the previous dump; 6-8 are
     * instantaneous. A shorter report than this prelude knows about prints the prefix it got. */
    if (reported >= 6) {
        kiln_c99_introspect_begin(&line, "runtime");
        kiln_c99_introspect_field(&line, "live_tasks", fields[0]);
        kiln_c99_introspect_field(&line, "park_count", fields[1]);
        kiln_c99_introspect_field(&line, "busy_us", fields[2]);
        kiln_c99_introspect_field(&line, "elapsed_us", fields[3]);
        kiln_c99_introspect_field(&line, "queue_depth", fields[4]);
        kiln_c99_introspect_field(&line, "workers", fields[5]);
        kiln_c99_introspect_flush(&line);
    }
    if (reported >= 9) {
        kiln_c99_introspect_begin(&line, "connections");
        kiln_c99_introspect_field(&line, "server", fields[6]);
        kiln_c99_introspect_field(&line, "client", fields[7]);
        kiln_c99_introspect_field(&line, "requests_in_flight", fields[8]);
        kiln_c99_introspect_flush(&line);
    }
}

static int kiln_c99_gc_hook_introspect(void *opaque) {
    kiln_gc_heap *heap = (kiln_gc_heap *)opaque;
    int status;
    if (heap == NULL || heap->fallback) return 0;
    status = kiln_c99_introspect_armed ? KILN_C99_INTROSPECT_ARMED : 0;
    if (kiln_c99_introspect_pending) {
        kiln_c99_introspect_pending = 0;
        kiln_c99_introspect_dump(heap);
        status |= KILN_C99_INTROSPECT_DUMPED;
    }
    return status;
}

#else

/* A target without POSIX signals compiles the same prelude with the dump inert; the two statics
 * stay declared so the ownership guard sees one shape on every platform. */
static volatile int kiln_c99_introspect_pending;
static int kiln_c99_introspect_armed;

static int kiln_c99_introspect_arm(void) { return 0; }

static int kiln_c99_gc_hook_introspect(void *opaque) {
    (void)opaque;
    (void)kiln_c99_introspect_pending;
    return 0;
}

#endif

static kiln_gc_heap *kiln_c99_gc_heap(void) {
    void *attached = kiln_realm_gc_current_heap();
    kiln_gc_heap *candidate;
    int status;
    if (attached != NULL) return (kiln_gc_heap *)attached;
    status = kiln_realm_gc_attach(&kiln_c99_gc_hooks, NULL, &attached);
    if (status == KILN_C99_OK && attached != NULL) return (kiln_gc_heap *)attached;
    if (status != KILN_C99_OK) {
        if (kiln_c99_gc_fallback.hooks == NULL) {
            kiln_c99_gc_heap_init(&kiln_c99_gc_fallback);
            kiln_c99_gc_fallback.enabled = 0;
            kiln_c99_gc_fallback.fallback = 1;
        }
        return &kiln_c99_gc_fallback;
    }
    candidate = (kiln_gc_heap *)malloc(sizeof *candidate);
    if (candidate == NULL) return NULL;
    kiln_c99_gc_heap_init(candidate);
    status = kiln_realm_gc_attach(&kiln_c99_gc_hooks, candidate, &attached);
    if (status != KILN_C99_OK || attached == NULL) {
        free(candidate);
        return NULL;
    }
    if (attached != candidate) free(candidate);
    /* Arm here, in the unit whose candidate won: the heap now carries *this* unit's static
     * kiln_c99_gc_hooks, so the `introspect` hook the archive calls reads the same copy of
     * kiln_c99_introspect_pending that this unit's handler writes. The prelude is emitted per
     * unit with internal linkage, so arming anywhere else would arm one flag and poll another. */
    else kiln_c99_introspect_armed = kiln_c99_introspect_arm();
    return (kiln_gc_heap *)attached;
}

static size_t kiln_c99_gc_hash(const void *pointer, size_t mask) {
    uint64_t bits = (uint64_t)(uintptr_t)pointer;
    bits ^= bits >> 29;
    bits *= UINT64_C(0xBF58476D1CE4E5B9);
    bits ^= bits >> 32;
    return (size_t)bits & mask;
}

static int kiln_c99_gc_ptrset_grow(kiln_gc_ptrset *set, size_t wanted) {
    void **grown;
    void **old = set->table;
    size_t old_capacity = set->capacity;
    size_t index;
    size_t capacity = 64;
    while (capacity < wanted * 2) {
        if (capacity > ((size_t)-1) / 4) return KILN_C99_OUT_OF_MEMORY;
        capacity *= 2;
    }
    grown = (void **)calloc(capacity, sizeof *grown);
    if (grown == NULL) return KILN_C99_OUT_OF_MEMORY;
    set->table = grown;
    set->capacity = capacity;
    set->count = 0;
    for (index = 0; index < old_capacity; ++index) {
        void *entry = old[index];
        if (entry != NULL) {
            size_t slot = kiln_c99_gc_hash(entry, capacity - 1);
            while (grown[slot] != NULL) slot = (slot + 1) & (capacity - 1);
            grown[slot] = entry;
            ++set->count;
        }
    }
    free(old);
    return KILN_C99_OK;
}

static int kiln_c99_gc_ptrset_insert(kiln_gc_ptrset *set, void *pointer) {
    size_t slot;
    size_t mask;
    if ((set->count + 1) * 2 > set->capacity) {
        int status = kiln_c99_gc_ptrset_grow(set, set->count + 1);
        if (status != KILN_C99_OK) return status;
    }
    mask = set->capacity - 1;
    slot = kiln_c99_gc_hash(pointer, mask);
    while (set->table[slot] != NULL) slot = (slot + 1) & mask;
    set->table[slot] = pointer;
    ++set->count;
    return KILN_C99_OK;
}

/* Linear probing with backward-shift deletion, so a release leaves no tombstone behind. */
static void kiln_c99_gc_ptrset_remove(kiln_gc_ptrset *set, const void *pointer) {
    size_t mask;
    size_t slot;
    size_t hole;
    if (set->capacity == 0) return;
    mask = set->capacity - 1;
    slot = kiln_c99_gc_hash(pointer, mask);
    while (set->table[slot] != pointer) {
        if (set->table[slot] == NULL) return;
        slot = (slot + 1) & mask;
    }
    hole = slot;
    for (;;) {
        size_t home;
        slot = (slot + 1) & mask;
        if (set->table[slot] == NULL) break;
        home = kiln_c99_gc_hash(set->table[slot], mask);
        /* Move the entry back into the hole when its home sits at or before the hole in probe
         * order, otherwise a lookup that started at its home would stop at the hole. */
        if ((slot > hole && (home <= hole || home > slot)) ||
            (slot < hole && home <= hole && home > slot)) {
            set->table[hole] = set->table[slot];
            hole = slot;
        }
    }
    set->table[hole] = NULL;
    --set->count;
}

/* Value-only membership: never dereferences `pointer`, only compares it against addresses this
 * heap itself inserted. This is what makes masking an untrusted candidate down to a slab base
 * safe to then dereference -- the slab header is read only after this returns true. */
static int kiln_c99_gc_ptrset_contains(const kiln_gc_ptrset *set, const void *pointer) {
    size_t mask;
    size_t slot;
    if (pointer == NULL || set->capacity == 0) return 0;
    mask = set->capacity - 1;
    slot = kiln_c99_gc_hash(pointer, mask);
    while (set->table[slot] != NULL) {
        if (set->table[slot] == pointer) return 1;
        slot = (slot + 1) & mask;
    }
    return 0;
}

/* Portable aligned allocation for slabs: only <stdlib.h> `malloc`/`free` are assumed (the
 * prelude is compiled `-std=c11`, and `posix_memalign` is not reliably declared there on every
 * target), so the raw pointer is stashed just before the aligned region it returns. */
static void *kiln_c99_gc_aligned_alloc(size_t alignment, size_t size) {
    unsigned char *raw = (unsigned char *)malloc(size + alignment - 1 + sizeof(void *));
    uintptr_t base;
    unsigned char *aligned;
    if (raw == NULL) return NULL;
    base = (uintptr_t)(raw + sizeof(void *));
    aligned = (unsigned char *)((base + alignment - 1) & ~(uintptr_t)(alignment - 1));
    ((void **)aligned)[-1] = raw;
    return aligned;
}

static void kiln_c99_gc_aligned_free(void *aligned) {
    if (aligned == NULL) return;
    free(((void **)aligned)[-1]);
}

/* Which size class (if any) holds a block whose header plus payload totals `total` bytes; -1
 * means the block is large and keeps the `malloc` path. */
static int kiln_c99_gc_slab_class(size_t total) {
    size_t rounded;
    if (total > KILN_GC_SLAB_CLASS_MAX) return -1;
    rounded = total <= KILN_GC_SLAB_CLASS_MIN
                  ? KILN_GC_SLAB_CLASS_MIN
                  : (total + (KILN_GC_SLAB_CLASS_STEP - 1)) &
                        ~(KILN_GC_SLAB_CLASS_STEP - 1);
    if (rounded > KILN_GC_SLAB_CLASS_MAX) return -1;
    return (int)((rounded - KILN_GC_SLAB_CLASS_MIN) / KILN_GC_SLAB_CLASS_STEP);
}

static kiln_gc_block *kiln_c99_gc_slab_slot(const kiln_gc_slab *slab, uint32_t index) {
    return (kiln_gc_block *)((char *)slab + slab->first_slot_offset +
                              (size_t)index * slab->slot_size);
}

static void kiln_c99_gc_slab_unlink_free(kiln_gc_heap *heap, kiln_gc_slab *slab) {
    if (slab->free_prev != NULL) slab->free_prev->free_next = slab->free_next;
    else heap->class_free_head[slab->class_index] = slab->free_next;
    if (slab->free_next != NULL) slab->free_next->free_prev = slab->free_prev;
    slab->free_prev = NULL;
    slab->free_next = NULL;
}

static void kiln_c99_gc_slab_link_free(kiln_gc_heap *heap, kiln_gc_slab *slab) {
    slab->free_prev = NULL;
    slab->free_next = heap->class_free_head[slab->class_index];
    if (heap->class_free_head[slab->class_index] != NULL)
        heap->class_free_head[slab->class_index]->free_prev = slab;
    heap->class_free_head[slab->class_index] = slab;
}

static void kiln_c99_gc_slab_unlink_all(kiln_gc_heap *heap, kiln_gc_slab *slab) {
    if (slab->all_prev != NULL) slab->all_prev->all_next = slab->all_next;
    else heap->all_slabs = slab->all_next;
    if (slab->all_next != NULL) slab->all_next->all_prev = slab->all_prev;
    slab->all_prev = NULL;
    slab->all_next = NULL;
}

/* A slab that is entirely free is destroyed immediately unless it is the only supply left for
 * its class (`free_prev`/`free_next` both NULL): that bounds retention to about one empty slab
 * per class without forcing a fresh slab on the very next allocation of that class. */
static void kiln_c99_gc_slab_maybe_destroy(kiln_gc_heap *heap, kiln_gc_slab *slab) {
    if (slab->free_prev == NULL && slab->free_next == NULL) return;
    kiln_c99_gc_slab_unlink_free(heap, slab);
    kiln_c99_gc_slab_unlink_all(heap, slab);
    kiln_c99_gc_ptrset_remove(&heap->slab_table, slab);
    kiln_c99_gc_aligned_free(slab);
}

static kiln_gc_slab *kiln_c99_gc_slab_new(kiln_gc_heap *heap, int class_index) {
    kiln_gc_slab *slab;
    size_t slot_size = KILN_GC_SLAB_CLASS_MIN + (size_t)class_index * KILN_GC_SLAB_CLASS_STEP;
    size_t first_slot_offset = sizeof(kiln_gc_slab);
    size_t usable;
    size_t slot_count;
    uint32_t index;
    if (first_slot_offset % sizeof(void *) != 0)
        first_slot_offset += sizeof(void *) - (first_slot_offset % sizeof(void *));
    if (first_slot_offset >= KILN_GC_SLAB_SIZE) return NULL;
    usable = KILN_GC_SLAB_SIZE - first_slot_offset;
    slot_count = usable / slot_size;
    if (slot_count == 0) return NULL;
    slab = (kiln_gc_slab *)kiln_c99_gc_aligned_alloc(KILN_GC_SLAB_SIZE, KILN_GC_SLAB_SIZE);
    if (slab == NULL) return NULL;
    slab->class_index = (uint32_t)class_index;
    slab->slot_size = (uint32_t)slot_size;
    slab->slot_count = (uint32_t)slot_count;
    slab->free_count = (uint32_t)slot_count;
    slab->first_slot_offset = (uint32_t)first_slot_offset;
    slab->free_head = NULL;
    for (index = 0; index < (uint32_t)slot_count; ++index) {
        kiln_gc_block *slot = kiln_c99_gc_slab_slot(slab, index);
        slot->size = KILN_GC_SLOT_FREE;
        slot->kind = KILN_GC_NONE;
        slot->mark = 0;
        slot->prev = NULL;
        slot->next = slab->free_head;
        slab->free_head = slot;
    }
    if (kiln_c99_gc_ptrset_insert(&heap->slab_table, slab) != KILN_C99_OK) {
        kiln_c99_gc_aligned_free(slab);
        return NULL;
    }
    slab->all_prev = NULL;
    slab->all_next = heap->all_slabs;
    if (heap->all_slabs != NULL) heap->all_slabs->all_prev = slab;
    heap->all_slabs = slab;
    slab->free_prev = NULL;
    slab->free_next = heap->class_free_head[class_index];
    if (heap->class_free_head[class_index] != NULL)
        heap->class_free_head[class_index]->free_prev = slab;
    heap->class_free_head[class_index] = slab;
    return slab;
}

/* The one membership check every candidate edge pays for (ADR 0041: marking never trusts a
 * pointer). Masking to a slab base and reading its header is safe only because that read happens
 * after `kiln_gc_ptrset_contains` confirms the exact address value is one this heap inserted;
 * nothing derived from the untrusted `payload` is dereferenced before that. A slab hit still
 * checks the slot is exactly a payload start and not free, so an interior or stale pointer is
 * rejected the same way a foreign pointer always was. */
static kiln_gc_block *kiln_c99_gc_lookup_slab(const kiln_gc_heap *heap, const void *payload,
                                              kiln_gc_slab **out_slab) {
    uintptr_t address = (uintptr_t)payload;
    void *slab_base;
    if (out_slab != NULL) *out_slab = NULL;
    /* A candidate edge is not necessarily a pointer at all: `bits` and `ptr` share one word, so
     * a builtin's code or a method's code arrives here as a small integer. Stepping back from it
     * with pointer arithmetic is undefined behaviour on a pointer that never pointed at an
     * object, and the sanitizer traps it -- so addresses are compared and masked as integers and
     * anything that cannot be a payload is rejected before a pointer exists. */
    if (payload == NULL || address < sizeof(kiln_gc_block)) return NULL;
    slab_base = (void *)(address & ~(uintptr_t)(KILN_GC_SLAB_SIZE - 1));
    if (kiln_c99_gc_ptrset_contains(&heap->slab_table, slab_base)) {
        const kiln_gc_slab *slab = (const kiln_gc_slab *)slab_base;
        size_t offset = address - (uintptr_t)slab_base;
        size_t relative;
        size_t slot_index;
        kiln_gc_block *block;
        if (offset < slab->first_slot_offset) return NULL;
        relative = offset - slab->first_slot_offset;
        if (relative % slab->slot_size != sizeof(kiln_gc_block)) return NULL;
        slot_index = relative / slab->slot_size;
        if (slot_index >= slab->slot_count) return NULL;
        block = kiln_c99_gc_slab_slot(slab, (uint32_t)slot_index);
        if (block->size == KILN_GC_SLOT_FREE) return NULL;
        if (out_slab != NULL) *out_slab = (kiln_gc_slab *)slab;
        return block;
    }
    {
        /* Large block: unchanged from the pre-slab collector, an arithmetic step back to the
         * header followed by a value-only table check. */
        kiln_gc_block *block = (kiln_gc_block *)(address - sizeof(kiln_gc_block));
        if (kiln_c99_gc_ptrset_contains(&heap->large_table, block)) return block;
    }
    return NULL;
}

static kiln_gc_block *kiln_c99_gc_lookup(const kiln_gc_heap *heap, const void *payload) {
    return kiln_c99_gc_lookup_slab(heap, payload, NULL);
}

/* A branch-light ASCII test, 8 bytes at a time: ASCII is exactly the bytes with their top bit
 * clear, so a single mask-and-compare per word answers for all eight at once. The tail below
 * 8 bytes falls back to one byte at a time. */
static int kiln_c99_ascii_scan(const char *bytes, size_t len) {
    size_t index = 0;
    while (index + 8 <= len) {
        uint64_t word;
        memcpy(&word, bytes + index, sizeof word);
        if ((word & UINT64_C(0x8080808080808080)) != 0) return 0;
        index += 8;
    }
    while (index < len) {
        if (((unsigned char)bytes[index]) & 0x80u) return 0;
        ++index;
    }
    return 1;
}

/* Is this string value entirely ASCII, where a UTF-16 code unit is a byte and `length` is `len`
 * (ADR 0022)? A heap string (`kiln_c99_gc_lookup` finds its block) caches the answer in the
 * block's otherwise-unused `kind` high bits (ADR 0041) after the first scan, so a string read
 * more than once -- a repeated header name, a request path used by several route checks --
 * pays for the scan at most once. A constant or foreign pointer has no block to cache in and is
 * scanned every call; constants are short (path segments, JSON keys, literals a program wrote)
 * and the scan is cheap relative to the UTF-8 decode it replaces. */
static int kiln_c99_string_is_ascii(kiln_value value) {
    kiln_gc_heap *heap;
    kiln_gc_block *block;
    int ascii;
    if (value.len == 0) return 1;
    heap = kiln_c99_gc_heap();
    block = heap != NULL ? kiln_c99_gc_lookup(heap, value.ptr) : NULL;
    if (block != NULL && (block->kind & KILN_GC_FLAG_ASCII_KNOWN) != 0) {
        return (block->kind & KILN_GC_FLAG_ASCII) != 0;
    }
    ascii = kiln_c99_ascii_scan(value.ptr, value.len);
    if (block != NULL) {
        block->kind = (block->kind & KILN_GC_KIND_MASK) | KILN_GC_FLAG_ASCII_KNOWN |
                      (ascii ? KILN_GC_FLAG_ASCII : 0u);
    }
    return ascii;
}

static void *kiln_c99_gc_alloc(uint32_t kind, size_t size) {
    kiln_gc_heap *heap = kiln_c99_gc_heap();
    size_t payload = size == 0 ? 1 : size;
    int class_index;
    kiln_gc_block *block;
    if (heap == NULL || size > ((size_t)-1) - sizeof(kiln_gc_block)) return NULL;
    class_index = kiln_c99_gc_slab_class(sizeof(kiln_gc_block) + payload);
    if (class_index >= 0) {
        kiln_gc_slab *slab = heap->class_free_head[class_index];
        if (slab == NULL) {
            slab = kiln_c99_gc_slab_new(heap, class_index);
            if (slab == NULL) return NULL;
        }
        block = slab->free_head;
        slab->free_head = block->next;
        block->next = NULL;
        if (--slab->free_count == 0) kiln_c99_gc_slab_unlink_free(heap, slab);
        block->kind = kind;
        block->mark = 0;
        block->size = size;
        heap->count += 1;
        heap->bytes += size;
        heap->since_collection += size;
        return block + 1;
    }
    block = (kiln_gc_block *)malloc(sizeof *block + payload);
    if (block == NULL) return NULL;
    if (kiln_c99_gc_ptrset_insert(&heap->large_table, block) != KILN_C99_OK) {
        free(block);
        return NULL;
    }
    block->kind = kind;
    block->mark = 0;
    block->size = size;
    block->prev = NULL;
    block->next = heap->head;
    if (heap->head != NULL) heap->head->prev = block;
    heap->head = block;
    ++heap->count;
    heap->bytes += size;
    heap->since_collection += size;
    return block + 1;
}

static void *kiln_c99_gc_calloc(uint32_t kind, size_t size) {
    void *payload = kiln_c99_gc_alloc(kind, size);
    if (payload != NULL && size != 0) memset(payload, 0, size);
    return payload;
}

/* ---- Interned keys ------------------------------------------------------------------------
 *
 * Three entry points, and the difference between them is the whole design:
 *
 *   kiln_c99_intern        canonical pointer, inserting. The one a *write* uses.
 *   kiln_c99_intern_static canonical pointer, inserting, for bytes that never die; pins.
 *   kiln_c99_interned      canonical pointer or NULL, never inserting. The one a *read* uses,
 *                          because storage is total: a spelling the table has never seen is a
 *                          key no object anywhere has, and the read can skip the scan.
 *
 * Canonical pointers are distinct as pointers: a block is adopted as canonical only when its
 * size is exactly the key's length, so two keys can never share an address with different
 * lengths, and the empty key is one static byte rather than whichever empty string came first. */
static const char kiln_c99_key_empty[1] = "";
static void kiln_c99_gc_mark_block(kiln_gc_heap *heap, const void *payload);

/* `KILN_KEYS_CHECK=1`: every pointer-identity scan is re-run by bytes and a disagreement aborts
 * naming the key. The acceptance test for "interning is total", not a mode to ship. Read once
 * per unit rather than kept on the heap, so a compiled site that never touches the heap is
 * checked too. */
static int kiln_c99_keys_check_mode = -1;

static int kiln_c99_keys_checked(void) {
    if (kiln_c99_keys_check_mode < 0) {
        const char *setting = getenv("KILN_KEYS_CHECK");
        kiln_c99_keys_check_mode = setting != NULL && setting[0] != '\0' && setting[0] != '0';
    }
    return kiln_c99_keys_check_mode;
}

static uint64_t kiln_c99_key_hash(const char *bytes, size_t len) {
    /* Eight bytes at a time, because nearly every key is shorter than sixteen.
     *
     * FNV-1a walked a byte per iteration and each multiply waited on the one before it, so a
     * ten-character header name cost ten dependent multiplies before the probe could start.
     * Interning was 4.79% of the mixed server workload with `kiln_c99_intern` the second most
     * expensive function in the prelude, and this is most of what it was doing: the keys that
     * reach it are the ones off the wire, hashed fresh on every request.
     *
     * The mixing constant and the shift are murmur3's finaliser, which is where the avalanche
     * comes from -- the table masks the low bits, so a hash whose low bits track the input would
     * lengthen every probe chain. `len` seeds it so that keys differing only by trailing bytes
     * that fall outside the last word still separate. Nothing persists a hash: it is used to
     * probe and to rehash, both within one process, so changing it changes no artifact. */
    const uint64_t mix = UINT64_C(0xff51afd7ed558ccd);
    uint64_t hash = UINT64_C(0x9e3779b97f4a7c15) ^ (uint64_t)len;
    size_t index = 0;
    while (len - index >= 8) {
        uint64_t word;
        memcpy(&word, bytes + index, sizeof word);
        hash = (hash ^ word) * mix;
        hash ^= hash >> 29;
        index += 8;
    }
    if (index < len) {
        /* The tail goes into a zeroed word, so a shorter key never reads past its own bytes. */
        uint64_t tail = 0;
        memcpy(&tail, bytes + index, len - index);
        hash = (hash ^ tail) * mix;
        hash ^= hash >> 29;
    }
    return hash ^ (hash >> 32);
}

static kiln_key_entry *kiln_c99_key_probe(const kiln_key_table *table, const char *bytes,
                                          size_t len, uint64_t hash) {
    size_t mask;
    size_t at;
    if (table->capacity == 0) return NULL;
    mask = table->capacity - 1;
    at = (size_t)hash & mask;
    for (;;) {
        kiln_key_entry *entry = &table->entries[at];
        if (entry->bytes == NULL) return NULL;
        if (entry->hash == hash && entry->len == len &&
            (entry->bytes == bytes || memcmp(entry->bytes, bytes, len) == 0))
            return entry;
        at = (at + 1) & mask;
    }
}

static void kiln_c99_key_place(kiln_key_table *table, kiln_key_entry entry) {
    size_t mask = table->capacity - 1;
    size_t at = (size_t)entry.hash & mask;
    while (table->entries[at].bytes != NULL) at = (at + 1) & mask;
    table->entries[at] = entry;
    table->count += 1;
}

static int kiln_c99_key_grow(kiln_key_table *table) {
    kiln_key_entry *old = table->entries;
    size_t old_capacity = table->capacity;
    size_t capacity = old_capacity == 0 ? 256 : old_capacity * 2;
    kiln_key_entry *grown;
    size_t index;
    if (capacity > ((size_t)-1) / sizeof *grown) return KILN_C99_OUT_OF_MEMORY;
    grown = (kiln_key_entry *)calloc(capacity, sizeof *grown);
    if (grown == NULL) return KILN_C99_OUT_OF_MEMORY;
    table->entries = grown;
    table->capacity = capacity;
    table->count = 0;
    for (index = 0; index < old_capacity; ++index)
        if (old[index].bytes != NULL) kiln_c99_key_place(table, old[index]);
    free(old);
    return KILN_C99_OK;
}

static const char *kiln_c99_key_insert(kiln_key_table *table, const char *bytes, size_t len,
                                       uint64_t hash, int pinned) {
    kiln_key_entry entry;
    if ((table->count + 1) * 2 > table->capacity && kiln_c99_key_grow(table) != KILN_C99_OK)
        return NULL;
    entry.bytes = bytes;
    entry.len = len;
    entry.hash = hash;
    entry.pinned = pinned;
    kiln_c99_key_place(table, entry);
    return bytes;
}

/* Linear probing deletes by shifting the entries after the hole back over it: an entry moves
 * into the hole when the hole lies on the probe path from its home slot to where it sits. */
static void kiln_c99_key_delete_at(kiln_key_table *table, size_t hole) {
    size_t mask = table->capacity - 1;
    size_t at = (hole + 1) & mask;
    table->entries[hole].bytes = NULL;
    table->count -= 1;
    while (table->entries[at].bytes != NULL) {
        size_t home = (size_t)table->entries[at].hash & mask;
        if (((hole - home) & mask) < ((at - home) & mask)) {
            table->entries[hole] = table->entries[at];
            table->entries[at].bytes = NULL;
            hole = at;
        }
        at = (at + 1) & mask;
    }
}

static const char *kiln_c99_interned_in(kiln_gc_heap *heap, const char *bytes, size_t len) {
    const kiln_key_entry *entry;
    if (len == 0) return kiln_c99_key_empty;
    if (heap == NULL) return NULL;
    entry = kiln_c99_key_probe(&heap->keys, bytes, len, kiln_c99_key_hash(bytes, len));
    return entry == NULL ? NULL : entry->bytes;
}

static const char *kiln_c99_interned(const char *bytes, size_t len) {
    return kiln_c99_interned_in(kiln_c99_gc_heap(), bytes, len);
}

static const char *kiln_c99_intern(const char *bytes, size_t len) {
    kiln_gc_heap *heap;
    uint64_t hash;
    kiln_key_entry *entry;
    kiln_gc_block *block;
    const char *canonical;
    if (len == 0) return kiln_c99_key_empty;
    heap = kiln_c99_gc_heap();
    if (heap == NULL) return NULL;
    hash = kiln_c99_key_hash(bytes, len);
    entry = kiln_c99_key_probe(&heap->keys, bytes, len, hash);
    if (entry != NULL) return entry->bytes;
    /* A string block of exactly this size is adopted as the canonical bytes: the properties
     * that hold the key keep it marked, and nothing is copied. Anything else -- a constant this
     * unit did not register, a slice, foreign memory borrowed for a callback -- is copied into a
     * block of its own, because a weak entry has to name something the collector can see die. */
    block = kiln_c99_gc_lookup(heap, bytes);
    if (block != NULL && (block->kind & KILN_GC_KIND_MASK) == KILN_GC_STRING &&
        block->size == len) {
        canonical = bytes;
    } else {
        char *copy = (char *)kiln_c99_gc_alloc(KILN_GC_STRING, len);
        if (copy == NULL) return NULL;
        memcpy(copy, bytes, len);
        canonical = copy;
    }
    return kiln_c99_key_insert(&heap->keys, canonical, len, hash, 0);
}

/* For bytes that outlive the heap. NULL only when the table cannot grow; a unit that gets NULL
 * for one of its keys fails its entry with out-of-memory rather than run a site with a pointer
 * that is not canonical. */
static const char *kiln_c99_intern_static(const char *bytes, size_t len) {
    kiln_gc_heap *heap;
    uint64_t hash;
    kiln_key_entry *entry;
    if (len == 0) return kiln_c99_key_empty;
    heap = kiln_c99_gc_heap();
    if (heap == NULL) return NULL;
    hash = kiln_c99_key_hash(bytes, len);
    entry = kiln_c99_key_probe(&heap->keys, bytes, len, hash);
    if (entry != NULL) {
        entry->pinned = 1;
        return entry->bytes;
    }
    return kiln_c99_key_insert(&heap->keys, bytes, len, hash, 1);
}

/* A string block released by hand (`kiln_c99_gc_release`) must not stay canonical. */
static void kiln_c99_key_forget(kiln_gc_heap *heap, const char *bytes, size_t len) {
    kiln_key_table *table = &heap->keys;
    size_t mask;
    size_t at;
    if (table->capacity == 0) return;
    mask = table->capacity - 1;
    at = (size_t)kiln_c99_key_hash(bytes, len) & mask;
    while (table->entries[at].bytes != NULL) {
        if (table->entries[at].bytes == bytes) {
            kiln_c99_key_delete_at(table, at);
            return;
        }
        at = (at + 1) & mask;
    }
}

static void kiln_c99_key_mark_pinned(kiln_gc_heap *heap) {
    size_t index;
    for (index = 0; index < heap->keys.capacity; ++index) {
        const kiln_key_entry *entry = &heap->keys.entries[index];
        if (entry->bytes != NULL && entry->pinned) kiln_c99_gc_mark_block(heap, entry->bytes);
    }
}

/* After marking and before the sweep: drop every weak entry whose block did not survive. In
 * place, so a purge needs no memory at the one moment the collector must not fail for lack of
 * it. A slot is re-examined after a deletion because the shift moves a later entry into it. */
static void kiln_c99_key_purge(kiln_gc_heap *heap) {
    kiln_key_table *table = &heap->keys;
    size_t index = 0;
    while (index < table->capacity) {
        const kiln_key_entry *entry = &table->entries[index];
        kiln_gc_block *block;
        if (entry->bytes == NULL || entry->pinned) {
            ++index;
            continue;
        }
        block = kiln_c99_gc_lookup(heap, entry->bytes);
        if (block != NULL && block->mark != heap->epoch) {
            kiln_c99_key_delete_at(table, index);
            continue;
        }
        ++index;
    }
}

static void kiln_c99_gc_unlink(kiln_gc_heap *heap, kiln_gc_block *block) {
    if (block->prev != NULL) block->prev->next = block->next;
    else heap->head = block->next;
    if (block->next != NULL) block->next->prev = block->prev;
    block->prev = NULL;
    block->next = NULL;
    --heap->count;
    heap->bytes -= block->size;
}

static void kiln_c99_gc_finalize(kiln_gc_block *block);

/* Explicit release for a block whose lifetime is already known: queued work after it ran, a
 * host-operation completion after its callback. The collector remains the backstop. */
static void kiln_c99_gc_release(void *payload) {
    kiln_gc_heap *heap;
    kiln_gc_slab *slab = NULL;
    kiln_gc_block *block;
    if (payload == NULL) return;
    heap = kiln_c99_gc_heap();
    if (heap == NULL) return;
    block = kiln_c99_gc_lookup_slab(heap, payload, &slab);
    if (block == NULL) return;
    if ((block->kind & KILN_GC_KIND_MASK) == KILN_GC_STRING)
        kiln_c99_key_forget(heap, (const char *)payload, block->size);
    kiln_c99_gc_finalize(block);
    heap->freed_blocks += 1;
    heap->freed_bytes += block->size;
    if (slab != NULL) {
        heap->count -= 1;
        heap->bytes -= block->size;
        block->size = KILN_GC_SLOT_FREE;
        block->kind = KILN_GC_NONE;
        block->next = slab->free_head;
        slab->free_head = block;
        if (++slab->free_count == 1) kiln_c99_gc_slab_link_free(heap, slab);
        if (slab->free_count == slab->slot_count) kiln_c99_gc_slab_maybe_destroy(heap, slab);
    } else {
        kiln_c99_gc_unlink(heap, block);
        kiln_c99_gc_ptrset_remove(&heap->large_table, block);
        free(block);
    }
}

static void kiln_c99_gc_mark_block(kiln_gc_heap *heap, const void *payload) {
    kiln_gc_block *block = kiln_c99_gc_lookup(heap, payload);
    if (block == NULL || block->mark == heap->epoch) return;
    block->mark = heap->epoch;
    if (heap->stack_len == heap->stack_capacity) {
        size_t wanted = heap->stack_capacity == 0 ? 256 : heap->stack_capacity * 2;
        kiln_gc_block **grown =
            (kiln_gc_block **)realloc(heap->stack, wanted * sizeof *heap->stack);
        if (grown == NULL) {
            /* A collection that cannot record what it saw must not free anything. */
            heap->mark_failed = 1;
            return;
        }
        heap->stack = grown;
        heap->stack_capacity = wanted;
    }
    heap->stack[heap->stack_len++] = block;
}

static void kiln_c99_gc_mark_value(kiln_gc_heap *heap, const kiln_value *value) {
    /* Every payload-carrying tag keeps its pointer in the shared word, and a pointer the heap
     * does not know (a constant, a well-known symbol, a provider handle) is simply not marked. */
    if (value == NULL || value->tag < KILN_VALUE_STRING) return;
    kiln_c99_gc_mark_block(heap, (const void *)(uintptr_t)value->bits);
}

static void kiln_c99_gc_mark_values(kiln_gc_heap *heap, const kiln_value *values,
                                    size_t count) {
    size_t index;
    if (values == NULL) return;
    for (index = 0; index < count; ++index) kiln_c99_gc_mark_value(heap, &values[index]);
}

static void kiln_c99_gc_trace(kiln_gc_heap *heap, kiln_gc_block *block) {
    void *payload = block + 1;
    switch (block->kind & KILN_GC_KIND_MASK) {
    case KILN_GC_OBJECT: {
        kiln_object *object = (kiln_object *)payload;
        size_t index;
        for (index = 0; index < object->count; ++index) {
            const kiln_property *property = &object->properties[index];
            kiln_c99_gc_mark_block(heap, property->key);
            kiln_c99_gc_mark_value(heap, &property->value);
            kiln_c99_gc_mark_value(heap, &property->getter);
            kiln_c99_gc_mark_value(heap, &property->setter);
        }
        kiln_c99_gc_mark_value(heap, &object->prototype);
        break;
    }
    case KILN_GC_CLOSURE: {
        kiln_closure *closure = (kiln_closure *)payload;
        kiln_c99_gc_mark_values(heap, closure->upvalues, closure->upvalue_count);
        kiln_c99_gc_mark_value(heap, &closure->captured_this);
        kiln_c99_gc_mark_value(heap, &closure->captured_new_target);
        kiln_c99_gc_mark_value(heap, &closure->captured_arguments);
        kiln_c99_gc_mark_value(heap, &closure->prototype);
        kiln_c99_gc_mark_value(heap, &closure->properties);
        break;
    }
    case KILN_GC_CELL:
        kiln_c99_gc_mark_value(heap, (const kiln_value *)payload);
        break;
    case KILN_GC_ARRAY: {
        kiln_array *array = (kiln_array *)payload;
        kiln_c99_gc_mark_values(heap, array->elements, array->count);
        kiln_c99_gc_mark_value(heap, &array->properties);
        break;
    }
    case KILN_GC_GENERATOR: {
        kiln_generator *frame = (kiln_generator *)payload;
        kiln_c99_gc_mark_value(heap, &frame->function);
        kiln_c99_gc_mark_value(heap, &frame->receiver);
        kiln_c99_gc_mark_value(heap, &frame->promise);
        kiln_c99_gc_mark_value(heap, &frame->sent);
        if (frame->function.tag == KILN_VALUE_FUNCTION) {
            const kiln_closure *closure = (const kiln_closure *)(uintptr_t)frame->function.bits;
            kiln_c99_gc_mark_values(heap, frame->locals, closure->local_count);
            kiln_c99_gc_mark_values(heap, frame->stack, closure->stack_size);
        }
        break;
    }
    case KILN_GC_PROMISE: {
        kiln_promise *promise = (kiln_promise *)payload;
        size_t index;
        kiln_c99_gc_mark_value(heap, &promise->value);
        for (index = 0; index < promise->count; ++index) {
            kiln_c99_gc_mark_value(heap, &promise->reactions[index].on_fulfilled);
            kiln_c99_gc_mark_value(heap, &promise->reactions[index].on_rejected);
            kiln_c99_gc_mark_value(heap, &promise->reactions[index].derived);
        }
        break;
    }
    case KILN_GC_SETTLER:
        kiln_c99_gc_mark_value(heap, &((kiln_settler *)payload)->promise);
        break;
    case KILN_GC_BOUND: {
        kiln_bound *bound = (kiln_bound *)payload;
        kiln_c99_gc_mark_value(heap, &bound->target);
        kiln_c99_gc_mark_value(heap, &bound->receiver);
        kiln_c99_gc_mark_values(heap, bound->arguments, bound->count);
        kiln_c99_gc_mark_value(heap, &bound->properties);
        break;
    }
    case KILN_GC_COLLECTION: {
        kiln_collection *collection = (kiln_collection *)payload;
        size_t index;
        for (index = 0; index < collection->count; ++index) {
            if (collection->entries[index].dead) continue;
            kiln_c99_gc_mark_value(heap, &collection->entries[index].key);
            kiln_c99_gc_mark_value(heap, &collection->entries[index].value);
        }
        kiln_c99_gc_mark_value(heap, &collection->properties);
        break;
    }
    case KILN_GC_PROXY:
        kiln_c99_gc_mark_value(heap, &((kiln_proxy *)payload)->target);
        kiln_c99_gc_mark_value(heap, &((kiln_proxy *)payload)->handler);
        break;
    case KILN_GC_TYPED_ARRAY:
        kiln_c99_gc_mark_block(heap, ((kiln_typed_array *)payload)->buffer);
        break;
    case KILN_GC_DATA_VIEW:
        kiln_c99_gc_mark_block(heap, ((kiln_data_view *)payload)->buffer);
        break;
    case KILN_GC_METHOD_STORE:
        kiln_c99_gc_mark_value(heap, &((kiln_method_store *)payload)->properties);
        kiln_c99_gc_mark_block(heap, ((kiln_method_store *)payload)->next);
        break;
    case KILN_GC_NODE_SERVER: {
        kiln_node_server *server = (kiln_node_server *)payload;
        kiln_c99_gc_mark_value(heap, &server->object);
        kiln_c99_gc_mark_value(heap, &server->connection_listener);
        kiln_c99_gc_mark_block(heap, server->sockets);
        break;
    }
    case KILN_GC_NODE_SOCKET: {
        kiln_node_socket *socket = (kiln_node_socket *)payload;
        kiln_c99_gc_mark_value(heap, &socket->object);
        kiln_c99_gc_mark_block(heap, socket->next);
        kiln_c99_gc_mark_block(heap, socket->server);
        kiln_c99_gc_mark_block(heap, socket->client_requests);
        kiln_c99_gc_mark_block(heap, socket->responses);
        break;
    }
    case KILN_GC_NODE_CLIENT_REQUEST: {
        kiln_node_client_request *request = (kiln_node_client_request *)payload;
        kiln_c99_gc_mark_value(heap, &request->object);
        kiln_c99_gc_mark_value(heap, &request->callback);
        kiln_c99_gc_mark_value(heap, &request->response);
        kiln_c99_gc_mark_block(heap, request->socket);
        kiln_c99_gc_mark_block(heap, request->next);
        break;
    }
    case KILN_GC_NODE_RESPONSE: {
        kiln_node_response *response = (kiln_node_response *)payload;
        kiln_c99_gc_mark_value(heap, &response->request);
        kiln_c99_gc_mark_value(heap, &response->object);
        kiln_c99_gc_mark_value(heap, &response->headers);
        kiln_c99_gc_mark_block(heap, response->socket);
        kiln_c99_gc_mark_block(heap, response->next);
        break;
    }
    case KILN_GC_NODE_COMPLETION: {
        kiln_node_completion *completion = (kiln_node_completion *)payload;
        kiln_c99_gc_mark_block(heap, completion->server);
        kiln_c99_gc_mark_block(heap, completion->socket);
        kiln_c99_gc_mark_block(heap, completion->client_request);
        break;
    }
    case KILN_GC_NODE_FS_COMPLETION:
        kiln_c99_gc_mark_value(heap, &((kiln_node_fs_completion *)payload)->promise);
        break;
    case KILN_GC_MICROTASK: {
        kiln_microtask *task = (kiln_microtask *)payload;
        kiln_c99_gc_mark_value(heap, &task->handler);
        kiln_c99_gc_mark_value(heap, &task->argument);
        kiln_c99_gc_mark_value(heap, &task->derived);
        break;
    }
    case KILN_GC_NEXT_TICK: {
        kiln_next_tick *tick = (kiln_next_tick *)payload;
        kiln_c99_gc_mark_value(heap, &tick->callback);
        kiln_c99_gc_mark_values(heap, tick->args, tick->argc);
        break;
    }
    case KILN_GC_TIMER: {
        kiln_timer *timer = (kiln_timer *)payload;
        kiln_c99_gc_mark_value(heap, &timer->callback);
        kiln_c99_gc_mark_values(heap, timer->args, timer->argc);
        break;
    }
    case KILN_GC_REGEXP: {
        /* A pattern borrows the bytes it was compiled from, which may be a unit constant or a
         * string the program built; the block table decides which. The provider handle is not a
         * block and is released by the finaliser instead. */
        kiln_regexp *regexp = (kiln_regexp *)payload;
        kiln_c99_gc_mark_block(heap, regexp->source);
        kiln_c99_gc_mark_block(heap, regexp->flags);
        break;
    }
    case KILN_GC_SYMBOL: {
        /* The reserved key is malloc-backed and outlives the heap, but a description can be a
         * collected string (`Symbol(String(x))`), so both are offered to the table. */
        kiln_symbol *symbol = (kiln_symbol *)payload;
        kiln_c99_gc_mark_block(heap, symbol->key);
        kiln_c99_gc_mark_block(heap, symbol->description);
        break;
    }
    case KILN_GC_STRING:
    case KILN_GC_BYTES:
    case KILN_GC_DATE:
    case KILN_GC_TYPED_BUFFER:
    case KILN_GC_COMMONJS_REQUIRE:
    default:
        break;
    }
}

/* Inner growth buffers are the owner's, never a value's, so they are released here and only
 * here. A kind whose payload owns a provider handle releases it here too. */
static void kiln_c99_gc_finalize(kiln_gc_block *block) {
    void *payload = block + 1;
    switch (block->kind & KILN_GC_KIND_MASK) {
    case KILN_GC_OBJECT:
        free(((kiln_object *)payload)->properties);
        break;
    case KILN_GC_CLOSURE:
        free(((kiln_closure *)payload)->upvalues);
        break;
    case KILN_GC_ARRAY:
        free(((kiln_array *)payload)->elements);
        break;
    case KILN_GC_GENERATOR: {
        kiln_generator *frame = (kiln_generator *)payload;
        free(frame->locals);
        free(frame->stack);
        free(frame->handlers);
        free(frame->handler_stack);
        break;
    }
    case KILN_GC_PROMISE:
        free(((kiln_promise *)payload)->reactions);
        break;
    case KILN_GC_BOUND:
        free(((kiln_bound *)payload)->arguments);
        break;
    case KILN_GC_COLLECTION:
        free(((kiln_collection *)payload)->entries);
        break;
    case KILN_GC_TYPED_BUFFER:
        free(((kiln_typed_buffer *)payload)->bytes);
        break;
    case KILN_GC_REGEXP:
        /* The compiled pattern is the provider's, and this is the only place it is handed back. */
        kiln_regexp_free(((kiln_regexp *)payload)->handle);
        break;
    case KILN_GC_NODE_SERVER:
        free(((kiln_node_server *)payload)->address);
        break;
    case KILN_GC_NODE_RESPONSE:
        free(((kiln_node_response *)payload)->request_headers);
        break;
    case KILN_GC_NODE_CLIENT_REQUEST: {
        kiln_node_client_request *request = (kiln_node_client_request *)payload;
        free(request->host_name);
        free(request->path);
        free(request->method);
        free(request->headers);
        free(request->pending_body);
        break;
    }
    case KILN_GC_NEXT_TICK:
        free(((kiln_next_tick *)payload)->args);
        break;
    case KILN_GC_TIMER:
        free(((kiln_timer *)payload)->args);
        break;
    default:
        break;
    }
}

static void kiln_c99_gc_drain(kiln_gc_heap *heap) {
    while (heap->stack_len != 0 && !heap->mark_failed) {
        kiln_gc_block *block = heap->stack[--heap->stack_len];
        kiln_c99_gc_trace(heap, block);
    }
}

static void kiln_c99_gc_begin(kiln_gc_heap *heap) {
    ++heap->epoch;
    if (heap->epoch == 0) ++heap->epoch;
    heap->stack_len = 0;
    heap->mark_failed = 0;
}

/* Free every unmarked block. Marked blocks either stay (an ordinary collection) or are orphaned
 * (teardown, where a result a caller still borrows must outlive the heap).
 *
 * Large blocks are unchanged from the pre-slab collector: each is its own allocation, so an
 * orphaned survivor is simply left off every list and table, still valid memory the caller
 * borrows. A slab-resident survivor cannot be orphaned the same way -- its memory is shared with
 * sibling slots in the same slab -- so at teardown a slab that still has a live slot in it is
 * leaked whole instead (a bounded, one-time cost paid only for a borrowed result, ADR 0034); a
 * slab with no survivors is freed like any other. An ordinary collection instead returns each
 * dead slot to its slab's free list and prunes a slab that lands completely empty. */
static void kiln_c99_gc_sweep(kiln_gc_heap *heap, int orphan_marked) {
    kiln_gc_block *block = heap->head;
    kiln_gc_slab *slab = heap->all_slabs;
    size_t survived = 0;
    while (block != NULL) {
        kiln_gc_block *next = block->next;
        if (block->mark != heap->epoch) {
            kiln_c99_gc_unlink(heap, block);
            kiln_c99_gc_ptrset_remove(&heap->large_table, block);
            kiln_c99_gc_finalize(block);
            heap->freed_blocks += 1;
            heap->freed_bytes += block->size;
            free(block);
        } else if (orphan_marked) {
            kiln_c99_gc_unlink(heap, block);
            kiln_c99_gc_ptrset_remove(&heap->large_table, block);
        } else {
            survived += block->size;
        }
        block = next;
    }
    while (slab != NULL) {
        kiln_gc_slab *next_slab = slab->all_next;
        uint32_t index;
        int had_survivor = 0;
        for (index = 0; index < slab->slot_count; ++index) {
            kiln_gc_block *slot = kiln_c99_gc_slab_slot(slab, index);
            if (slot->size == KILN_GC_SLOT_FREE) continue;
            if (slot->mark != heap->epoch) {
                kiln_c99_gc_finalize(slot);
                heap->freed_blocks += 1;
                heap->freed_bytes += slot->size;
                heap->count -= 1;
                heap->bytes -= slot->size;
                slot->size = KILN_GC_SLOT_FREE;
                slot->kind = KILN_GC_NONE;
                slot->next = slab->free_head;
                slab->free_head = slot;
                if (++slab->free_count == 1) kiln_c99_gc_slab_link_free(heap, slab);
            } else if (orphan_marked) {
                had_survivor = 1;
            } else {
                survived += slot->size;
            }
        }
        if (orphan_marked) {
            if (!had_survivor) {
                if (slab->free_count != 0) kiln_c99_gc_slab_unlink_free(heap, slab);
                kiln_c99_gc_slab_unlink_all(heap, slab);
                kiln_c99_gc_ptrset_remove(&heap->slab_table, slab);
                kiln_c99_gc_aligned_free(slab);
            }
            /* else: leaked with a live survivor inside; the heap is being torn down anyway. */
        } else if (slab->free_count == slab->slot_count) {
            kiln_c99_gc_slab_maybe_destroy(heap, slab);
        }
        slab = next_slab;
    }
    heap->survived = survived;
    heap->since_collection = 0;
}

static void kiln_c99_gc_collect(void *opaque, int force) {
    kiln_gc_heap *heap = (kiln_gc_heap *)opaque;
    size_t threshold;
    uint64_t before_blocks;
    uint64_t before_bytes;
    if (heap == NULL || heap->collecting || heap->fallback) return;
    if (!heap->enabled && !heap->stress) return;
    threshold = heap->floor > heap->survived ? heap->floor : heap->survived;
    if (!force && !heap->stress && heap->since_collection < threshold) return;
    heap->collecting = 1;
    before_blocks = heap->freed_blocks;
    before_bytes = heap->freed_bytes;
    kiln_c99_gc_begin(heap);
    (void)kiln_realm_gc_mark_roots(heap);
    kiln_c99_key_mark_pinned(heap);
    kiln_c99_gc_drain(heap);
    if (!heap->mark_failed) {
        kiln_c99_key_purge(heap);
        kiln_c99_gc_sweep(heap, 0);
        heap->collections += 1;
    }
    if (heap->trace) {
        kiln_c99_gc_trace_text("kiln gc: collection ");
        kiln_c99_gc_trace_count(heap->collections);
        kiln_c99_gc_trace_text(" freed ");
        kiln_c99_gc_trace_count(heap->freed_blocks - before_blocks);
        kiln_c99_gc_trace_text(" blocks (");
        kiln_c99_gc_trace_count(heap->freed_bytes - before_bytes);
        kiln_c99_gc_trace_text(" bytes), ");
        kiln_c99_gc_trace_count((uint64_t)heap->count);
        kiln_c99_gc_trace_text(" blocks (");
        kiln_c99_gc_trace_count((uint64_t)heap->bytes);
        kiln_c99_gc_trace_text(heap->mark_failed
                                   ? " bytes) live (marking ran out of memory; nothing freed)\n"
                                   : " bytes) live\n");
    }
    heap->collecting = 0;
}

static void kiln_c99_gc_teardown_begin(void *opaque) {
    kiln_gc_heap *heap = (kiln_gc_heap *)opaque;
    if (heap == NULL || heap->fallback) return;
    heap->collecting = 1;
    kiln_c99_gc_begin(heap);
}

static void kiln_c99_gc_teardown_end(void *opaque) {
    kiln_gc_heap *heap = (kiln_gc_heap *)opaque;
    if (heap == NULL || heap->fallback) return;
    kiln_c99_gc_drain(heap);
    if (heap->mark_failed) {
        /* Orphan everything rather than free a block a survivor root still names. */
        ++heap->epoch;
        if (heap->epoch == 0) ++heap->epoch;
        for (kiln_gc_block *block = heap->head; block != NULL; block = block->next)
            block->mark = heap->epoch;
        for (kiln_gc_slab *slab = heap->all_slabs; slab != NULL; slab = slab->all_next) {
            uint32_t index;
            for (index = 0; index < slab->slot_count; ++index) {
                kiln_gc_block *slot = kiln_c99_gc_slab_slot(slab, index);
                if (slot->size != KILN_GC_SLOT_FREE) slot->mark = heap->epoch;
            }
        }
    }
    kiln_c99_gc_sweep(heap, 1);
    if (heap->trace) {
        kiln_c99_gc_trace_text("kiln gc: teardown after ");
        kiln_c99_gc_trace_count(heap->collections);
        kiln_c99_gc_trace_text(" collections, ");
        kiln_c99_gc_trace_count(heap->freed_blocks);
        kiln_c99_gc_trace_text(" blocks (");
        kiln_c99_gc_trace_count(heap->freed_bytes);
        kiln_c99_gc_trace_text(" bytes) freed in total\n");
    }
    free(heap->large_table.table);
    free(heap->slab_table.table);
    free(heap->stack);
    free(heap->keys.entries);
    free(heap);
}

static void kiln_c99_gc_hook_mark_value(void *opaque, const void *value) {
    kiln_c99_gc_mark_value((kiln_gc_heap *)opaque, (const kiln_value *)value);
}

static void kiln_c99_gc_hook_mark_block(void *opaque, void *payload) {
    kiln_c99_gc_mark_block((kiln_gc_heap *)opaque, payload);
}

static const kiln_gc_hooks kiln_c99_gc_hooks = {
    KILN_GC_HOOKS_VERSION,
    (uint32_t)sizeof(kiln_gc_hooks),
    kiln_c99_gc_collect,
    kiln_c99_gc_teardown_begin,
    kiln_c99_gc_teardown_end,
    kiln_c99_gc_hook_mark_value,
    kiln_c99_gc_hook_mark_block,
    kiln_c99_gc_hook_introspect,
};

/* A borrowed result slot: marked during collection and again at teardown, so the value a caller
 * reads after `kiln_realm_destroy` is never freed by it (ADR 0034's borrowed-result contract). */
static void kiln_c99_gc_result_roots(void *opaque, void *context, int phase) {
    if (phase == KILN_GC_PHASE_RESET || context == NULL) return;
    kiln_c99_gc_mark_value((kiln_gc_heap *)opaque, (const kiln_value *)context);
}

/* Allocation helpers for the kinds whose bytes are the value: strings and raw byte payloads. */
static char *kiln_c99_gc_alloc_text(size_t len) {
    return (char *)kiln_c99_gc_alloc(KILN_GC_STRING, len == 0 ? 1 : len);
}

static int kiln_c99_new_object(kiln_value *out) {
    kiln_object *object = (kiln_object *)kiln_c99_gc_alloc(KILN_GC_OBJECT, sizeof *object);
    if (object == NULL) {
        return KILN_C99_OUT_OF_MEMORY;
    }
    object->properties = NULL;
    object->count = 0;
    object->capacity = 0;
    object->prototype = kiln_c99_undefined();
    object->frozen = 0;
    object->not_extensible = 0;
    object->module_namespace = 0;
    object->arguments_brand = 0;
    object->accessor_count = 0;
    object->read_only_seen = 0;
    *out = kiln_c99_value(KILN_VALUE_OBJECT, (uint64_t)(uintptr_t)object, NULL, 0);
    return KILN_C99_OK;
}

/* An object whose property array is sized once, for a caller that already knows how many members
 * it is about to add.
 *
 * `kiln_c99_reserve` doubles from four, which is the right policy for a program building an
 * object a member at a time and the wrong one for the runtime filling a fixed template: a
 * `node:http` request with twelve own properties walked 4 -> 8 -> 16, paying a `realloc` and a
 * copy at each step to arrive at the capacity it could have asked for. The capacity is a hint,
 * not a limit -- passing a number too small only restores the doubling. */
static int kiln_c99_new_object_with_capacity(kiln_value *out, size_t capacity) {
    kiln_object *object;
    int status = kiln_c99_new_object(out);
    if (status != KILN_C99_OK) {
        return status;
    }
    object = kiln_c99_object_of(*out);
    /* The object is already on the heap, so a failure here leaves it to the collector rather
     * than to a partially built caller. */
    return kiln_c99_reserve((void **)&object->properties, &object->capacity, capacity,
                            sizeof *object->properties);
}

/* Cancellation is ordinary object state, not a new value kind. That keeps the hot value layout
 * and every host boundary unchanged: only the controller owns the method that can transition its
 * signal, while consumers receive the signal object and its read/listen surface. */
static int kiln_c99_new_abort_signal(kiln_value *out) {
    kiln_value listeners;
    int status = kiln_c99_new_object(out);
    if (status == KILN_C99_OK) status = kiln_c99_new_array(&listeners);
    if (status == KILN_C99_OK)
        status = kiln_c99_put(NULL, *out, "aborted", 7, kiln_c99_boolean(0), 0, NULL);
    if (status == KILN_C99_OK)
        status = kiln_c99_put(NULL, *out, "reason", 6, kiln_c99_undefined(), 0, NULL);
    if (status == KILN_C99_OK)
        status = kiln_c99_put(NULL, *out, "onabort", 7, kiln_c99_null(), 0, NULL);
    if (status == KILN_C99_OK)
        status = kiln_c99_define_method(*out, "addEventListener", 16,
                                        kiln_c99_method(KILN_C99_METHOD_ABORT_SIGNAL_LISTEN));
    if (status == KILN_C99_OK)
        status = kiln_c99_define_method(*out, "removeEventListener", 19,
                                        kiln_c99_method(KILN_C99_METHOD_ABORT_SIGNAL_UNLISTEN));
    if (status == KILN_C99_OK)
        status = kiln_c99_define_method(*out, "throwIfAborted", 14,
                                        kiln_c99_method(KILN_C99_METHOD_ABORT_SIGNAL_THROW));
    if (status == KILN_C99_OK)
        status = kiln_c99_put(NULL, *out, "@@abort-listeners", 17, listeners, 0, NULL);
    if (status == KILN_C99_OK) {
        kiln_object *object = kiln_c99_object_of(*out);
        size_t index;
        for (index = 0; index < object->count; ++index) object->properties[index].enumerable = 0;
    }
    return status;
}

static int kiln_c99_new_abort_controller(kiln_value *out) {
    kiln_value signal;
    int status = kiln_c99_new_abort_signal(&signal);
    if (status == KILN_C99_OK) status = kiln_c99_new_object(out);
    if (status == KILN_C99_OK)
        status = kiln_c99_put(NULL, *out, "signal", 6, signal, 0, NULL);
    if (status == KILN_C99_OK)
        status = kiln_c99_define_method(*out, "abort", 5,
                                        kiln_c99_method(KILN_C99_METHOD_ABORT_CONTROLLER_ABORT));
    if (status == KILN_C99_OK) {
        kiln_object *object = kiln_c99_object_of(*out);
        size_t index;
        for (index = 0; index < object->count; ++index) object->properties[index].enumerable = 0;
    }
    return status;
}

static int kiln_c99_new_text_encoder(kiln_value *out) {
    int status = kiln_c99_new_object(out);
    if (status == KILN_C99_OK)
        status = kiln_c99_put(NULL, *out, "encoding", 8, kiln_c99_copy_text("utf-8", 5), 0,
                              NULL);
    if (status == KILN_C99_OK)
        status = kiln_c99_define_method(*out, "encode", 6,
                                        kiln_c99_method(KILN_C99_METHOD_TEXT_ENCODER_ENCODE));
    if (status == KILN_C99_OK) {
        kiln_object *object = kiln_c99_object_of(*out);
        size_t index;
        for (index = 0; index < object->count; ++index) object->properties[index].enumerable = 0;
    }
    return status;
}

static int kiln_c99_abort(kiln_host *host, kiln_value signal, kiln_value reason, int depth,
                          kiln_value *thrown) {
    kiln_value aborted;
    kiln_value listeners;
    kiln_value handler;
    kiln_value event;
    size_t index;
    int status;
    if (signal.tag != KILN_VALUE_OBJECT) return KILN_C99_OK;
    status = kiln_c99_get(host, signal, "aborted", 7, &aborted, depth, thrown);
    if (status != KILN_C99_OK || kiln_c99_truthy(aborted)) return status;
    if (reason.tag == KILN_VALUE_UNDEFINED) {
        status = kiln_c99_new_error(&reason, KILN_C99_BUILTIN_ERROR,
                                    kiln_c99_copy_text("This operation was aborted", 26));
        if (status == KILN_C99_OK)
            status = kiln_c99_put(NULL, reason, "name", 4,
                                  kiln_c99_copy_text("AbortError", 10), 0, NULL);
        if (status != KILN_C99_OK) return status;
    }
    status = kiln_c99_put(NULL, signal, "aborted", 7, kiln_c99_boolean(1), 0, NULL);
    if (status == KILN_C99_OK)
        status = kiln_c99_put(NULL, signal, "reason", 6, reason, 0, NULL);
    if (status == KILN_C99_OK)
        status = kiln_c99_get(host, signal, "@@abort-listeners", 17, &listeners, depth, thrown);
    if (status == KILN_C99_OK)
        status = kiln_c99_get(host, signal, "onabort", 7, &handler, depth, thrown);
    if (status == KILN_C99_OK) status = kiln_c99_new_object(&event);
    if (status == KILN_C99_OK)
        status = kiln_c99_put(NULL, event, "type", 4, kiln_c99_copy_text("abort", 5), 0, NULL);
    if (status == KILN_C99_OK)
        status = kiln_c99_put(NULL, event, "target", 6, signal, 0, NULL);
    if (status != KILN_C99_OK) return status;
    if (kiln_c99_is_callable(handler)) {
        kiln_value ignored;
        status = kiln_c99_call(host, &ignored, handler, signal, &event, 1, depth, thrown);
        if (status != KILN_C99_OK) return status;
    }
    if (listeners.tag != KILN_VALUE_ARRAY) return KILN_C99_OK;
    for (index = 0; index < kiln_c99_array_of(listeners)->count; ++index) {
        kiln_value ignored;
        kiln_value listener = kiln_c99_array_of(listeners)->elements[index];
        status = kiln_c99_call(host, &ignored, listener, signal, &event, 1, depth, thrown);
        if (status != KILN_C99_OK) return status;
    }
    return KILN_C99_OK;
}

static int kiln_c99_buffer_from(kiln_value *out, kiln_value source, kiln_value *thrown) {
    kiln_value built;
    size_t index;
    int status;
    if (source.tag == KILN_VALUE_STRING) {
        status = kiln_c99_typed_alloc(&built, KILN_TYPED_UINT8, source.len);
        if (status != KILN_C99_OK) return status;
        for (index = 0; index < source.len; ++index) {
            kiln_c99_typed_set(kiln_c99_typed_array_of(built), index,
                               (double)(unsigned char)source.ptr[index]);
        }
    } else if (source.tag == KILN_VALUE_TYPED_ARRAY || source.tag == KILN_VALUE_ARRAY) {
        status = kiln_c99_new_typed_array(&built, KILN_TYPED_UINT8, source, 0, 0, 0, thrown);
        if (status != KILN_C99_OK) return status;
    } else {
        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                    "Buffer.from needs a string, array, or typed array");
    }
    kiln_c99_typed_array_of(built)->is_buffer = 1;
    *out = built;
    return KILN_C99_OK;
}

static int kiln_c99_buffer_method(kiln_value *out, uint64_t code, kiln_value receiver,
                                  const kiln_value *args, size_t argc, kiln_value *thrown) {
    kiln_value first = argc > 0 ? args[0] : kiln_c99_undefined();
    if (code == KILN_C99_METHOD_BUFFER_FROM) return kiln_c99_buffer_from(out, first, thrown);
    if (code == KILN_C99_METHOD_BUFFER_ALLOC) {
        size_t length;
        int status = kiln_c99_to_index(first, &length, thrown);
        if (status != KILN_C99_OK) return status;
        status = kiln_c99_typed_alloc(out, KILN_TYPED_UINT8, length);
        if (status == KILN_C99_OK) kiln_c99_typed_array_of(*out)->is_buffer = 1;
        return status;
    }
    if (code == KILN_C99_METHOD_BUFFER_IS_BUFFER) {
        *out = kiln_c99_boolean(first.tag == KILN_VALUE_TYPED_ARRAY &&
                                kiln_c99_typed_array_of(first)->is_buffer);
        return KILN_C99_OK;
    }
    if (code == KILN_C99_METHOD_BUFFER_BYTE_LENGTH) {
        *out = kiln_c99_number(first.tag == KILN_VALUE_STRING ? (double)first.len :
            first.tag == KILN_VALUE_TYPED_ARRAY ?
                (double)(kiln_c99_typed_array_of(first)->length *
                         kiln_c99_typed_width(kiln_c99_typed_array_of(first)->kind)) :
                (double)kiln_c99_text(first).len);
        return KILN_C99_OK;
    }
    if (code == KILN_C99_METHOD_BUFFER_CONCAT) {
        kiln_array *items;
        size_t total = 0;
        size_t at;
        size_t written = 0;
        int status;
        if (first.tag != KILN_VALUE_ARRAY)
            return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                        "Buffer.concat needs an array");
        items = kiln_c99_array_of(first);
        for (at = 0; at < items->count; ++at) {
            if (items->elements[at].tag != KILN_VALUE_TYPED_ARRAY)
                return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                            "Buffer.concat entries must be typed arrays");
            total += kiln_c99_typed_array_of(items->elements[at])->length;
        }
        status = kiln_c99_typed_alloc(out, KILN_TYPED_UINT8, total);
        if (status != KILN_C99_OK) return status;
        for (at = 0; at < items->count; ++at) {
            kiln_typed_array *view = kiln_c99_typed_array_of(items->elements[at]);
            size_t index;
            for (index = 0; index < view->length; ++index)
                kiln_c99_typed_set(kiln_c99_typed_array_of(*out), written++,
                                   kiln_c99_typed_get(view, index));
        }
        kiln_c99_typed_array_of(*out)->is_buffer = 1;
        return KILN_C99_OK;
    }
    if (code == KILN_C99_METHOD_BUFFER_TO_STRING) {
        kiln_typed_array *view;
        kiln_value encoding = argc > 0 ? kiln_c99_text(args[0]) : kiln_c99_copy_text("utf8", 4);
        if (receiver.tag != KILN_VALUE_TYPED_ARRAY ||
            !(encoding.len == 4 && memcmp(encoding.ptr, "utf8", 4) == 0) )
            return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                        "unsupported Buffer encoding");
        view = kiln_c99_typed_array_of(receiver);
        *out = kiln_c99_copy_text((const char *)view->buffer->bytes + view->byte_offset,
                                  view->length);
        return out->tag == KILN_VALUE_STRING ? KILN_C99_OK : KILN_C99_OUT_OF_MEMORY;
    }
    KILN_C99_REFUSE("a Buffer method this backend has not implemented");
}

static int kiln_c99_node_dns_lookup(kiln_host *host, kiln_value *out,
                                    const kiln_value *args, size_t argc, int depth,
                                    kiln_value *thrown) {
    kiln_value name = argc > 0 ? kiln_c99_text(args[0]) : kiln_c99_copy_text("localhost", 9);
    kiln_value callback = kiln_c99_undefined();
    kiln_value options = argc > 1 ? args[1] : kiln_c99_undefined();
    kiln_value addresses;
    kiln_value callback_args[3];
    size_t at;
    int all = 0;
    int status;
    for (at = argc; at > 0; --at) {
        if (kiln_c99_is_callable(args[at - 1])) {
            callback = args[at - 1];
            break;
        }
    }
    if (!kiln_c99_is_callable(callback))
        return kiln_c99_throw_not_a(thrown, callback, "function");
    if (name.tag != KILN_VALUE_STRING) return KILN_C99_OUT_OF_MEMORY;
    if (options.tag == KILN_VALUE_OBJECT) {
        kiln_value read;
        status = kiln_c99_get(host, options, "all", 3, &read, depth, thrown);
        if (status != KILN_C99_OK) return status;
        all = kiln_c99_truthy(read);
    }
    status = kiln_c99_new_array(&addresses);
    if (status != KILN_C99_OK) return status;
    if ((name.len == 9 && memcmp(name.ptr, "localhost", 9) == 0) ||
        (name.len == 9 && memcmp(name.ptr, "127.0.0.1", 9) == 0)) {
        kiln_value address;
        status = kiln_c99_new_object(&address);
        if (status == KILN_C99_OK)
            status = kiln_c99_put(NULL, address, "address", 7,
                                  kiln_c99_copy_text("127.0.0.1", 9), 0, NULL);
        if (status == KILN_C99_OK)
            status = kiln_c99_put(NULL, address, "family", 6, kiln_c99_number(4.0), 0, NULL);
        if (status == KILN_C99_OK) status = kiln_c99_array_push(addresses, address);
    }
    if ((name.len == 9 && memcmp(name.ptr, "localhost", 9) == 0) ||
        (name.len == 3 && memcmp(name.ptr, "::1", 3) == 0)) {
        kiln_value address;
        status = kiln_c99_new_object(&address);
        if (status == KILN_C99_OK)
            status = kiln_c99_put(NULL, address, "address", 7,
                                  kiln_c99_copy_text("::1", 3), 0, NULL);
        if (status == KILN_C99_OK)
            status = kiln_c99_put(NULL, address, "family", 6, kiln_c99_number(6.0), 0, NULL);
        if (status == KILN_C99_OK) status = kiln_c99_array_push(addresses, address);
    }
    if (status != KILN_C99_OK) return status;
    if (kiln_c99_array_of(addresses)->count == 0) {
        callback_args[0] = kiln_c99_copy_text("ENOTFOUND", 9);
        callback_args[1] = addresses;
        status = kiln_c99_call(host, out, callback, kiln_c99_undefined(), callback_args, 2,
                               depth, thrown);
    } else if (all) {
        callback_args[0] = kiln_c99_null();
        callback_args[1] = addresses;
        status = kiln_c99_call(host, out, callback, kiln_c99_undefined(), callback_args, 2,
                               depth, thrown);
    } else {
        kiln_value first = kiln_c99_array_of(addresses)->elements[0];
        callback_args[0] = kiln_c99_null();
        status = kiln_c99_get(host, first, "address", 7, &callback_args[1], depth, thrown);
        if (status == KILN_C99_OK)
            status = kiln_c99_get(host, first, "family", 6, &callback_args[2], depth, thrown);
        if (status == KILN_C99_OK)
            status = kiln_c99_call(host, out, callback, kiln_c99_undefined(), callback_args, 3,
                                   depth, thrown);
    }
    if (status == KILN_C99_OK) *out = kiln_c99_undefined();
    return status;
}

static int kiln_c99_node_channel(kiln_value *out) {
    int status = kiln_c99_new_object(out);
    if (status == KILN_C99_OK)
        status = kiln_c99_put(NULL, *out, "hasSubscribers", 14, kiln_c99_boolean(0), 0, NULL);
    if (status == KILN_C99_OK)
        status = kiln_c99_define_method(
            *out, "publish", 7, kiln_c99_method(KILN_C99_METHOD_NODE_DIAGNOSTICS_PUBLISH));
    if (status == KILN_C99_OK)
        status = kiln_c99_define_method(
            *out, "runStores", 9, kiln_c99_method(KILN_C99_METHOD_NODE_DIAGNOSTICS_RUN_STORES));
    return status;
}

static int kiln_c99_node_diagnostics(kiln_host *host, kiln_value *out, uint64_t code,
                                     const kiln_value *args, size_t argc, int depth,
                                     kiln_value *thrown) {
    if (code == KILN_C99_METHOD_NODE_DIAGNOSTICS_CHANNEL)
        return kiln_c99_node_channel(out);
    if (code == KILN_C99_METHOD_NODE_DIAGNOSTICS_TRACING_CHANNEL) {
        static const char *const names[] = {"start", "end", "asyncStart", "asyncEnd", "error"};
        size_t index;
        int status = kiln_c99_new_object(out);
        if (status == KILN_C99_OK)
            status = kiln_c99_put(NULL, *out, "hasSubscribers", 14,
                                  kiln_c99_boolean(0), 0, NULL);
        for (index = 0; status == KILN_C99_OK && index < sizeof names / sizeof *names; ++index) {
            kiln_value channel;
            status = kiln_c99_node_channel(&channel);
            if (status == KILN_C99_OK)
                status = kiln_c99_put(NULL, *out, names[index], strlen(names[index]), channel,
                                      0, NULL);
        }
        return status;
    }
    if (code == KILN_C99_METHOD_NODE_DIAGNOSTICS_PUBLISH) {
        KILN_C99_ABSENT("not a member read: diagnostics_channel publish answers undefined", out);
    }
    if (code == KILN_C99_METHOD_NODE_DIAGNOSTICS_RUN_STORES) {
        kiln_value callback = argc > 1 ? args[1] : kiln_c99_undefined();
        kiln_value receiver = argc > 2 ? args[2] : kiln_c99_undefined();
        if (!kiln_c99_is_callable(callback))
            return kiln_c99_throw_not_a(thrown, callback, "function");
        return kiln_c99_call(host, out, callback, receiver, argc > 3 ? args + 3 : NULL,
                             argc > 3 ? argc - 3 : 0, depth, thrown);
    }
    KILN_C99_REFUSE("a diagnostics_channel member this backend has not implemented");
}

static int kiln_c99_node_util(kiln_host *host, kiln_value *out, uint64_t code,
                              const kiln_value *args, size_t argc, int depth,
                              kiln_value *thrown) {
    size_t index;
    if (code == KILN_C99_METHOD_NODE_UTIL_DEBUGLOG) {
        *out = kiln_c99_method(KILN_C99_METHOD_NODE_UTIL_NOOP);
        return KILN_C99_OK;
    }
    if (code == KILN_C99_METHOD_NODE_UTIL_NOOP) {
        KILN_C99_ABSENT("not a member read: the util no-op answers undefined", out);
    }
    if (code == KILN_C99_METHOD_NODE_UTIL_DEPRECATE) {
        *out = argc > 0 ? args[0] : kiln_c99_undefined();
        return KILN_C99_OK;
    }
    if (code == KILN_C99_METHOD_NODE_UTIL_INSPECT) {
        *out = kiln_c99_inspect(argc > 0 ? args[0] : kiln_c99_undefined());
        return out->tag == KILN_VALUE_STRING ? KILN_C99_OK : KILN_C99_OUT_OF_MEMORY;
    }
    if (code == KILN_C99_METHOD_NODE_UTIL_INHERITS) {
        kiln_value constructor = argc > 0 ? args[0] : kiln_c99_undefined();
        kiln_value parent = argc > 1 ? args[1] : kiln_c99_undefined();
        kiln_value constructor_prototype;
        kiln_value parent_prototype;
        int status;
        if (!kiln_c99_is_callable(constructor) || !kiln_c99_is_callable(parent))
            return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                        "The constructor and super constructor must be functions");
        status = kiln_c99_callable_prototype(constructor, &constructor_prototype);
        if (status == KILN_C99_OK)
            status = kiln_c99_callable_prototype(parent, &parent_prototype);
        if (status == KILN_C99_OK)
            status = kiln_c99_set_prototype(constructor_prototype, parent_prototype);
        *out = kiln_c99_undefined();
        return status;
    }
    if (code == KILN_C99_METHOD_NODE_UTIL_FORMAT) {
        kiln_text built = {NULL, 0, 0};
        size_t next = 1;
        if (argc == 0) {
            *out = kiln_c99_copy_text("", 0);
            return KILN_C99_OK;
        }
        if (args[0].tag != KILN_VALUE_STRING) {
            for (index = 0; index < argc; ++index) {
                kiln_value shown = kiln_c99_inspect(args[index]);
                if ((index != 0 && kiln_c99_text_append_c(&built, " ") != KILN_C99_OK) ||
                    shown.tag != KILN_VALUE_STRING ||
                    kiln_c99_text_append(&built, shown.ptr, shown.len) != KILN_C99_OK) {
                    free(built.bytes);
                    return KILN_C99_OUT_OF_MEMORY;
                }
            }
        } else {
            for (index = 0; index < args[0].len; ++index) {
                char character = args[0].ptr[index];
                if (character != '%' || index + 1 >= args[0].len) {
                    if (kiln_c99_text_append(&built, &character, 1) != KILN_C99_OK) goto format_oom;
                    continue;
                }
                character = args[0].ptr[++index];
                if (character == '%') {
                    if (kiln_c99_text_append_c(&built, "%") != KILN_C99_OK) goto format_oom;
                    continue;
                }
                if (next >= argc) {
                    if (kiln_c99_text_append_c(&built, "%") != KILN_C99_OK ||
                        kiln_c99_text_append(&built, &character, 1) != KILN_C99_OK) goto format_oom;
                    continue;
                }
                {
                    kiln_value value = args[next++];
                    kiln_value shown = character == 's' ? kiln_c99_text(value) :
                                       (character == 'd' || character == 'i' || character == 'f') ?
                                           kiln_c99_text(kiln_c99_number(kiln_c99_as_number(value))) :
                                       (character == 'j' || character == 'o' || character == 'O') ?
                                           kiln_c99_inspect(value) : kiln_c99_undefined();
                    if (shown.tag == KILN_VALUE_UNDEFINED) {
                        if (kiln_c99_text_append_c(&built, "%") != KILN_C99_OK ||
                            kiln_c99_text_append(&built, &character, 1) != KILN_C99_OK) goto format_oom;
                        next -= 1;
                    } else if (shown.tag != KILN_VALUE_STRING ||
                               kiln_c99_text_append(&built, shown.ptr, shown.len) != KILN_C99_OK) {
                        goto format_oom;
                    }
                }
            }
            while (next < argc) {
                kiln_value shown = kiln_c99_inspect(args[next++]);
                if (kiln_c99_text_append_c(&built, " ") != KILN_C99_OK ||
                    shown.tag != KILN_VALUE_STRING ||
                    kiln_c99_text_append(&built, shown.ptr, shown.len) != KILN_C99_OK)
                    goto format_oom;
            }
        }
        *out = kiln_c99_text_value(&built);
        return KILN_C99_OK;
format_oom:
        free(built.bytes);
        return KILN_C99_OUT_OF_MEMORY;
    }
    (void)host;
    (void)depth;
    KILN_C99_REFUSE("a util member this backend has not implemented");
}

static int kiln_c99_new_array(kiln_value *out) {
    kiln_array *array = (kiln_array *)kiln_c99_gc_alloc(KILN_GC_ARRAY, sizeof *array);
    if (array == NULL) {
        return KILN_C99_OUT_OF_MEMORY;
    }
    array->elements = NULL;
    array->count = 0;
    array->capacity = 0;
    array->properties = kiln_c99_undefined();
    array->frozen = 0;
    array->not_extensible = 0;
    *out = kiln_c99_value(KILN_VALUE_ARRAY, (uint64_t)(uintptr_t)array, NULL, 0);
    return KILN_C99_OK;
}

static size_t kiln_c99_typed_width(kiln_typed_kind kind) {
    switch (kind) {
    case KILN_TYPED_INT8:
    case KILN_TYPED_UINT8:
    case KILN_TYPED_UINT8_CLAMPED: return 1;
    case KILN_TYPED_INT16:
    case KILN_TYPED_UINT16: return 2;
    case KILN_TYPED_INT32:
    case KILN_TYPED_UINT32:
    case KILN_TYPED_FLOAT32: return 4;
    case KILN_TYPED_FLOAT64: return 8;
    }
    return 1;
}

static const char *kiln_c99_typed_name(kiln_typed_kind kind) {
    switch (kind) {
    case KILN_TYPED_INT8: return "Int8Array";
    case KILN_TYPED_UINT8: return "Uint8Array";
    case KILN_TYPED_UINT8_CLAMPED: return "Uint8ClampedArray";
    case KILN_TYPED_INT16: return "Int16Array";
    case KILN_TYPED_UINT16: return "Uint16Array";
    case KILN_TYPED_INT32: return "Int32Array";
    case KILN_TYPED_UINT32: return "Uint32Array";
    case KILN_TYPED_FLOAT32: return "Float32Array";
    case KILN_TYPED_FLOAT64: return "Float64Array";
    }
    return "TypedArray";
}

static int kiln_c99_builtin_typed_kind(uint64_t builtin, kiln_typed_kind *kind) {
    switch (builtin) {
    case KILN_C99_BUILTIN_INT8_ARRAY: *kind = KILN_TYPED_INT8; return 1;
    case KILN_C99_BUILTIN_UINT8_ARRAY: *kind = KILN_TYPED_UINT8; return 1;
    case KILN_C99_BUILTIN_UINT8_CLAMPED_ARRAY: *kind = KILN_TYPED_UINT8_CLAMPED; return 1;
    case KILN_C99_BUILTIN_INT16_ARRAY: *kind = KILN_TYPED_INT16; return 1;
    case KILN_C99_BUILTIN_UINT16_ARRAY: *kind = KILN_TYPED_UINT16; return 1;
    case KILN_C99_BUILTIN_INT32_ARRAY: *kind = KILN_TYPED_INT32; return 1;
    case KILN_C99_BUILTIN_UINT32_ARRAY: *kind = KILN_TYPED_UINT32; return 1;
    case KILN_C99_BUILTIN_FLOAT32_ARRAY: *kind = KILN_TYPED_FLOAT32; return 1;
    case KILN_C99_BUILTIN_FLOAT64_ARRAY: *kind = KILN_TYPED_FLOAT64; return 1;
    default: return 0;
    }
}

static uint64_t kiln_c99_typed_constructor(kiln_typed_kind kind) {
    switch (kind) {
    case KILN_TYPED_INT8: return KILN_C99_BUILTIN_INT8_ARRAY;
    case KILN_TYPED_UINT8: return KILN_C99_BUILTIN_UINT8_ARRAY;
    case KILN_TYPED_UINT8_CLAMPED: return KILN_C99_BUILTIN_UINT8_CLAMPED_ARRAY;
    case KILN_TYPED_INT16: return KILN_C99_BUILTIN_INT16_ARRAY;
    case KILN_TYPED_UINT16: return KILN_C99_BUILTIN_UINT16_ARRAY;
    case KILN_TYPED_INT32: return KILN_C99_BUILTIN_INT32_ARRAY;
    case KILN_TYPED_UINT32: return KILN_C99_BUILTIN_UINT32_ARRAY;
    case KILN_TYPED_FLOAT32: return KILN_C99_BUILTIN_FLOAT32_ARRAY;
    case KILN_TYPED_FLOAT64: return KILN_C99_BUILTIN_FLOAT64_ARRAY;
    }
    return KILN_C99_BUILTIN_UINT8_ARRAY;
}

static uint64_t kiln_c99_typed_prototype(kiln_typed_kind kind) {
    switch (kind) {
    case KILN_TYPED_INT8: return KILN_C99_BUILTIN_INT8_ARRAY_PROTOTYPE;
    case KILN_TYPED_UINT8: return KILN_C99_BUILTIN_UINT8_ARRAY_PROTOTYPE;
    case KILN_TYPED_UINT8_CLAMPED: return KILN_C99_BUILTIN_UINT8_CLAMPED_ARRAY_PROTOTYPE;
    case KILN_TYPED_INT16: return KILN_C99_BUILTIN_INT16_ARRAY_PROTOTYPE;
    case KILN_TYPED_UINT16: return KILN_C99_BUILTIN_UINT16_ARRAY_PROTOTYPE;
    case KILN_TYPED_INT32: return KILN_C99_BUILTIN_INT32_ARRAY_PROTOTYPE;
    case KILN_TYPED_UINT32: return KILN_C99_BUILTIN_UINT32_ARRAY_PROTOTYPE;
    case KILN_TYPED_FLOAT32: return KILN_C99_BUILTIN_FLOAT32_ARRAY_PROTOTYPE;
    case KILN_TYPED_FLOAT64: return KILN_C99_BUILTIN_FLOAT64_ARRAY_PROTOTYPE;
    }
    return KILN_C99_BUILTIN_UINT8_ARRAY_PROTOTYPE;
}

static int kiln_c99_typed_prototype_kind(uint64_t builtin, kiln_typed_kind *kind) {
    switch (builtin) {
    case KILN_C99_BUILTIN_INT8_ARRAY_PROTOTYPE: *kind = KILN_TYPED_INT8; return 1;
    case KILN_C99_BUILTIN_UINT8_ARRAY_PROTOTYPE: *kind = KILN_TYPED_UINT8; return 1;
    case KILN_C99_BUILTIN_UINT8_CLAMPED_ARRAY_PROTOTYPE:
        *kind = KILN_TYPED_UINT8_CLAMPED;
        return 1;
    case KILN_C99_BUILTIN_INT16_ARRAY_PROTOTYPE: *kind = KILN_TYPED_INT16; return 1;
    case KILN_C99_BUILTIN_UINT16_ARRAY_PROTOTYPE: *kind = KILN_TYPED_UINT16; return 1;
    case KILN_C99_BUILTIN_INT32_ARRAY_PROTOTYPE: *kind = KILN_TYPED_INT32; return 1;
    case KILN_C99_BUILTIN_UINT32_ARRAY_PROTOTYPE: *kind = KILN_TYPED_UINT32; return 1;
    case KILN_C99_BUILTIN_FLOAT32_ARRAY_PROTOTYPE: *kind = KILN_TYPED_FLOAT32; return 1;
    case KILN_C99_BUILTIN_FLOAT64_ARRAY_PROTOTYPE: *kind = KILN_TYPED_FLOAT64; return 1;
    default: return 0;
    }
}

static int kiln_c99_new_typed_buffer(kiln_value *out, size_t byte_length) {
    kiln_typed_buffer *buffer =
        (kiln_typed_buffer *)kiln_c99_gc_alloc(KILN_GC_TYPED_BUFFER, sizeof *buffer);
    if (buffer == NULL) {
        return KILN_C99_OUT_OF_MEMORY;
    }
    /* The bytes stay malloc-backed: nothing but this buffer ever points at them, and the
     * KILN_GC_TYPED_BUFFER finaliser is the one place they are released. */
    buffer->bytes = (uint8_t *)calloc(byte_length == 0 ? 1 : byte_length, 1);
    if (buffer->bytes == NULL) {
        kiln_c99_gc_release(buffer);
        return KILN_C99_OUT_OF_MEMORY;
    }
    buffer->byte_length = byte_length;
    *out = kiln_c99_value(KILN_VALUE_ARRAY_BUFFER, (uint64_t)(uintptr_t)buffer, NULL, 0);
    return KILN_C99_OK;
}

/* ECMAScript's ToIndex: missing/undefined/NaN become zero, but negative, infinite and unsafe
 * integer values are RangeErrors.  The size_t bound is separate because a 32-bit C target is
 * narrower than JavaScript's maximum safe integer.  Checking both before the cast keeps the
 * generated C out of the implementation-defined/undefined floating-to-integer range. */
static int kiln_c99_to_index(kiln_value value, size_t *out, kiln_value *thrown) {
    double numeric = kiln_c99_as_number(value);
    double integral;
    if (isnan(numeric) || numeric == 0.0) {
        *out = 0;
        return KILN_C99_OK;
    }
    integral = trunc(numeric);
    if (!isfinite(integral) || integral < 0.0 || integral > 9007199254740991.0 ||
        integral > (double)((size_t)-1)) {
        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_RANGE_ERROR,
                                    "invalid array buffer or typed array length");
    }
    *out = (size_t)integral;
    return KILN_C99_OK;
}

static double kiln_c99_typed_get(kiln_typed_array *view, size_t index) {
    size_t width = kiln_c99_typed_width(view->kind);
    size_t at;
    const uint8_t *bytes;
    uint32_t bits32;
    uint64_t bits64;
    int32_t signed32;
    int16_t signed16;
    uint16_t unsigned16;
    float float32;
    double float64;
    if (index >= view->length || view->byte_offset > view->buffer->byte_length ||
        index > (((size_t)-1) - view->byte_offset) / width) {
        return NAN;
    }
    at = view->byte_offset + index * width;
    if (at > view->buffer->byte_length || width > view->buffer->byte_length - at) {
        return NAN;
    }
    bytes = view->buffer->bytes + at;
    switch (view->kind) {
    case KILN_TYPED_INT8: return (double)(int8_t)bytes[0];
    case KILN_TYPED_UINT8:
    case KILN_TYPED_UINT8_CLAMPED: return (double)bytes[0];
    case KILN_TYPED_INT16:
        signed16 = (int16_t)((uint16_t)bytes[0] | ((uint16_t)bytes[1] << 8));
        return (double)signed16;
    case KILN_TYPED_UINT16:
        unsigned16 = (uint16_t)bytes[0] | ((uint16_t)bytes[1] << 8);
        return (double)unsigned16;
    case KILN_TYPED_INT32:
        bits32 = (uint32_t)bytes[0] | ((uint32_t)bytes[1] << 8) |
                 ((uint32_t)bytes[2] << 16) | ((uint32_t)bytes[3] << 24);
        memcpy(&signed32, &bits32, sizeof signed32);
        return (double)signed32;
    case KILN_TYPED_UINT32:
        bits32 = (uint32_t)bytes[0] | ((uint32_t)bytes[1] << 8) |
                 ((uint32_t)bytes[2] << 16) | ((uint32_t)bytes[3] << 24);
        return (double)bits32;
    case KILN_TYPED_FLOAT32:
        bits32 = (uint32_t)bytes[0] | ((uint32_t)bytes[1] << 8) |
                 ((uint32_t)bytes[2] << 16) | ((uint32_t)bytes[3] << 24);
        memcpy(&float32, &bits32, sizeof float32);
        return (double)float32;
    case KILN_TYPED_FLOAT64:
        bits64 = (uint64_t)bytes[0] | ((uint64_t)bytes[1] << 8) |
                 ((uint64_t)bytes[2] << 16) | ((uint64_t)bytes[3] << 24) |
                 ((uint64_t)bytes[4] << 32) | ((uint64_t)bytes[5] << 40) |
                 ((uint64_t)bytes[6] << 48) | ((uint64_t)bytes[7] << 56);
        memcpy(&float64, &bits64, sizeof float64);
        return float64;
    }
    return NAN;
}

static void kiln_c99_typed_set(kiln_typed_array *view, size_t index, double value) {
    size_t width = kiln_c99_typed_width(view->kind);
    size_t at;
    uint8_t *bytes;
    uint32_t bits32;
    uint64_t bits64;
    int32_t signed32;
    int16_t signed16;
    uint16_t unsigned16;
    float float32;
    double float64;
    double rounded;
    if (index >= view->length || view->byte_offset > view->buffer->byte_length ||
        index > (((size_t)-1) - view->byte_offset) / width) {
        return;
    }
    at = view->byte_offset + index * width;
    if (at > view->buffer->byte_length || width > view->buffer->byte_length - at) {
        return;
    }
    bytes = view->buffer->bytes + at;
    switch (view->kind) {
    case KILN_TYPED_INT8: bytes[0] = (uint8_t)kiln_c99_to_uint32(value); return;
    case KILN_TYPED_UINT8:
        bytes[0] = (uint8_t)kiln_c99_to_uint32(value);
        return;
    case KILN_TYPED_UINT8_CLAMPED:
        if (isnan(value) || value <= 0.0) {
            bytes[0] = 0;
        } else if (value >= 255.0) {
            bytes[0] = 255;
        } else {
            rounded = floor(value);
            if (value - rounded > 0.5 ||
                (value - rounded == 0.5 && fmod(rounded, 2.0) != 0.0)) {
                rounded += 1.0;
            }
            bytes[0] = (uint8_t)rounded;
        }
        return;
    case KILN_TYPED_INT16:
        signed16 = (int16_t)kiln_c99_to_uint32(value);
        unsigned16 = (uint16_t)signed16;
        bytes[0] = (uint8_t)unsigned16;
        bytes[1] = (uint8_t)(unsigned16 >> 8);
        return;
    case KILN_TYPED_UINT16:
        unsigned16 = (uint16_t)kiln_c99_to_uint32(value);
        bytes[0] = (uint8_t)unsigned16;
        bytes[1] = (uint8_t)(unsigned16 >> 8);
        return;
    case KILN_TYPED_INT32:
        signed32 = (int32_t)kiln_c99_to_uint32(value);
        memcpy(&bits32, &signed32, sizeof bits32);
        bytes[0] = (uint8_t)bits32;
        bytes[1] = (uint8_t)(bits32 >> 8);
        bytes[2] = (uint8_t)(bits32 >> 16);
        bytes[3] = (uint8_t)(bits32 >> 24);
        return;
    case KILN_TYPED_UINT32:
        bits32 = kiln_c99_to_uint32(value);
        bytes[0] = (uint8_t)bits32;
        bytes[1] = (uint8_t)(bits32 >> 8);
        bytes[2] = (uint8_t)(bits32 >> 16);
        bytes[3] = (uint8_t)(bits32 >> 24);
        return;
    case KILN_TYPED_FLOAT32:
        float32 = (float)value;
        memcpy(&bits32, &float32, sizeof bits32);
        bytes[0] = (uint8_t)bits32;
        bytes[1] = (uint8_t)(bits32 >> 8);
        bytes[2] = (uint8_t)(bits32 >> 16);
        bytes[3] = (uint8_t)(bits32 >> 24);
        return;
    case KILN_TYPED_FLOAT64:
        float64 = value;
        memcpy(&bits64, &float64, sizeof bits64);
        bytes[0] = (uint8_t)bits64;
        bytes[1] = (uint8_t)(bits64 >> 8);
        bytes[2] = (uint8_t)(bits64 >> 16);
        bytes[3] = (uint8_t)(bits64 >> 24);
        bytes[4] = (uint8_t)(bits64 >> 32);
        bytes[5] = (uint8_t)(bits64 >> 40);
        bytes[6] = (uint8_t)(bits64 >> 48);
        bytes[7] = (uint8_t)(bits64 >> 56);
        return;
    }
}

static int kiln_c99_new_typed_array(kiln_value *out, kiln_typed_kind kind,
                                    kiln_value source, size_t byte_offset, size_t length,
                                    int has_length, kiln_value *thrown) {
    size_t width = kiln_c99_typed_width(kind);
    kiln_value buffer_value;
    kiln_typed_array *view;
    size_t count;
    size_t index;
    int status;
    if (source.tag == KILN_VALUE_ARRAY_BUFFER) {
        kiln_typed_buffer *buffer = kiln_c99_typed_buffer_of(source);
        if (byte_offset % width != 0 || byte_offset > buffer->byte_length ||
            (!has_length && (buffer->byte_length - byte_offset) % width != 0) ||
            (has_length && (length > (buffer->byte_length - byte_offset) / width))) {
            return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_RANGE_ERROR,
                                        "invalid typed array length or offset");
        }
        count = has_length ? length : (buffer->byte_length - byte_offset) / width;
        view = (kiln_typed_array *)kiln_c99_gc_alloc(KILN_GC_TYPED_ARRAY, sizeof *view);
        if (view == NULL) {
            return KILN_C99_OUT_OF_MEMORY;
        }
        view->buffer = buffer;
        view->byte_offset = byte_offset;
        view->length = count;
        view->kind = kind;
        view->is_buffer = 0;
        *out = kiln_c99_value(KILN_VALUE_TYPED_ARRAY, (uint64_t)(uintptr_t)view, NULL, 0);
        return KILN_C99_OK;
    }
    if (source.tag == KILN_VALUE_ARRAY) {
        count = kiln_c99_array_of(source)->count;
    } else if (source.tag == KILN_VALUE_TYPED_ARRAY) {
        count = kiln_c99_typed_array_of(source)->length;
    } else {
        status = kiln_c99_to_index(source, &count, thrown);
        if (status != KILN_C99_OK) {
            return status;
        }
    }
    if (count > ((size_t)-1) / width) {
        return KILN_C99_OUT_OF_MEMORY;
    }
    status = kiln_c99_new_typed_buffer(&buffer_value, count * width);
    if (status != KILN_C99_OK) {
        return status;
    }
    view = (kiln_typed_array *)kiln_c99_gc_alloc(KILN_GC_TYPED_ARRAY, sizeof *view);
    if (view == NULL) {
        return KILN_C99_OUT_OF_MEMORY;
    }
    view->buffer = kiln_c99_typed_buffer_of(buffer_value);
    view->byte_offset = 0;
    view->length = count;
    view->kind = kind;
    view->is_buffer = 0;
    for (index = 0; index < count; ++index) {
        double item = source.tag == KILN_VALUE_ARRAY
                          ? kiln_c99_as_number(kiln_c99_array_of(source)->elements[index])
                      : source.tag == KILN_VALUE_TYPED_ARRAY
                          ? kiln_c99_typed_get(kiln_c99_typed_array_of(source), index)
                          : 0.0;
        kiln_c99_typed_set(view, index, item);
    }
    *out = kiln_c99_value(KILN_VALUE_TYPED_ARRAY, (uint64_t)(uintptr_t)view, NULL, 0);
    return KILN_C99_OK;
}

/* An all-zero view of `count` elements over a buffer of its own.
 *
 * `map`, `filter` and `slice` each answer with a *view of the receiver's kind* rather than with a
 * plain array: `new Uint8Array([3, 1, 2]).map(f)` is a `Uint8Array`, and an Array would lose both
 * the width and the wrapping that decide what the values mean. */
static int kiln_c99_typed_alloc(kiln_value *out, kiln_typed_kind kind, size_t count) {
    size_t width = kiln_c99_typed_width(kind);
    kiln_value buffer;
    kiln_typed_array *view;
    int status;
    if (count > ((size_t)-1) / width) {
        return KILN_C99_OUT_OF_MEMORY;
    }
    status = kiln_c99_new_typed_buffer(&buffer, count * width);
    if (status != KILN_C99_OK) {
        return status;
    }
    view = (kiln_typed_array *)kiln_c99_gc_alloc(KILN_GC_TYPED_ARRAY, sizeof *view);
    if (view == NULL) {
        return KILN_C99_OUT_OF_MEMORY;
    }
    view->buffer = kiln_c99_typed_buffer_of(buffer);
    view->byte_offset = 0;
    view->length = count;
    view->kind = kind;
    view->is_buffer = 0;
    *out = kiln_c99_value(KILN_VALUE_TYPED_ARRAY, (uint64_t)(uintptr_t)view, NULL, 0);
    return KILN_C99_OK;
}

/* `new DataView(buffer[, byteOffset[, byteLength]])`.
 *
 * The two failures are different errors and a program tells them apart: a first argument that is
 * not an ArrayBuffer is a TypeError, and a window that does not fit inside the buffer is a
 * RangeError. Both messages are the interpreter's, word for word -- `error.message` is
 * program-visible, so a fault the two engines spell differently is a divergence like any other.
 *
 * There is no alignment rule to check, which is the other half of why this type exists: byte
 * offset 1 is a legal DataView and an illegal `Uint16Array`, and `kiln_c99_new_typed_array`'s
 * `byte_offset % width` test has no counterpart here. */
static int kiln_c99_new_data_view(kiln_value *out, kiln_value source, size_t byte_offset,
                                  size_t byte_length, int has_length, kiln_value *thrown) {
    kiln_typed_buffer *buffer;
    kiln_data_view *view;
    if (source.tag != KILN_VALUE_ARRAY_BUFFER) {
        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                    "Method called on an incompatible receiver");
    }
    buffer = kiln_c99_typed_buffer_of(source);
    if (byte_offset > buffer->byte_length) {
        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_RANGE_ERROR,
                                    "Offset is outside the bounds of the DataView");
    }
    /* A subtraction from a quantity already known to be large enough, never `byte_offset +
     * byte_length`: both are size_t, that sum can wrap, and a wrapped sum compares *small* --
     * which would admit exactly the window this test exists to reject. */
    if (has_length && byte_length > buffer->byte_length - byte_offset) {
        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_RANGE_ERROR,
                                    "Offset is outside the bounds of the DataView");
    }
    view = (kiln_data_view *)kiln_c99_gc_alloc(KILN_GC_DATA_VIEW, sizeof *view);
    if (view == NULL) {
        return KILN_C99_OUT_OF_MEMORY;
    }
    view->buffer = buffer;
    view->byte_offset = byte_offset;
    /* With no explicit length the view runs to the end of the buffer, and keeps running there:
     * nothing here can resize a buffer, so the length is settled once rather than tracked. */
    view->byte_length = has_length ? byte_length : buffer->byte_length - byte_offset;
    *out = kiln_c99_value(KILN_VALUE_DATA_VIEW, (uint64_t)(uintptr_t)view, NULL, 0);
    return KILN_C99_OK;
}

/* Where an access of `width` bytes at `offset` lands, or zero when it does not fit entirely
 * inside the window.
 *
 * Every bound is a subtraction, for the reason given in the constructor: `offset + width` on two
 * size_t values can wrap, and the wrapped sum is small enough to pass a naive comparison. Written
 * this way, an access that ends *exactly* at the last byte is admitted (`offset == byte_length -
 * width`) and one byte further is not. */
static int kiln_c99_data_view_at(kiln_data_view *view, size_t offset, size_t width,
                                 uint8_t **out) {
    if (view->byte_length < width || offset > view->byte_length - width) {
        return 0;
    }
    /* The window was measured against the buffer when the view was made and no buffer here can
     * shrink afterwards. Asked again anyway, because this is the test that decides whether these
     * particular bytes are ours to touch, and a mistake in it is a read out of bounds rather than
     * a wrong number. */
    if (view->byte_offset > view->buffer->byte_length ||
        view->byte_length > view->buffer->byte_length - view->byte_offset) {
        return 0;
    }
    *out = view->buffer->bytes + view->byte_offset + offset;
    return 1;
}

/* Gather `width` bytes into an unsigned integer, most significant byte first unless the call
 * asked for little-endian.
 *
 * BIG-ENDIAN IS THE DEFAULT, and it is the one thing about this type that surprises everybody. A
 * typed array is always the platform's order and cannot be asked for another; a DataView is
 * always network order unless the call passes a truthy `littleEndian`. So `dv.getUint16(0)` and
 * `new Uint16Array(b)[0]` disagree on every machine this runs on, and both are right -- which is
 * why getting the default backwards is invisible until the bytes meet something outside the
 * program.
 *
 * A byte loop rather than a `memcpy` and a swap: the width is 1, 2, 4 or 8 and the buffer offset
 * is arbitrary, so there is no aligned load to make anyway. Unaligned access is the ordinary case
 * here, not the exception. */
static uint64_t kiln_c99_data_view_gather(const uint8_t *bytes, size_t width, int little_endian) {
    uint64_t bits = 0;
    size_t index;
    for (index = 0; index < width; ++index) {
        bits = (bits << 8) | (uint64_t)bytes[little_endian ? width - 1 - index : index];
    }
    return bits;
}

/* The inverse, and deliberately the same shape: one function per direction, each reading the flag
 * the same way, so the pair cannot disagree about which way the reversal goes. A disagreement
 * there round-trips perfectly through the same view and only shows once the bytes leave. */
static void kiln_c99_data_view_scatter(uint8_t *bytes, size_t width, int little_endian,
                                       uint64_t bits) {
    size_t index;
    for (index = 0; index < width; ++index) {
        bytes[little_endian ? index : width - 1 - index] = (uint8_t)(bits & UINT64_C(0xFF));
        bits >>= 8;
    }
}

/* An unsigned 64-bit value as decimal digits, most significant first, answering the length.
 *
 * `out` needs room for twenty, which is what 18446744073709551615 takes. Written out rather than
 * reached through `snprintf` because `%llu` is the one length modifier this backend cannot assume
 * (ADR 0013 keeps the number formatting in the provider for the same reason). */
static size_t kiln_c99_uint64_digits(uint64_t value, char *out) {
    char reversed[20];
    size_t count = 0;
    size_t index;
    do {
        reversed[count++] = (char)('0' + (int)(value % 10));
        value /= 10;
    } while (value != 0);
    for (index = 0; index < count; ++index) {
        out[index] = reversed[count - 1 - index];
    }
    return count;
}

/* `0xFFFFFFFFFFFFFFFF`, built once.
 *
 * `BigInt.asUintN(64, v)` spelled as the provider's own bitwise AND, which is what
 * `setBigInt64`/`setBigUint64` need: the language *wraps* there rather than refusing, so
 * `setBigUint64(0, -1n)` stores all ones and `setBigInt64(0, 2n ** 200n)` stores the low sixty-
 * four bits. `kiln_bigint_to_i128` alone would have called the second of those out of range.
 *
 * Nothing frees a BigInt handle here, so a shared one is safe to keep -- the same bargain
 * `kiln_c99_bigint_one` makes for `1n`. */
static const void *kiln_c99_bigint_uint64_mask(void) {
    static void *mask = NULL;
    if (mask == NULL) {
        static const unsigned char digits[] = "18446744073709551615";
        if (kiln_bigint_parse(digits, sizeof digits - 1, 10, &mask) != 0) {
            return NULL;
        }
    }
    return mask;
}

/* The order `%TypedArray%.prototype.sort` uses with no comparator: ascending *numeric*.
 *
 * Not `Array.prototype.sort`'s order, which spells its elements out and compares the text --
 * `[10, 9].sort()` is `[10, 9]` and `new Uint8Array([10, 9]).sort()` is `[9, 10]`. A view holds
 * numbers and nothing else, so there is nothing to spell out. NaN sorts last and `-0` before
 * `+0`, which are the two places the ordinary comparison has no answer. */
static int kiln_c99_typed_order(double left, double right) {
    int left_nan = left != left;
    int right_nan = right != right;
    if (left_nan || right_nan) {
        return left_nan - right_nan;
    }
    if (left < right) {
        return -1;
    }
    if (left > right) {
        return 1;
    }
    return kiln_c99_signbit(right) - kiln_c99_signbit(left);
}

/* Doubling, so building an object or an array one member at a time stays linear overall. */
static int kiln_c99_reserve(void **items, size_t *capacity, size_t needed, size_t item_size) {
    size_t grown = *capacity == 0 ? 4 : *capacity;
    void *moved;
    if (needed <= *capacity) {
        return KILN_C99_OK;
    }
    while (grown < needed) {
        if (grown > ((size_t)-1) / 2 / item_size) {
            return KILN_C99_OUT_OF_MEMORY;
        }
        grown *= 2;
    }
    moved = realloc(*items, grown * item_size);
    if (moved == NULL) {
        return KILN_C99_OUT_OF_MEMORY;
    }
    *items = moved;
    *capacity = grown;
    return KILN_C99_OK;
}

/* The index a key names, or -1.  A key is an index only when it round-trips through the
 * canonical decimal spelling, which is what keeps `a["01"]` a named property and `a["1"]` an
 * element -- the distinction the language draws and the one a naive `atoi` erases. */
static double kiln_c99_key_index(const char *key, size_t key_len) {
    char buffer[64];
    double number;
    int length;
    /* Every canonical index spelling is one to ten ASCII digits with no sign and no leading
     * space: the largest index is 4294967294, and a number that round-trips cannot begin with
     * anything but a digit.  Both are *necessary* conditions, so failing either is a decision
     * this can make without the provider -- which matters because the provider call is a
     * cross-crate FFI hop and every property write on every object makes one.  Nothing about
     * `a["01"]` or `a["1"]` moves: the round-trip below still decides those. */
    if (key_len == 0 || key_len > 10 ||
        (unsigned char)(key[0] - '0') > 9u) {
        return -1.0;
    }
    number = kiln_string_to_number(key, key_len);
    if (!(number >= 0.0) || floor(number) != number || number > 4294967294.0) {
        return -1.0;
    }
    length = kiln_number_to_js_string(number, buffer, sizeof(buffer));
    if (length < 0 || (size_t)length != key_len || memcmp(buffer, key, key_len) != 0) {
        return -1.0;
    }
    return number;
}

static int kiln_c99_same_name(const char *key, size_t key_len, const char *name) {
    size_t len = strlen(name);
    return len == key_len && memcmp(key, name, len) == 0;
}

static int kiln_c99_same_key(const kiln_property *property, const char *key, size_t key_len) {
    if (property->key_len != key_len) {
        return 0;
    }
    if (key_len == 0) {
        return 1;
    }
    /* Two pointers into the same bytes are the same key -- trivially, not by assumption -- and
     * are common: an emitted constant is one static array shared by every read and write of a
     * given literal name, and a property that was itself interned this way keeps that pointer
     * when it is looked up again by the same literal. Different pointers say nothing either way,
     * which is why this falls through to a real comparison rather than refusing on the strength
     * of not matching -- a dynamic key can hold the same bytes as an interned one at some other
     * address. */
    if (property->key == key) {
        return 1;
    }
    if (property->key[0] != key[0]) {
        return 0;
    }
    return memcmp(property->key, key, key_len) == 0;
}

/* The slot on `object` holding the canonical key `ikey`, or `object->count` when there is none.
 * A NULL `ikey` is a spelling the realm never interned, and since every stored key is interned
 * (`kiln_c99_own_slot` is the one place a key is written and it always canonicalizes) that is a
 * key no object has: the scan is skipped rather than run. `KILN_KEYS_CHECK=1` re-runs it by
 * bytes and aborts on a disagreement, which is how "every stored key is interned" is tested
 * rather than assumed. */
static size_t kiln_c99_own_index(const kiln_object *object, const char *ikey, const char *key,
                                 size_t key_len) {
    size_t index;
    size_t found = object->count;
    if (ikey != NULL) {
        for (index = 0; index < object->count; ++index) {
            if (object->properties[index].key == ikey) {
                found = index;
                break;
            }
        }
    }
    if (kiln_c99_keys_checked()) {
        size_t by_bytes = object->count;
        for (index = 0; index < object->count; ++index) {
            if (kiln_c99_same_key(&object->properties[index], key, key_len)) {
                by_bytes = index;
                break;
            }
        }
        if (by_bytes != found) {
            fputs("kiln: interned-key invariant broken for `", stderr);
            fwrite(key, 1, key_len, stderr);
            fputs(found == object->count ? "`: stored under a non-canonical pointer\n"
                                         : "`: two slots answer one key\n",
                  stderr);
            abort();
        }
    }
    return found;
}

/* A write to an existing key replaces it in place, so insertion order is the order of first
 * appearance -- which is the order `Object.keys` reports and the order a console prints. */
static int kiln_c99_put_mode(kiln_host *host, kiln_value target, const char *key, size_t key_len,
                             kiln_value value, int strict, int depth, kiln_value *thrown) {
    return kiln_c99_put_mode_ikey(host, target, key, key_len, NULL, value, strict, depth, thrown);
}

/* `ikey` as for `kiln_c99_get_ikey`: a compiled site's canonical pointer, or NULL to look it
 * up. */
static int kiln_c99_put_mode_ikey(kiln_host *host, kiln_value target, const char *key,
                                  size_t key_len, const char *ikey, kiln_value value, int strict,
                                  int depth, kiln_value *thrown) {
    kiln_object *object;
    size_t index;
    int status;
    /* Node resource pointers are represented by reserved values in the ordinary property store,
     * but they are not JavaScript properties.  A program can spell the key as a string, so hide
     * and ignore these protocol slots explicitly instead of exposing a C pointer as an object or
     * allowing an assignment to replace it. */
    if (kiln_c99_is_node_hidden_key(key, key_len)) {
        return KILN_C99_OK;
    }
    if (target.tag == KILN_VALUE_PROXY) {
        /* Writing to a proxy is a call, exactly as reading one is. */
        return kiln_c99_proxy_set(host, target,
                                  kiln_c99_value(KILN_VALUE_STRING, 0, key, key_len), value,
                                  strict, depth, thrown);
    }
    if (target.tag == KILN_VALUE_FUNCTION) {
        kiln_closure *closure = kiln_c99_closure_of(target);
        /* `f.prototype = {...}` replaces the object `new f()` links instances to, which is a
         * slot on the closure rather than an ordinary property -- a write that landed in the
         * store would be read back by `f.prototype` and ignored by `new`. */
        if (kiln_c99_same_name(key, key_len, "prototype")) {
            if (closure->properties.tag == KILN_VALUE_OBJECT &&
                kiln_c99_object_of(closure->properties)->frozen) {
                return strict ? kiln_c99_throw_read_only(thrown, key, key_len) : KILN_C99_OK;
            }
            closure->prototype = value;
            return KILN_C99_OK;
        }
        if (closure->properties.tag != KILN_VALUE_OBJECT) {
            status = kiln_c99_new_object(&closure->properties);
            if (status != KILN_C99_OK) {
                return status;
            }
        }
        return kiln_c99_put_mode(host, closure->properties, key, key_len, value, strict, depth,
                                 thrown);
    }
    if (target.tag == KILN_VALUE_METHOD || target.tag == KILN_VALUE_BUILTIN ||
        target.tag == KILN_VALUE_BOUND) {
        kiln_value store;
        status = kiln_c99_callable_store(target, &store, 1);
        if (status != KILN_C99_OK) return status;
        return kiln_c99_put_mode(host, store, key, key_len, value, strict, depth, thrown);
    }
    if (target.tag == KILN_VALUE_REGEXP && kiln_c99_same_name(key, key_len, "lastIndex")) {
        /* The one property of a regexp a program may write, and the reason two evaluations of one
         * literal must not share a `kiln_regexp`. */
        kiln_c99_regexp_of(target)->last_index = kiln_c99_as_number(value);
        return KILN_C99_OK;
    }
    if (target.tag == KILN_VALUE_ARRAY) {
        /* An array is not an object here, but it can still carry a property that is not an
         * element -- which is how `exec` reports `index` and `input`. */
        kiln_array *array = kiln_c99_array_of(target);
        if (kiln_c99_same_name(key, key_len, "length")) {
            /* `length` is derived, not stored, so writing it is a resize: truncating drops the
             * tail and growing fills with holes, which are not undefined elements. */
            double wanted = kiln_c99_as_number(value);
            size_t at;
            if (!(wanted >= 0.0) || floor(wanted) != wanted || wanted > 4294967295.0) {
                /* A RangeError rather than a write that quietly does nothing. */
                return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_RANGE_ERROR,
                                            "Invalid array length");
            }
            if (array->frozen) {
                return strict ? kiln_c99_throw_read_only(thrown, key, key_len) : KILN_C99_OK;
            }
            if ((size_t)wanted <= array->count) {
                array->count = (size_t)wanted;
                return KILN_C99_OK;
            }
            status = kiln_c99_reserve((void **)&array->elements, &array->capacity, (size_t)wanted,
                                      sizeof *array->elements);
            if (status != KILN_C99_OK) {
                return status;
            }
            for (at = array->count; at < (size_t)wanted; ++at) {
                array->elements[at] = kiln_c99_hole();
            }
            array->count = (size_t)wanted;
            return KILN_C99_OK;
        }
        if (array->frozen) {
            return strict ? kiln_c99_throw_read_only(thrown, key, key_len) : KILN_C99_OK;
        }
        if (array->properties.tag != KILN_VALUE_OBJECT) {
            status = kiln_c99_new_object(&array->properties);
            if (status != KILN_C99_OK) {
                return status;
            }
        }
        return kiln_c99_put_mode(host, array->properties, key, key_len, value, strict, depth,
                                 thrown);
    }
    if (target.tag != KILN_VALUE_OBJECT) {
        /* A primitive has nowhere to put a property, and every module is strict, so the language
         * ends the statement with a catchable TypeError rather than dropping the write. The
         * object-like tags that also land here -- a Date, a collection, a promise, a bound
         * function -- are a different thing: those *are* objects a program may write to, and
         * this backend simply has no store for them yet, so they keep saying so.
         *
         * A collection has a store, but only for the prototype a subclass gave it: nothing that
         * enumerates -- `Object.keys`, `JSON.stringify`, a console -- looks in it, so a write
         * accepted here would read back correctly and be invisible everywhere else. Refusing is
         * the honest answer until those paths know about it. */
        if (target.tag == KILN_VALUE_UNDEFINED || target.tag == KILN_VALUE_NULL ||
            target.tag == KILN_VALUE_BOOLEAN || target.tag == KILN_VALUE_NUMBER ||
            target.tag == KILN_VALUE_STRING || target.tag == KILN_VALUE_BIGINT ||
            target.tag == KILN_VALUE_SYMBOL) {
                return strict ? kiln_c99_throw_cannot_set(thrown, target) : KILN_C99_OK;
        }
        KILN_C99_REFUSE("a property write to a value whose store nothing enumerates");
    }
    object = kiln_c99_object_of(target);
    if (object->module_namespace) {
        return strict ? kiln_c99_throw_module_namespace_write(thrown, key, key_len)
                       : KILN_C99_OK;
    }
    if (object->frozen) {
        return strict ? kiln_c99_throw_read_only(thrown, key, key_len) : KILN_C99_OK;
    }
    /* Interned here, once, rather than looked up here and interned again in `own_slot` when
     * the write turns out to create the property: a fresh object being filled by the prelude --
     * a request, a response, a URL -- is a run of new-key writes, and each was paying two
     * hashes. A refused write leaves a weak entry behind, which the next collection drops. */
    if (ikey == NULL) {
        ikey = kiln_c99_intern(key, key_len);
        if (ikey == NULL) {
            return KILN_C99_OUT_OF_MEMORY;
        }
    }
    if (kiln_c99_same_name(key, key_len, "__proto__") &&
        object->prototype.tag != KILN_VALUE_NULL) {
        /* The other half of the accessor above: a write relinks the object instead of giving it
         * a property called `__proto__`. A primitive is ignored rather than refused, which is the
         * setter's rule and the literal's -- `Object.setPrototypeOf` is the spelling that
         * complains. */
        if (kiln_c99_can_be_prototype(value)) {
            object->prototype = value;
        }
        return KILN_C99_OK;
    }
    /* A setter found anywhere on the chain takes the write; only when there is none does the
     * assignment create an own data property. That asymmetry is what makes `obj.x = 1` on a
     * class with `set x()` run the setter rather than shadow it.
     *
     * The own list is checked on its own, first: an own *data* property shadows every inherited
     * setter -- per the specification, [[Set]] never consults the prototype once the receiver's
     * own property is found -- so a hit here ends the search without a single prototype touched.
     * That is most writes on a request or response past its first: the key is already there. */
    index = kiln_c99_own_index(object, ikey, key, key_len);
    if (index != object->count) {
        if (!object->properties[index].accessor) {
            /* A refused write is observable only in strict code. */
            if (object->properties[index].writable) {
                object->properties[index].value = value;
            } else if (strict) {
                return kiln_c99_throw_read_only(thrown, key, key_len);
            }
            return KILN_C99_OK;
        }
        if (object->properties[index].setter.tag == KILN_VALUE_UNDEFINED) {
            if (strict) {
                return kiln_c99_throw_read_only(thrown, key, key_len);
            }
            return KILN_C99_OK;
        }
        {
            kiln_value ignored;
            return kiln_c99_call(host, &ignored, object->properties[index].setter, target, &value,
                                 1, depth, thrown);
        }
    }
    /* Nothing of this name on the object itself. Walk the prototypes looking for a setter to
     * run, or a non-writable data property that blocks creating an own one in its place -- a
     * level with no accessors of its own has no setter to offer, so `accessor_count == 0` skips
     * scanning its property list at all and the level costs one pointer follow instead, unless a
     * data property of that level was ever made read-only (`read_only_seen`), which is the other
     * thing a prototype can do to a write. */
    {
        kiln_value current = object->prototype;
        size_t steps;
        for (steps = 0; steps < KILN_C99_MAX_PROTOTYPE_DEPTH; ++steps) {
            kiln_object *walked;
            if (current.tag != KILN_VALUE_OBJECT) {
                break;
            }
            walked = kiln_c99_object_of(current);
            if (walked->accessor_count != 0 || walked->read_only_seen) {
                index = kiln_c99_own_index(walked, ikey, key, key_len);
                if (index != walked->count) {
                    if (!walked->properties[index].accessor) {
                        if (!walked->properties[index].writable) {
                            return strict ? kiln_c99_throw_read_only(thrown, key, key_len)
                                           : KILN_C99_OK;
                        }
                        /* A writable data property up the chain: nothing intercepts, and the
                         * write shadows it with an own property below. */
                        break;
                    }
                    if (walked->properties[index].setter.tag == KILN_VALUE_UNDEFINED) {
                        if (strict) {
                            return kiln_c99_throw_read_only(thrown, key, key_len);
                        }
                        return KILN_C99_OK;
                    }
                    {
                        kiln_value ignored;
                        return kiln_c99_call(host, &ignored, walked->properties[index].setter,
                                             target, &value, 1, depth, thrown);
                    }
                }
            }
            current = walked->prototype;
        }
    }
    {
        kiln_property *slot;
        /* A non-extensible object takes a write to a property it already has and refuses to grow.
         * The walk above has already handled every own property, so arriving here with the flag
         * set means this key would be a new one. The wording is the frozen write's on purpose:
         * the interpreter words both the same, and `error.message` is program-visible. */
        if (object->not_extensible) {
            if (kiln_c99_own_index(object, ikey, key, key_len) == object->count) {
                return strict ? kiln_c99_throw_read_only(thrown, key, key_len) : KILN_C99_OK;
            }
        }
        status = kiln_c99_own_slot_ikey(target, key, key_len, ikey, &slot);
        if (status != KILN_C99_OK) {
            return status;
        }
        slot->value = value;
    }
    return KILN_C99_OK;
}

static int kiln_c99_put(kiln_host *host, kiln_value target, const char *key, size_t key_len,
                        kiln_value value, int depth, kiln_value *thrown) {
    return kiln_c99_put_mode(host, target, key, key_len, value, 0, depth, thrown);
}

/* Find or create the own property named `key`, bypassing the prototype chain entirely.
 *
 * An assignment cannot start here -- a setter anywhere above has first claim, and `kiln_c99_put`
 * walks for one before it arrives -- but a *definition* must: `Object.defineProperty` writes
 * past an inherited setter rather than through it, and reaching this by way of the assignment
 * path would call one.
 *
 * A fresh slot is what an assignment would have made: writable, configurable, enumerable, data.
 * A definition then overwrites exactly the fields its descriptor mentions. */
static int kiln_c99_own_slot(kiln_value target, const char *key, size_t key_len,
                             kiln_property **out) {
    return kiln_c99_own_slot_ikey(target, key, key_len, NULL, out);
}

/* `ikey` is the canonical pointer when the caller holds one that is already in the table -- a
 * compiled site's, or a write path's successful lookup -- or NULL to intern here. */
static int kiln_c99_own_slot_ikey(kiln_value target, const char *key, size_t key_len,
                                  const char *ikey, kiln_property **out) {
    kiln_object *object;
    size_t index;
    int status;
    if (target.tag != KILN_VALUE_OBJECT) {
        KILN_C99_REFUSE("an own-slot lookup on a value that is not an ordinary object");
    }
    object = kiln_c99_object_of(target);
    /* The one place a key is stored, so the one place interning has to be total (ADR 0047):
     * whatever spelling arrived, the property carries the realm's canonical pointer for it. */
    if (ikey == NULL) {
        ikey = kiln_c99_intern(key, key_len);
        if (ikey == NULL) {
            return KILN_C99_OUT_OF_MEMORY;
        }
    }
    index = kiln_c99_own_index(object, ikey, key, key_len);
    if (index != object->count) {
        *out = &object->properties[index];
        return KILN_C99_OK;
    }
    status = kiln_c99_reserve((void **)&object->properties, &object->capacity, object->count + 1,
                              sizeof *object->properties);
    if (status != KILN_C99_OK) {
        return status;
    }
    /* Integer-like keys come first, in ascending numeric order, and everything else follows in
     * insertion order.  That is the order the language specifies for own properties, so it is
     * the order a console prints and the order `Object.keys` would report -- keeping the list
     * sorted on insert means nothing downstream has to re-derive it. */
    {
        double slot_index = kiln_c99_key_index(key, key_len);
        size_t position = object->count;
        if (slot_index >= 0.0) {
            for (index = 0; index < object->count; ++index) {
                double existing = kiln_c99_key_index(object->properties[index].key,
                                                     object->properties[index].key_len);
                if (existing < 0.0 || existing > slot_index) {
                    break;
                }
            }
            position = index;
        }
        for (index = object->count; index > position; --index) {
            object->properties[index] = object->properties[index - 1];
        }
        object->properties[position].key = ikey;
        object->properties[position].key_len = key_len;
        /* A `@@` key is the machine's, not the program's: a private member, an error's kind, a
         * combinator's bookkeeping. Nothing that enumerates own string properties may see one, and
         * marking it here is one place rather than at each of the seven walks. The program cannot
         * spell such a key -- `obj["@@x"]` is a different property only because nothing in the
         * language produces this prefix. */
        object->properties[position].enumerable =
            !(key_len >= 2 && key[0] == '@' && key[1] == '@');
        object->properties[position].writable = 1;
        object->properties[position].configurable = 1;
        object->properties[position].accessor = 0;
        object->properties[position].refused = 0;
        object->properties[position].value = kiln_c99_undefined();
        object->properties[position].getter = kiln_c99_undefined();
        object->properties[position].setter = kiln_c99_undefined();
        *out = &object->properties[position];
    }
    object->count += 1;
    return KILN_C99_OK;
}

/* Write an own data property the way an assignment would have left it, without asking the
 * prototype chain for permission first.
 *
 * `kiln_c99_put` walks the chain for a setter before every write, because for *program* code a
 * setter above has first claim.  The runtime's own initialisers are not program code: they fill
 * a freshly created object whose prototype is one the runtime just linked, so no setter can
 * exist to claim the write and the walk can only ever fail.  Building a `node:http` request ran
 * that walk twelve times per request over a three-deep prototype chain, for nothing.
 *
 * This is *not* a faster `put`.  It skips the hidden-key guard, the exotic-target dispatch, the
 * frozen and module-namespace refusals and `__proto__`'s relinking, so it is correct only for a
 * plain object the caller just made and a key the caller wrote as a literal.  Program-visible
 * assignment keeps going through `kiln_c99_put`. */
static int kiln_c99_own_data(kiln_value target, const char *key, size_t key_len,
                             kiln_value value) {
    kiln_property *slot;
    int status = kiln_c99_own_slot(target, key, key_len, &slot);
    if (status != KILN_C99_OK) {
        return status;
    }
    slot->value = value;
    return KILN_C99_OK;
}

/* The canonical pointer for one of KILN_C99_HOT_KEYS in the active heap, interned on first use.
 *
 * Lazy rather than filled at heap init, so nothing depends on the order in which a realm brings
 * its heap up; a NULL slot simply means "not yet", and the first request through pays for it. */
static const char *kiln_c99_hot_key(int id) {
    kiln_gc_heap *heap = kiln_c99_gc_heap();
    /* A `@@` name is a reserved slot reached through a Symbol, not a string key, and it is
     * resolved by a path this table knows nothing about. Interning one as a string and handing the
     * canonical pointer to `put` writes a property nothing later finds: measured as 5,793 wrong
     * answers out of 15k when `@@url:query` was briefly listed here. Refusing returns NULL, which
     * every caller already treats as "not interned, do it the ordinary way".
     *
     * The check is here rather than in a comment on the table because the table is the tempting
     * place to add one. */
    if (kiln_c99_hot_key_len[id] >= 2 && kiln_c99_hot_key_text[id][0] == '@' &&
        kiln_c99_hot_key_text[id][1] == '@') {
        return NULL;
    }
    if (heap == NULL) return NULL;
    if (heap->hot_keys[id] == NULL) {
        heap->hot_keys[id] =
            kiln_c99_intern(kiln_c99_hot_key_text[id], kiln_c99_hot_key_len[id]);
    }
    return heap->hot_keys[id];
}

/* `kiln_c99_put` for one of those keys, so a fresh object's members skip the hash too. */
static int kiln_c99_put_hot_key(kiln_host *host, kiln_value target, int id, kiln_value value,
                                int depth, kiln_value *thrown) {
    return kiln_c99_put_mode_ikey(host, target, kiln_c99_hot_key_text[id], kiln_c99_hot_key_len[id],
                                  kiln_c99_hot_key(id), value, 0, depth, thrown);
}

/* `kiln_c99_own_data` for a key the HTTP path already knows the canonical pointer for. */
static int kiln_c99_own_data_hot_key(kiln_value target, int id, kiln_value value) {
    kiln_property *slot;
    int status = kiln_c99_own_slot_ikey(target, kiln_c99_hot_key_text[id],
                                        kiln_c99_hot_key_len[id], kiln_c99_hot_key(id), &slot);
    if (status != KILN_C99_OK) {
        return status;
    }
    slot->value = value;
    return KILN_C99_OK;
}

static kiln_generator *kiln_c99_generator_of(kiln_value value) {
    return (kiln_generator *)(uintptr_t)value.bits;
}

/* The frame a generator will run in, filled with its arguments and nothing else.
 *
 * Both arrays are sized from the body's own counts, so a suspension has somewhere to put every
 * local and every operand the verifier proved it can hold. */
static int kiln_c99_new_generator(kiln_value *out, kiln_value function, kiln_value receiver,
                                  const kiln_value *args, size_t argc) {
    kiln_closure *closure = kiln_c99_closure_of(function);
    kiln_generator *frame = (kiln_generator *)kiln_c99_gc_alloc(KILN_GC_GENERATOR, sizeof *frame);
    size_t locals = closure->local_count == 0 ? 1 : closure->local_count;
    size_t depth = closure->stack_size == 0 ? 1 : closure->stack_size;
    size_t index;
    if (frame == NULL) {
        return KILN_C99_OUT_OF_MEMORY;
    }
    frame->locals = (kiln_value *)malloc(locals * sizeof *frame->locals);
    frame->stack = (kiln_value *)malloc(depth * sizeof *frame->stack);
    /* One handler slot per instruction is the bound the emitter uses for an automatic frame, and
     * it is the same bound here: a body cannot register more handlers than it has instructions. */
    frame->handlers = (size_t *)malloc((closure->handler_depth == 0 ? 1 : closure->handler_depth) *
                                       sizeof *frame->handlers);
    frame->handler_stack =
        (size_t *)malloc((closure->handler_depth == 0 ? 1 : closure->handler_depth) *
                         sizeof *frame->handler_stack);
    if (frame->locals == NULL || frame->stack == NULL || frame->handlers == NULL ||
        frame->handler_stack == NULL) {
        /* The four buffers are the frame's, so the KILN_GC_GENERATOR finaliser releases the ones
         * that did allocate; releasing the frame is the whole of the cleanup. */
        kiln_c99_gc_release(frame);
        return KILN_C99_OUT_OF_MEMORY;
    }
    frame->handler_top = 0;
    for (index = 0; index < locals; ++index) {
        frame->locals[index] = kiln_c99_undefined();
    }
    for (index = 0; index < argc && index < locals; ++index) {
        frame->locals[index] = args[index];
    }
    frame->function = function;
    frame->receiver = receiver;
    frame->sp = 0;
    frame->resume = 0;
    frame->sent = kiln_c99_undefined();
    frame->awaiting = 0;
    frame->async_generator = 0;
    frame->raising = 0;
    frame->binding = 0;
    frame->promise = kiln_c99_undefined();
    frame->is_async = 0;
    frame->started = 0;
    frame->done = 0;
    frame->running = 0;
    *out = kiln_c99_value(KILN_VALUE_GENERATOR, (uint64_t)(uintptr_t)frame, NULL, 0);
    return KILN_C99_OK;
}

/* The instant a set of UTC parts names, with every part allowed out of range.
 *
 * Out-of-range parts roll over rather than clamping: `Date.UTC(2026, 12)` is January of 2027.
 * That falls out of normalising the month into a year offset and then counting in whole units,
 * and it is what the specification says -- clamping would quietly produce a different date than
 * the program asked for. */
static double kiln_c99_instant_from_parts(const double parts[7]) {
    double months;
    double year;
    double month;
    double days;
    size_t index;
    for (index = 0; index < 7; ++index) {
        if (!isfinite(parts[index])) {
            return 0.0 / 0.0;
        }
    }
    months = floor(parts[1]);
    year = trunc(parts[0]) + floor(months / 12.0);
    month = months - floor(months / 12.0) * 12.0;
    /* Beyond what a civil date can name; the calendar arithmetic would overflow. */
    if (fabs(year) > 400000.0) {
        return 0.0 / 0.0;
    }
    days = (double)kiln_c99_days_from_civil((int64_t)year, (int64_t)month + 1, 1) +
           (parts[2] - 1.0);
    return days * 86400000.0 + parts[3] * 3600000.0 + parts[4] * 60000.0 + parts[5] * 1000.0 +
           parts[6];
}

/* Whether an array method changes how many elements there are.
 *
 * `sort` and `reverse` are not on the list: they write elements a frozen array already refuses,
 * and the length they leave is the one they were given. */
static int kiln_c99_changes_length(uint64_t code) {
    return code == KILN_C99_METHOD_PUSH || code == KILN_C99_METHOD_POP ||
           code == KILN_C99_METHOD_ARRAY_SHIFT || code == KILN_C99_METHOD_ARRAY_UNSHIFT ||
           code == KILN_C99_METHOD_ARRAY_SPLICE;
}

/* Whether an array method only reads its receiver.
 *
 * The array methods are generic -- `Array.prototype.slice.call(arguments)` is how `arguments`
 * became a list for twenty years -- but generalising one that *writes* would need the writes to
 * land back on the array-like, and a snapshot cannot carry them. So the readers generalise and
 * the mutators keep saying they only know how to work on a real array. */
static int kiln_c99_reads_only(uint64_t code) {
    return !kiln_c99_changes_length(code) && code != KILN_C99_METHOD_ARRAY_SORT &&
           code != KILN_C99_METHOD_ARRAY_REVERSE && code != KILN_C99_METHOD_ARRAY_FILL &&
           code != KILN_C99_METHOD_ARRAY_COPY_WITHIN;
}

/* The method object obtained from `Array.prototype` retains Array's generic species: unlike
 * `view.map`, `Array.prototype.map.call(view, fn)` returns an ordinary Array. */
static uint64_t kiln_c99_array_prototype_method(const char *key, size_t key_len) {
    uint64_t code = kiln_c99_array_method(key, key_len);
    if (code == KILN_C99_METHOD_MAP) {
        return KILN_C99_METHOD_ARRAY_GENERIC_MAP;
    }
    if (code == KILN_C99_METHOD_FILTER) {
        return KILN_C99_METHOD_ARRAY_GENERIC_FILTER;
    }
    return code;
}

static kiln_value kiln_c99_method(uint64_t code) {
    return kiln_c99_value(KILN_VALUE_METHOD, code, NULL, 0);
}

static int kiln_c99_commonjs_require_value(kiln_value *out, kiln_commonjs_require_fn call,
                                           void *context) {
    kiln_commonjs_require *require;
    if (out == NULL || call == NULL) return KILN_C99_INVALID_ARGUMENT;
    require = (kiln_commonjs_require *)kiln_c99_gc_alloc(KILN_GC_COMMONJS_REQUIRE,
                                                         sizeof *require);
    if (require == NULL) return KILN_C99_OUT_OF_MEMORY;
    require->call = call;
    require->context = context;
    *out = kiln_c99_value(KILN_VALUE_COMMONJS_REQUIRE, (uint64_t)(uintptr_t)require, NULL, 0);
    return KILN_C99_OK;
}

/* The name a console shows, which is the JavaScript name rather than the implementation's --
 * `[Function: push]`, the same text the interpreter produces. */
static const char *kiln_c99_method_name(uint64_t code) {
    if (kiln_c99_method_in_group(code, KILN_C99_GROUP_TEMPORAL_OP)) {
        return kiln_c99_temporal_op_name(code);
    }
    if (kiln_c99_method_in_group(code, KILN_C99_GROUP_TEMPORAL_NOW_READ)) {
        return kiln_c99_temporal_now_name(code);
    }
    switch (code) {
    /* What `require("node:assert")` and `require("node:events")` hand back. Both are the module's
     * callable default, and node names them for the module rather than for the implementation:
     * `assert` and `EventEmitter`. One code serves `assert`, `assert.ok` and `assert.strict` here,
     * so the name is the one node gives the value `require` returns -- the read this was measured
     * on -- and `assert.ok.name` reads "assert" rather than node's "ok" until the three become
     * three codes. */
    case KILN_C99_METHOD_NODE_ASSERT_OK: return "assert";
    case KILN_C99_METHOD_NODE_EVENT_EMITTER: return "EventEmitter";
    case KILN_C99_METHOD_NODE_HTTP_SERVER: return "Server";
    case KILN_C99_METHOD_NODE_REQUEST_HEADERS_GET: return "headers";
    case KILN_C99_METHOD_NODE_REQUEST_HEADERS_SET: return "headers";
    case KILN_C99_METHOD_NODE_HTTP_CREATE_SERVER: return "createServer";
    case KILN_C99_METHOD_NODE_QUERYSTRING_PARSE: return "parse";
    case KILN_C99_METHOD_PROCESS_NEXT_TICK: return "nextTick";
    case KILN_C99_METHOD_TEMPORAL_INSTANT_FROM_EPOCH_MILLISECONDS:
        return "fromEpochMilliseconds";
    case KILN_C99_METHOD_TEMPORAL_INSTANT_FROM_EPOCH_NANOSECONDS:
        return "fromEpochNanoseconds";
    /* `DataView.prototype`. Spelled out rather than derived from a width and a signedness,
     * because the names are not a pattern the language follows all the way -- `getUint8` and
     * `getBigUint64` capitalise the "U" differently, and a rule that got that wrong would name a
     * method nobody can call. */
    case KILN_C99_METHOD_DATA_VIEW_GET_INT8: return "getInt8";
    case KILN_C99_METHOD_DATA_VIEW_GET_UINT8: return "getUint8";
    case KILN_C99_METHOD_DATA_VIEW_GET_INT16: return "getInt16";
    case KILN_C99_METHOD_DATA_VIEW_GET_UINT16: return "getUint16";
    case KILN_C99_METHOD_DATA_VIEW_GET_INT32: return "getInt32";
    case KILN_C99_METHOD_DATA_VIEW_GET_UINT32: return "getUint32";
    case KILN_C99_METHOD_DATA_VIEW_GET_FLOAT32: return "getFloat32";
    case KILN_C99_METHOD_DATA_VIEW_GET_FLOAT64: return "getFloat64";
    case KILN_C99_METHOD_DATA_VIEW_GET_BIG_INT64: return "getBigInt64";
    case KILN_C99_METHOD_DATA_VIEW_GET_BIG_UINT64: return "getBigUint64";
    case KILN_C99_METHOD_DATA_VIEW_SET_INT8: return "setInt8";
    case KILN_C99_METHOD_DATA_VIEW_SET_UINT8: return "setUint8";
    case KILN_C99_METHOD_DATA_VIEW_SET_INT16: return "setInt16";
    case KILN_C99_METHOD_DATA_VIEW_SET_UINT16: return "setUint16";
    case KILN_C99_METHOD_DATA_VIEW_SET_INT32: return "setInt32";
    case KILN_C99_METHOD_DATA_VIEW_SET_UINT32: return "setUint32";
    case KILN_C99_METHOD_DATA_VIEW_SET_FLOAT32: return "setFloat32";
    case KILN_C99_METHOD_DATA_VIEW_SET_FLOAT64: return "setFloat64";
    case KILN_C99_METHOD_DATA_VIEW_SET_BIG_INT64: return "setBigInt64";
    case KILN_C99_METHOD_DATA_VIEW_SET_BIG_UINT64: return "setBigUint64";
    case KILN_C99_METHOD_PUSH: return "push";
    case KILN_C99_METHOD_POP: return "pop";
    case KILN_C99_METHOD_JOIN: return "join";
    case KILN_C99_METHOD_INDEX_OF: return "indexOf";
    case KILN_C99_METHOD_INCLUDES: return "includes";
    case KILN_C99_METHOD_SLICE: return "slice";
    case KILN_C99_METHOD_CONCAT: return "concat";
    case KILN_C99_METHOD_ARRAY_AT: return "at";
    case KILN_C99_METHOD_ARRAY_FIND: return "find";
    case KILN_C99_METHOD_ARRAY_FIND_INDEX: return "findIndex";
    case KILN_C99_METHOD_ARRAY_SOME: return "some";
    case KILN_C99_METHOD_ARRAY_EVERY: return "every";
    case KILN_C99_METHOD_ARRAY_SORT: return "sort";
    case KILN_C99_METHOD_ARRAY_FLAT: return "flat";
    case KILN_C99_METHOD_ARRAY_ENTRIES: return "entries";
    case KILN_C99_METHOD_ARRAY_KEYS: return "keys";
    case KILN_C99_METHOD_ARRAY_REVERSE: return "reverse";
    case KILN_C99_METHOD_ARRAY_SHIFT: return "shift";
    case KILN_C99_METHOD_ARRAY_UNSHIFT: return "unshift";
    case KILN_C99_METHOD_ARRAY_FILL: return "fill";
    case KILN_C99_METHOD_ARRAY_LAST_INDEX_OF: return "lastIndexOf";
    case KILN_C99_METHOD_ARRAY_SPLICE: return "splice";
    case KILN_C99_METHOD_ARRAY_COPY_WITHIN: return "copyWithin";
    case KILN_C99_METHOD_ARRAY_TO_STRING: return "toString";
    case KILN_C99_METHOD_ARRAY_FLAT_MAP: return "flatMap";
    case KILN_C99_METHOD_ARRAY_FIND_LAST: return "findLast";
    case KILN_C99_METHOD_ARRAY_FIND_LAST_INDEX: return "findLastIndex";
    case KILN_C99_METHOD_ARRAY_REDUCE_RIGHT: return "reduceRight";
    case KILN_C99_METHOD_ARRAY_TO_SORTED: return "toSorted";
    case KILN_C99_METHOD_ARRAY_TO_REVERSED: return "toReversed";
    case KILN_C99_METHOD_ARRAY_TO_SPLICED: return "toSpliced";
    case KILN_C99_METHOD_ARRAY_WITH: return "with";
    case KILN_C99_METHOD_MAP: return "map";
    case KILN_C99_METHOD_FILTER: return "filter";
    case KILN_C99_METHOD_ARRAY_GENERIC_MAP: return "map";
    case KILN_C99_METHOD_ARRAY_GENERIC_FILTER: return "filter";
    case KILN_C99_METHOD_FOR_EACH: return "forEach";
    case KILN_C99_METHOD_REDUCE: return "reduce";
    case KILN_C99_METHOD_STRING_MATCH: return "match";
    case KILN_C99_METHOD_STRING_MATCH_ALL: return "matchAll";
    case KILN_C99_METHOD_STRING_SEARCH: return "search";
    case KILN_C99_METHOD_ERROR_TO_STRING: return "toString";
    case KILN_C99_METHOD_STRING_CODE_POINT_AT: return "codePointAt";
    case KILN_C99_METHOD_STRING_SUBSTR: return "substr";
    case KILN_C99_METHOD_STRING_LOCALE_COMPARE: return "localeCompare";
    case KILN_C99_METHOD_STRING_NORMALIZE: return "normalize";
    case KILN_C99_METHOD_STRING_TO_STRING: return "toString";
    case KILN_C99_METHOD_STRING_VALUE_OF: return "valueOf";
    case KILN_C99_METHOD_STRING_SLICE: return "slice";
    case KILN_C99_METHOD_SUBSTRING: return "substring";
    case KILN_C99_METHOD_STRING_INDEX_OF: return "indexOf";
    case KILN_C99_METHOD_LAST_INDEX_OF: return "lastIndexOf";
    case KILN_C99_METHOD_STRING_INCLUDES: return "includes";
    case KILN_C99_METHOD_STARTS_WITH: return "startsWith";
    case KILN_C99_METHOD_ENDS_WITH: return "endsWith";
    case KILN_C99_METHOD_CHAR_AT: return "charAt";
    case KILN_C99_METHOD_CHAR_CODE_AT: return "charCodeAt";
    case KILN_C99_METHOD_STRING_AT: return "at";
    case KILN_C99_METHOD_TRIM: return "trim";
    case KILN_C99_METHOD_TRIM_START: return "trimStart";
    case KILN_C99_METHOD_TRIM_END: return "trimEnd";
    case KILN_C99_METHOD_SPLIT: return "split";
    case KILN_C99_METHOD_REPEAT: return "repeat";
    case KILN_C99_METHOD_PAD_START: return "padStart";
    case KILN_C99_METHOD_PAD_END: return "padEnd";
    case KILN_C99_METHOD_TO_UPPER_CASE: return "toUpperCase";
    case KILN_C99_METHOD_TO_LOWER_CASE: return "toLowerCase";
    case KILN_C99_METHOD_REPLACE: return "replace";
    case KILN_C99_METHOD_REPLACE_ALL: return "replaceAll";
    case KILN_C99_METHOD_STRING_CONCAT: return "concat";
    case KILN_C99_METHOD_TEMPORAL_TO_STRING: return "toString";
    case KILN_C99_METHOD_TEMPORAL_TO_JSON: return "toJSON";
    case KILN_C99_METHOD_TEMPORAL_VALUE_OF: return "valueOf";
    case KILN_C99_METHOD_TEMPORAL_NOW_TIME_ZONE_ID: return "timeZoneId";
    case KILN_C99_METHOD_MATH_FLOOR: return "floor";
    case KILN_C99_METHOD_MATH_CEIL: return "ceil";
    case KILN_C99_METHOD_MATH_ROUND: return "round";
    case KILN_C99_METHOD_MATH_TRUNC: return "trunc";
    case KILN_C99_METHOD_MATH_ABS: return "abs";
    case KILN_C99_METHOD_MATH_SIGN: return "sign";
    case KILN_C99_METHOD_MATH_SQRT: return "sqrt";
    case KILN_C99_METHOD_MATH_CBRT: return "cbrt";
    case KILN_C99_METHOD_MATH_POW: return "pow";
    case KILN_C99_METHOD_MATH_MIN: return "min";
    case KILN_C99_METHOD_MATH_MAX: return "max";
    case KILN_C99_METHOD_MATH_LOG: return "log";
    case KILN_C99_METHOD_MATH_LOG2: return "log2";
    case KILN_C99_METHOD_MATH_LOG10: return "log10";
    case KILN_C99_METHOD_MATH_EXP: return "exp";
    case KILN_C99_METHOD_MATH_HYPOT: return "hypot";
    case KILN_C99_METHOD_MATH_ATAN2: return "atan2";
    case KILN_C99_METHOD_MATH_SIN: return "sin";
    case KILN_C99_METHOD_MATH_COS: return "cos";
    case KILN_C99_METHOD_MATH_TAN: return "tan";
    case KILN_C99_METHOD_MATH_ASIN: return "asin";
    case KILN_C99_METHOD_MATH_ACOS: return "acos";
    case KILN_C99_METHOD_MATH_ATAN: return "atan";
    case KILN_C99_METHOD_QUEUE_MICROTASK: return "queueMicrotask";
    case KILN_C99_METHOD_SET_TIMEOUT: return "setTimeout";
    case KILN_C99_METHOD_SET_INTERVAL: return "setInterval";
    case KILN_C99_METHOD_CLEAR_TIMEOUT: return "clearTimeout";
    case KILN_C99_METHOD_CLEAR_INTERVAL: return "clearInterval";
    /* The immediate pair arrived after the other four timers and nothing carried the name over,
     * so `setImmediate.name` read `""` against the interpreter's "setImmediate". Neither is
     * reached through `kiln_c99_lookup_method`, so the guard that covers a lookup range could not
     * see them; `every_global_reports_the_name_it_is_published_under` reads the global table
     * itself and does. */
    case KILN_C99_METHOD_SET_IMMEDIATE: return "setImmediate";
    case KILN_C99_METHOD_CLEAR_IMMEDIATE: return "clearImmediate";
    case KILN_C99_METHOD_ESCAPE: return "escape";
    case KILN_C99_METHOD_UNESCAPE: return "unescape";
    case KILN_C99_METHOD_ENCODE_URI: return "encodeURI";
    case KILN_C99_METHOD_ENCODE_URI_COMPONENT: return "encodeURIComponent";
    case KILN_C99_METHOD_DECODE_URI: return "decodeURI";
    case KILN_C99_METHOD_DECODE_URI_COMPONENT: return "decodeURIComponent";
    case KILN_C99_METHOD_NUMBER_TO_STRING: return "toString";
    case KILN_C99_METHOD_NUMBER_TO_FIXED: return "toFixed";
    case KILN_C99_METHOD_NUMBER_TO_EXPONENTIAL: return "toExponential";
    case KILN_C99_METHOD_NUMBER_TO_PRECISION: return "toPrecision";
    case KILN_C99_METHOD_NUMBER_VALUE_OF: return "valueOf";
    case KILN_C99_METHOD_PARSE_INT: return "parseInt";
    case KILN_C99_METHOD_PARSE_FLOAT: return "parseFloat";
    case KILN_C99_METHOD_GLOBAL_IS_NAN: return "isNaN";
    case KILN_C99_METHOD_GLOBAL_IS_FINITE: return "isFinite";
    case KILN_C99_METHOD_IS_INTEGER: return "isInteger";
    case KILN_C99_METHOD_IS_SAFE_INTEGER: return "isSafeInteger";
    case KILN_C99_METHOD_IS_NAN: return "isNaN";
    case KILN_C99_METHOD_IS_FINITE: return "isFinite";
    case KILN_C99_METHOD_OBJECT_KEYS: return "keys";
    case KILN_C99_METHOD_OBJECT_VALUES: return "values";
    case KILN_C99_METHOD_OBJECT_ENTRIES: return "entries";
    case KILN_C99_METHOD_OBJECT_ASSIGN: return "assign";
    case KILN_C99_METHOD_HAS_OWN: return "hasOwn";
    case KILN_C99_METHOD_IS_ARRAY: return "isArray";
    case KILN_C99_METHOD_ARRAY_OF: return "of";
    case KILN_C99_METHOD_OBJECT_CREATE: return "create";
    case KILN_C99_METHOD_JSON_STRINGIFY: return "stringify";
    case KILN_C99_METHOD_JSON_PARSE: return "parse";
    case KILN_C99_METHOD_GENERATOR_NEXT: return "next";
    case KILN_C99_METHOD_GENERATOR_RETURN: return "return";
    /* `[Symbol.iterator]()` on a generator hands back the generator, which is what lets one be
     * spread, destructured or walked by `for...of` -- and what makes walking it twice walk a
     * generator that is already finished. */
    case KILN_C99_METHOD_GENERATOR_SELF: return "@@iterator";
    case KILN_C99_METHOD_GET_TIME: return "getTime";
    case KILN_C99_METHOD_GET_FULL_YEAR: return "getFullYear";
    case KILN_C99_METHOD_GET_MONTH: return "getMonth";
    case KILN_C99_METHOD_GET_DATE: return "getDate";
    case KILN_C99_METHOD_GET_DAY: return "getDay";
    case KILN_C99_METHOD_GET_HOURS: return "getHours";
    case KILN_C99_METHOD_GET_MINUTES: return "getMinutes";
    case KILN_C99_METHOD_GET_SECONDS: return "getSeconds";
    case KILN_C99_METHOD_GET_MILLISECONDS: return "getMilliseconds";
    case KILN_C99_METHOD_GET_TIMEZONE_OFFSET: return "getTimezoneOffset";
    case KILN_C99_METHOD_TO_ISO_STRING: return "toISOString";
    case KILN_C99_METHOD_TO_JSON: return "toJSON";
    case KILN_C99_METHOD_DATE_TO_STRING: return "toString";
    case KILN_C99_METHOD_DATE_NOW: return "now";
    case KILN_C99_METHOD_DATE_UTC: return "UTC";
    case KILN_C99_METHOD_DATE_PARSE: return "parse";
    case KILN_C99_METHOD_HAS_OWN_PROPERTY: return "hasOwnProperty";
    case KILN_C99_METHOD_OBJECT_TO_STRING: return "toString";
    case KILN_C99_METHOD_OBJECT_TO_LOCALE_STRING: return "toLocaleString";
    case KILN_C99_METHOD_OBJECT_VALUE_OF: return "valueOf";
    case KILN_C99_METHOD_COLLECTION_GET: return "get";
    case KILN_C99_METHOD_COLLECTION_SET: return "set";
    case KILN_C99_METHOD_COLLECTION_ADD: return "add";
    case KILN_C99_METHOD_COLLECTION_HAS: return "has";
    case KILN_C99_METHOD_COLLECTION_DELETE: return "delete";
    case KILN_C99_METHOD_COLLECTION_CLEAR: return "clear";
    case KILN_C99_METHOD_COLLECTION_FOR_EACH: return "forEach";
    case KILN_C99_METHOD_COLLECTION_KEYS: return "keys";
    case KILN_C99_METHOD_COLLECTION_VALUES: return "values";
    case KILN_C99_METHOD_COLLECTION_ENTRIES: return "entries";
    case KILN_C99_METHOD_COLLECTION_SELF: return "@@iterator";
    case KILN_C99_METHOD_ITERATOR_NEXT: return "next";
    case KILN_C99_METHOD_COLLECTION_ITERATOR_NEXT: return "next";
    case KILN_C99_METHOD_PRIMITIVE_TO_STRING: return "toString";
    case KILN_C99_METHOD_ITERATOR_SELF: return "@@iterator";
    /* The iterator helpers. These names are not decoration: `kiln_c99_lookup_method` walks the
     * 470..481 range comparing them, so this switch *is* the `Iterator.prototype` member table. */
    case KILN_C99_METHOD_ITERATOR_MAP: return "map";
    case KILN_C99_METHOD_ITERATOR_FILTER: return "filter";
    case KILN_C99_METHOD_ITERATOR_TAKE: return "take";
    case KILN_C99_METHOD_ITERATOR_DROP: return "drop";
    case KILN_C99_METHOD_ITERATOR_FLAT_MAP: return "flatMap";
    case KILN_C99_METHOD_ITERATOR_REDUCE: return "reduce";
    case KILN_C99_METHOD_ITERATOR_TO_ARRAY: return "toArray";
    case KILN_C99_METHOD_ITERATOR_FOR_EACH: return "forEach";
    case KILN_C99_METHOD_ITERATOR_SOME: return "some";
    case KILN_C99_METHOD_ITERATOR_EVERY: return "every";
    case KILN_C99_METHOD_ITERATOR_FIND: return "find";
    case KILN_C99_METHOD_ITERATOR_INCLUDES: return "includes";
    /* A helper's own two methods, and the static that wraps a foreign iterator in one. Past the
     * name range above, so none of them can be found by reading a name off the table. */
    case KILN_C99_METHOD_ITERATOR_HELPER_NEXT: return "next";
    case KILN_C99_METHOD_ITERATOR_HELPER_RETURN: return "return";
    case KILN_C99_METHOD_ITERATOR_FROM: return "from";
    case KILN_C99_METHOD_ARRAY_VALUES: return "values";
    /* `%ThrowTypeError%` is the realm's one anonymous function, so the name is empty rather
     * than descriptive: a program reading `arguments.callee`'s getter must not learn one. */
    case KILN_C99_METHOD_THROW_TYPE_ERROR: return "";
    case KILN_C99_METHOD_THEN: return "then";
    case KILN_C99_METHOD_CATCH: return "catch";
    case KILN_C99_METHOD_FINALLY: return "finally";
    case KILN_C99_METHOD_PROMISE_RESOLVE: return "resolve";
    case KILN_C99_METHOD_PROMISE_REJECT: return "reject";
    case KILN_C99_METHOD_PROMISE_ALL: return "all";
    case KILN_C99_METHOD_PROMISE_RACE: return "race";
    /* Not reachable by name: these are the two handlers an `await` attaches to what it is
     * waiting on, and nothing looks them up. */
    case KILN_C99_METHOD_ASYNC_RESUME: return "@@resume";
    case KILN_C99_METHOD_ASYNC_THROW: return "@@throw";
    case KILN_C99_METHOD_PROMISE_ALL_SETTLED: return "allSettled";
    case KILN_C99_METHOD_PROMISE_ANY: return "any";
    case KILN_C99_METHOD_ALL_STEP: return "@@allStep";
    case KILN_C99_METHOD_REJECT_STEP: return "@@rejectStep";
    case KILN_C99_METHOD_FINALLY_PASS: return "@@finallyPass";
    case KILN_C99_METHOD_FINALLY_RAISE: return "@@finallyRaise";
    case KILN_C99_METHOD_FINALLY_RESTORE: return "@@finallyRestore";
    case KILN_C99_METHOD_FINALLY_RERAISE: return "@@finallyReraise";
    case KILN_C99_METHOD_ADOPT_STEP: return "@@adopt";
    case KILN_C99_METHOD_REGEXP_TEST: return "test";
    case KILN_C99_METHOD_REGEXP_EXEC: return "exec";
    case KILN_C99_METHOD_REGEXP_TO_STRING: return "toString";
    case KILN_C99_METHOD_TYPED_SUBARRAY: return "subarray";
    case KILN_C99_METHOD_TYPED_SET: return "set";
    case KILN_C99_METHOD_TYPED_FILL: return "fill";
    case KILN_C99_METHOD_TYPED_OF: return "of";
    case KILN_C99_METHOD_TYPED_FROM: return "from";
    case KILN_C99_METHOD_TYPED_TO_STRING_TAG: return "get [Symbol.toStringTag]";
    case KILN_C99_METHOD_MATH_RANDOM: return "random";
    case KILN_C99_METHOD_MATH_SINH: return "sinh";
    case KILN_C99_METHOD_MATH_COSH: return "cosh";
    case KILN_C99_METHOD_MATH_TANH: return "tanh";
    case KILN_C99_METHOD_MATH_ASINH: return "asinh";
    case KILN_C99_METHOD_MATH_ACOSH: return "acosh";
    case KILN_C99_METHOD_MATH_ATANH: return "atanh";
    case KILN_C99_METHOD_MATH_LOG1P: return "log1p";
    case KILN_C99_METHOD_MATH_EXPM1: return "expm1";
    case KILN_C99_METHOD_MATH_FROUND: return "fround";
    case KILN_C99_METHOD_MATH_CLZ32: return "clz32";
    case KILN_C99_METHOD_MATH_IMUL: return "imul";
    case KILN_C99_METHOD_OBJECT_GET_PROTOTYPE_OF: return "getPrototypeOf";
    case KILN_C99_METHOD_OBJECT_SET_PROTOTYPE_OF: return "setPrototypeOf";
    case KILN_C99_METHOD_OBJECT_GET_OWN_PROPERTY_NAMES: return "getOwnPropertyNames";
    case KILN_C99_METHOD_OBJECT_GET_OWN_PROPERTY_SYMBOLS: return "getOwnPropertySymbols";
    case KILN_C99_METHOD_OBJECT_DEFINE_PROPERTY: return "defineProperty";
    case KILN_C99_METHOD_OBJECT_DEFINE_PROPERTIES: return "defineProperties";
    case KILN_C99_METHOD_OBJECT_GET_OWN_PROPERTY_DESCRIPTOR: return "getOwnPropertyDescriptor";
    case KILN_C99_METHOD_OBJECT_GET_OWN_PROPERTY_DESCRIPTORS: return "getOwnPropertyDescriptors";
    case KILN_C99_METHOD_OBJECT_IS: return "is";
    case KILN_C99_METHOD_ERROR_CAPTURE_STACK_TRACE: return "captureStackTrace";
    case KILN_C99_METHOD_REFLECT_OWN_KEYS: return "ownKeys";
    case KILN_C99_METHOD_REFLECT_GET: return "get";
    case KILN_C99_METHOD_REFLECT_SET: return "set";
    case KILN_C99_METHOD_REFLECT_HAS: return "has";
    case KILN_C99_METHOD_REFLECT_DELETE_PROPERTY: return "deleteProperty";
    case KILN_C99_METHOD_REFLECT_APPLY: return "apply";
    case KILN_C99_METHOD_REFLECT_CONSTRUCT: return "construct";
    case KILN_C99_METHOD_PROXY_REVOCABLE: return "revocable";
    case KILN_C99_METHOD_PROXY_REVOKE: return "revoke";
    case KILN_C99_METHOD_OBJECT_FROM_ENTRIES: return "fromEntries";
    case KILN_C99_METHOD_ARRAY_FROM: return "from";
    case KILN_C99_METHOD_SET_TIME: return "setTime";
    case KILN_C99_METHOD_SET_FULL_YEAR: return "setUTCFullYear";
    case KILN_C99_METHOD_SET_MONTH: return "setUTCMonth";
    case KILN_C99_METHOD_SET_DATE: return "setUTCDate";
    case KILN_C99_METHOD_SET_HOURS: return "setUTCHours";
    case KILN_C99_METHOD_SET_MINUTES: return "setUTCMinutes";
    case KILN_C99_METHOD_SET_SECONDS: return "setUTCSeconds";
    case KILN_C99_METHOD_SET_MILLISECONDS: return "setUTCMilliseconds";
    case KILN_C99_METHOD_STRING_RAW: return "raw";
    case KILN_C99_METHOD_STRING_FROM_CHAR_CODE: return "fromCharCode";
    case KILN_C99_METHOD_IS_PROTOTYPE_OF: return "isPrototypeOf";
    case KILN_C99_METHOD_PROPERTY_IS_ENUMERABLE: return "propertyIsEnumerable";
    case KILN_C99_METHOD_FUNCTION_CALL: return "call";
    case KILN_C99_METHOD_FUNCTION_APPLY: return "apply";
    case KILN_C99_METHOD_FUNCTION_BIND: return "bind";
    case KILN_C99_METHOD_FUNCTION_TO_STRING: return "toString";
    case KILN_C99_METHOD_OBJECT_FREEZE: return "freeze";
    case KILN_C99_METHOD_OBJECT_IS_FROZEN: return "isFrozen";
    case KILN_C99_METHOD_OBJECT_PREVENT_EXTENSIONS: return "preventExtensions";
    case KILN_C99_METHOD_OBJECT_IS_EXTENSIBLE: return "isExtensible";
    default: return NULL;
    }
}

/* The state `Math.random` advances belongs to the active realm, while generated C retains the
 * exact SplitMix64 policy seed and the conversion from one word to an ECMAScript number.
 *
 * SplitMix64: a full-period generator with good statistical properties in five lines, which is
 * what a language builtin needs. Seeded from a constant rather than from the environment, so the
 * same program produces the same sequence in both engines and on every machine -- the same trade
 * the virtual clock makes, and the reason an artifact hash stays honest. See ADR 0026. */
#ifndef KILN_REALM_ENTROPY_SEED
#define KILN_REALM_ENTROPY_SEED UINT64_C(0x9E3779B97F4A7C15)
#endif
static int kiln_c99_entropy_word(kiln_host *host, uint64_t *out) {
#if KILN_REALM_HOST_ENTROPY
    kiln_value answer;
    if (host == NULL || host->call == NULL || out == NULL ||
        host->call(host->context, UINT16_C(3), UINT16_C(0), NULL, &answer) != 0 ||
        answer.tag != KILN_VALUE_NUMBER) {
        return KILN_C99_HOST_ERROR;
    }
    *out = answer.bits;
    return KILN_C99_OK;
#else
    (void)host;
    return kiln_realm_entropy_word(KILN_REALM_ENTROPY_SEED, out);
#endif
}

static int kiln_c99_random(kiln_host *host, double *out) {
    uint64_t word;
    int status = kiln_c99_entropy_word(host, &word);
    if (status != KILN_C99_OK || out == NULL) return status;
    /* 53 random bits, which is every bit the mantissa has. */
    *out = (double)(word >> 11) / 9007199254740992.0;
    return KILN_C99_OK;
}

/* One hex digit's value, or -1. Used by every unescaping path, none of which may accept the
 * `0x` prefixes or the locale-dependent classifications that `strtol` would. */
static int kiln_c99_hex_digit(char character) {
    if (character >= '0' && character <= '9') {
        return character - '0';
    }
    if (character >= 'a' && character <= 'f') {
        return character - 'a' + 10;
    }
    if (character >= 'A' && character <= 'F') {
        return character - 'A' + 10;
    }
    return -1;
}

static int kiln_c99_append_hex(kiln_text *text, unsigned value, int width) {
    static const char alphabet[] = "0123456789ABCDEF";
    char digits[4];
    int place;
    for (place = width - 1; place >= 0; --place) {
        digits[place] = alphabet[value & 0xFu];
        value >>= 4;
    }
    return kiln_c99_text_append(text, digits, (size_t)width);
}

/* Whether a byte span is well-formed UTF-8.
 *
 * `decodeURI` is the only place this is asked: percent-escapes name bytes, and a program can name
 * bytes that are not a string. Rust's `String::from_utf8` is what rejects them on the interpreter
 * side, and this is that check -- overlong forms, surrogates and out-of-range scalars included,
 * because accepting any of them would let the two engines hold different strings. */
static int kiln_c99_valid_utf8(const char *bytes, size_t len) {
    size_t at = 0;
    while (at < len) {
        unsigned char lead = (unsigned char)bytes[at];
        uint32_t codepoint;
        size_t extra;
        size_t index;
        if (lead < 0x80u) {
            at += 1;
            continue;
        }
        if (lead >= 0xC2u && lead <= 0xDFu) {
            extra = 1;
            codepoint = lead & 0x1Fu;
        } else if (lead >= 0xE0u && lead <= 0xEFu) {
            extra = 2;
            codepoint = lead & 0x0Fu;
        } else if (lead >= 0xF0u && lead <= 0xF4u) {
            extra = 3;
            codepoint = lead & 0x07u;
        } else {
            /* A continuation byte on its own, or an overlong two-byte lead. */
            return 0;
        }
        if (at + extra >= len) {
            /* The sequence runs off the end. */
            return 0;
        }
        for (index = 1; index <= extra; ++index) {
            unsigned char next = (unsigned char)bytes[at + index];
            if ((next & 0xC0u) != 0x80u) {
                return 0;
            }
            codepoint = (codepoint << 6) | (next & 0x3Fu);
        }
        if (codepoint > UINT32_C(0x10FFFF) ||
            (codepoint >= UINT32_C(0xD800) && codepoint <= UINT32_C(0xDFFF)) ||
            (extra == 2 && codepoint < UINT32_C(0x800)) ||
            (extra == 3 && codepoint < UINT32_C(0x10000))) {
            return 0;
        }
        at += extra + 1;
    }
    return 1;
}

/* The URL subset shared with the interpreter today: absolute HTTP-like input, or a path against
 * an absolute base; pathname and query lookup are the public state. Keeping that advertised
 * subset identical is better than a broader native parser that disagrees in corners. */
static int kiln_c99_url_construct(kiln_value *out, const kiln_value *args, size_t argc,
                                  kiln_value *thrown) {
    kiln_value input = kiln_c99_text(argc > 0 ? args[0] : kiln_c99_undefined());
    kiln_value base = kiln_c99_text(argc > 1 ? args[1] : kiln_c99_undefined());
    const char *start;
    const char *end;
    const char *query;
    const char *fragment;
    const char *scheme;
    kiln_value params;
    int status;
    if (input.tag != KILN_VALUE_STRING || base.tag != KILN_VALUE_STRING) {
        return KILN_C99_OUT_OF_MEMORY;
    }
    scheme = NULL;
    {
        size_t index;
        for (index = 0; index + 2 < input.len; ++index) {
            if (input.ptr[index] == ':' && input.ptr[index + 1] == '/' &&
                input.ptr[index + 2] == '/') {
                scheme = input.ptr + index;
                break;
            }
        }
    }
    if (scheme != NULL) {
        start = scheme + 3;
        end = input.ptr + input.len;
        while (start < end && *start != '/' && *start != '?' && *start != '#') ++start;
        if (start == end || *start != '/') start = "/";
    } else {
        int valid_base = 0;
        size_t index;
        for (index = 0; index + 2 < base.len; ++index) {
            if (base.ptr[index] == ':' && base.ptr[index + 1] == '/' &&
                base.ptr[index + 2] == '/') {
                valid_base = 1;
                break;
            }
        }
        if (!valid_base) {
            return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR, "Invalid base URL");
        }
        start = input.len == 0 ? "/" : input.ptr;
        end = input.ptr + input.len;
    }
    if (start == input.ptr || (start >= input.ptr && start <= input.ptr + input.len)) {
        end = input.ptr + input.len;
    } else {
        end = start + 1;
    }
    query = memchr(start, '?', (size_t)(end - start));
    fragment = memchr(start, '#', (size_t)(end - start));
    if (fragment != NULL && (query == NULL || fragment < query)) query = NULL;
    status = kiln_c99_new_object(out);
    if (status != KILN_C99_OK) return status;
    status = kiln_c99_put_hot_key(NULL, *out, KILN_C99_HOT_KEY_PATHNAME,
                          kiln_c99_copy_text(start, (size_t)((query != NULL ? query :
                              (fragment != NULL ? fragment : end)) - start)), 0, NULL);
    if (status != KILN_C99_OK) return status;
    status = kiln_c99_new_object(&params);
    if (status != KILN_C99_OK) return status;
    status = kiln_c99_put(NULL, params, "@@url:query", 11,
                          query == NULL ? kiln_c99_copy_text("", 0) :
                          kiln_c99_copy_text(query + 1, (size_t)((fragment != NULL ? fragment : end) - query - 1)),
                          0, NULL);
    if (status != KILN_C99_OK) return status;
    status = kiln_c99_put_hot_key(NULL, params, KILN_C99_HOT_KEY_GET,
                          kiln_c99_method(KILN_C99_METHOD_URL_SEARCH_PARAMS_GET), 0, NULL);
    if (status != KILN_C99_OK) return status;
    return kiln_c99_put_hot_key(NULL, *out, KILN_C99_HOT_KEY_SEARCH_PARAMS, params, 0, NULL);
}

static int kiln_c99_url_search_params_get(kiln_value *out, kiln_value receiver,
                                          const kiln_value *args, size_t argc,
                                          kiln_value *thrown) {
    kiln_value query;
    kiln_value name = kiln_c99_text(argc > 0 ? args[0] : kiln_c99_undefined());
    size_t at = 0;
    int status;
    if (receiver.tag != KILN_VALUE_OBJECT || name.tag != KILN_VALUE_STRING) {
        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                    "URLSearchParams.get called on an incompatible receiver");
    }
    status = kiln_c99_get(NULL, receiver, "@@url:query", 11, &query, 0, NULL);
    if (status != KILN_C99_OK || query.tag != KILN_VALUE_STRING)
        KILN_C99_REFUSE("URLSearchParams with no captured query string");
    while (at <= query.len) {
        size_t pair_end = at;
        size_t equal;
        while (pair_end < query.len && query.ptr[pair_end] != '&') ++pair_end;
        equal = at;
        while (equal < pair_end && query.ptr[equal] != '=') ++equal;
        if (equal - at == name.len && memcmp(query.ptr + at, name.ptr, name.len) == 0) {
            kiln_value encoded = kiln_c99_copy_text(
                equal < pair_end ? query.ptr + equal + 1 : "",
                equal < pair_end ? pair_end - equal - 1 : 0);
            const kiln_value decode_args[1] = {encoded};
            return kiln_c99_invoke_method(NULL, out, KILN_C99_METHOD_DECODE_URI_COMPONENT,
                                          kiln_c99_undefined(), decode_args, 1, 0, thrown);
        }
        if (pair_end == query.len) break;
        at = pair_end + 1;
    }
    *out = kiln_c99_null();
    return KILN_C99_OK;
}

/* Decode one application/x-www-form-urlencoded component. Unlike decodeURIComponent, malformed
 * percent escapes remain literal and `+` names a space. Query strings are deliberately forgiving:
 * one bad escape must not make the whole request unreadable. */
static int kiln_c99_form_decode(const char *bytes, size_t len, kiln_value *out) {
    kiln_text decoded = {NULL, 0, 0};
    size_t index = 0;
    int status;
    while (index < len) {
        char byte = bytes[index];
        if (byte == '+') {
            byte = ' ';
        } else if (byte == '%' && index + 2 < len) {
            int high = kiln_c99_hex_digit(bytes[index + 1]);
            int low = kiln_c99_hex_digit(bytes[index + 2]);
            if (high >= 0 && low >= 0) {
                byte = (char)((high << 4) | low);
                index += 2;
            }
        }
        status = kiln_c99_text_append(&decoded, &byte, 1);
        if (status != KILN_C99_OK) {
            free(decoded.bytes);
            return status;
        }
        index += 1;
    }
    if (kiln_c99_valid_utf8(decoded.bytes, decoded.len)) {
        *out = kiln_c99_text_value(&decoded);
        return KILN_C99_OK;
    }
    {
        kiln_value raw = kiln_c99_value(KILN_VALUE_STRING, 0, decoded.bytes, decoded.len);
        uint16_t *units = NULL;
        size_t count = 0;
        status = kiln_c99_to_units(raw, &units, &count);
        if (status == KILN_C99_OK) {
            *out = kiln_c99_from_units(units, count);
            if (out->tag != KILN_VALUE_STRING) status = KILN_C99_OUT_OF_MEMORY;
        }
        free(units);
        free(decoded.bytes);
        return status;
    }
}

/* The interpreter-first querystring surface: parse/decode form data into an own-property object,
 * preserving the first `=` and collecting repeated names in encounter order. Definition-style
 * insertion is intentional: a decoded `__proto__` is data, not a request to relink the result. */
static int kiln_c99_node_querystring_parse(kiln_value *out, const kiln_value *args, size_t argc) {
    kiln_value input = argc > 0 ? kiln_c99_text(args[0]) : kiln_c99_copy_text("", 0);
    size_t at = 0;
    int status;
    if (input.tag != KILN_VALUE_STRING) return KILN_C99_OUT_OF_MEMORY;
    status = kiln_c99_new_object(out);
    if (status != KILN_C99_OK) return status;
    while (at < input.len) {
        size_t pair_end = at;
        size_t equal;
        kiln_value name;
        kiln_value value;
        kiln_object *object;
        kiln_property *property = NULL;
        size_t index;
        while (pair_end < input.len && input.ptr[pair_end] != '&') pair_end += 1;
        if (pair_end == at) {
            at = pair_end + 1;
            continue;
        }
        equal = at;
        while (equal < pair_end && input.ptr[equal] != '=') equal += 1;
        status = kiln_c99_form_decode(input.ptr + at, equal - at, &name);
        if (status != KILN_C99_OK) return status;
        status = kiln_c99_form_decode(equal < pair_end ? input.ptr + equal + 1 : "",
                                      equal < pair_end ? pair_end - equal - 1 : 0, &value);
        if (status != KILN_C99_OK) {
            kiln_c99_gc_release((void *)name.ptr);
            return status;
        }
        object = kiln_c99_object_of(*out);
        for (index = 0; index < object->count; ++index) {
            if (kiln_c99_same_key(&object->properties[index], name.ptr, name.len)) {
                property = &object->properties[index];
                break;
            }
        }
        if (property == NULL) {
            status = kiln_c99_own_slot(*out, name.ptr, name.len, &property);
            if (status == KILN_C99_OK) property->value = value;
        } else {
            kiln_value previous = property->value;
            kiln_c99_gc_release((void *)name.ptr);
            if (previous.tag == KILN_VALUE_ARRAY) {
                status = kiln_c99_array_push(previous, value);
            } else {
                kiln_value values;
                status = kiln_c99_new_array(&values);
                if (status == KILN_C99_OK) status = kiln_c99_array_push(values, previous);
                if (status == KILN_C99_OK) status = kiln_c99_array_push(values, value);
                if (status == KILN_C99_OK) property->value = values;
            }
        }
        if (status != KILN_C99_OK) return status;
        at = pair_end + 1;
    }
    return KILN_C99_OK;
}

/* `Number.prototype.toFixed`, rounded on the value's *exact* decimal expansion.
 *
 * Not the printf `%.*f` conversion: it reads the process locale's decimal point, and an embedded
 * module runs in whatever locale its host set. The same objection is why a number's shortest
 * spelling comes from the number provider (ADR 0018); this is that rule applied to the other
 * direction.
 *
 * Not `floor(value * 10^f + 0.5)` either, which is the obvious arithmetic and is wrong:
 * `(1.45).toFixed(1)` is `"1.4"` because 1.45 is really 1.44999..., and multiplying by ten rounds
 * that up to exactly 14.5 before any rounding decision is made. The digits that decide have to be
 * the value's own.
 *
 * So the expansion is computed exactly. A binary64 is `m * 2^e` with `m` under 2^53, and for
 * negative `e` that is `m * 5^(-e)` with the point `-e` digits from the right -- an exact integer
 * a decimal digit array can hold, and repeated multiplication by five is the whole of the
 * arithmetic. 5^1074 is 751 digits, `m` adds sixteen, and values at or above 1e21 are handed back
 * to `ToString` by the specification, so the array is bounded and so is the loop. */
/* The exact decimal expansion of `value`'s magnitude, little-endian: `digits[0]` is the least
 * significant, and `*fractional` of them sit below the decimal point. Returns the digit count.
 *
 * Shared by the three renderings that have to round -- `toFixed`, `toExponential`,
 * `toPrecision` -- because all three must round on the value's *own* digits. That is the whole
 * argument of the comment above: scale by a power of ten first and the deciding digit is gone
 * before the language ever looks at it. Writing the expansion once means the three cannot come
 * to different conclusions about the same double.
 *
 * `value` must be finite; the callers answer the infinities and NaN from `ToString` instead. */
static size_t kiln_c99_exact_digits(double value, unsigned char *digits, size_t capacity,
                                    size_t *fractional) {
    size_t count = 0;
    size_t index;
    double mantissa;
    uint64_t scaled;
    int exponent;
    mantissa = frexp(fabs(value), &exponent);
    scaled = (uint64_t)ldexp(mantissa, 53);
    exponent -= 53;
    while (scaled != 0 && count < capacity) {
        digits[count++] = (unsigned char)(scaled % 10u);
        scaled /= 10u;
    }
    if (count == 0) {
        digits[count++] = 0;
    }
    if (exponent >= 0) {
        int step;
        for (step = 0; step < exponent; ++step) {
            unsigned carry = 0;
            for (index = 0; index < count; ++index) {
                unsigned product = (unsigned)digits[index] * 2u + carry;
                digits[index] = (unsigned char)(product % 10u);
                carry = product / 10u;
            }
            while (carry != 0 && count < capacity) {
                digits[count++] = (unsigned char)(carry % 10u);
                carry /= 10u;
            }
        }
        *fractional = 0;
    } else {
        int step;
        for (step = 0; step < -exponent; ++step) {
            unsigned carry = 0;
            for (index = 0; index < count; ++index) {
                unsigned product = (unsigned)digits[index] * 5u + carry;
                digits[index] = (unsigned char)(product % 10u);
                carry = product / 10u;
            }
            while (carry != 0 && count < capacity) {
                digits[count++] = (unsigned char)(carry % 10u);
                carry /= 10u;
            }
        }
        *fractional = (size_t)(-exponent);
    }
    /* Pad so there is a digit at every place the answer names, above the point and below it. */
    while (count < *fractional + 1 && count < capacity) {
        digits[count++] = 0;
    }
    return count;
}

static int kiln_c99_to_fixed(kiln_value *out, double value, int places) {
    /* Little-endian decimal digits: `digits[0]` is the least significant. */
    unsigned char digits[1200];
    size_t count;
    size_t fractional;
    size_t index;
    kiln_text text = {NULL, 0, 0};
    int negative = value < 0.0;
    int round_up;
    if (!isfinite(value) || fabs(value) >= 1e21) {
        *out = kiln_c99_text(kiln_c99_number(value));
        return out->tag == KILN_VALUE_STRING ? KILN_C99_OK : KILN_C99_OUT_OF_MEMORY;
    }
    count = kiln_c99_exact_digits(value, digits, sizeof digits, &fractional);
    if (fractional <= (size_t)places) {
        round_up = 0;
    } else {
        /* Round half *up*: "if there are two such n, pick the larger n". Only the deciding digit
         * is consulted, because at or above five the answer is up whatever follows. */
        round_up = digits[fractional - (size_t)places - 1] >= 5;
        for (index = 0; index < fractional - (size_t)places; ++index) {
            digits[index] = 0;
        }
        memmove(digits, digits + (fractional - (size_t)places),
                count - (fractional - (size_t)places));
        count -= fractional - (size_t)places;
        fractional = (size_t)places;
    }
    while (fractional < (size_t)places && count < sizeof digits) {
        /* The value terminated above the requested precision, so the rest is zeros. */
        memmove(digits + 1, digits, count);
        digits[0] = 0;
        count += 1;
        fractional += 1;
    }
    if (round_up) {
        for (index = 0; index < count; ++index) {
            if (digits[index] == 9) {
                digits[index] = 0;
                continue;
            }
            digits[index] += 1;
            break;
        }
        if (index == count && count < sizeof digits) {
            digits[count++] = 1;
        }
    }
    while (count > fractional + 1 && digits[count - 1] == 0) {
        /* A leading zero above the point is not part of the spelling, but the one before the
         * point is: `.5` is `0.5`. */
        count -= 1;
    }
    /* The sign survives rounding: `(-0.0004).toFixed(2)` is `"-0.00"`, which says the value was
     * negative rather than that it was nonzero. Negative zero is not negative here, so
     * `(-0).toFixed(2)` is `"0.00"`. */
    if (negative && kiln_c99_text_append_c(&text, "-") != KILN_C99_OK) {
        free(text.bytes);
        return KILN_C99_OUT_OF_MEMORY;
    }
    for (index = count; index > 0; --index) {
        char character = (char)('0' + digits[index - 1]);
        if (index == fractional && kiln_c99_text_append_c(&text, ".") != KILN_C99_OK) {
            free(text.bytes);
            return KILN_C99_OUT_OF_MEMORY;
        }
        if (kiln_c99_text_append(&text, &character, 1) != KILN_C99_OK) {
            free(text.bytes);
            return KILN_C99_OUT_OF_MEMORY;
        }
    }
    *out = kiln_c99_text_value(&text);
    return KILN_C99_OK;
}

/* The `n` and `e` the specification's exponential formats name, for a finite value.
 *
 * `count` significant digits of the *exact* expansion of the magnitude, rounded half up -- "if
 * there are two such sets, pick the larger n" -- written most significant first into `mantissa`
 * and NUL terminated, and the decimal exponent of the first of them in `*exponent`. `mantissa`
 * must have room for `count + 1` bytes. */
static void kiln_c99_significant_digits(double value, size_t count, char *mantissa,
                                        int *exponent) {
    unsigned char digits[1200];
    size_t total;
    size_t fractional;
    size_t point;
    size_t first;
    size_t index;
    size_t carry_at;
    total = kiln_c99_exact_digits(value, digits, sizeof digits, &fractional);
    /* `digits` is little-endian, so big-endian position `i` is `digits[total - 1 - i]`, and
     * `point` of those sit above the decimal point. */
    point = total - fractional;
    for (first = 0; first < total && digits[total - 1 - first] == 0; ++first) {
    }
    if (first == total) {
        /* Zero has no significant digit: the specification names its mantissa as zeros and its
         * exponent as zero. */
        for (index = 0; index < count; ++index) {
            mantissa[index] = '0';
        }
        mantissa[count] = '\0';
        *exponent = 0;
        return;
    }
    /* The digit at `first` sits in the place worth 10^(point - 1 - first). */
    *exponent = (int)point - 1 - (int)first;
    for (index = 0; index < count; ++index) {
        size_t at = first + index;
        mantissa[index] = at < total ? (char)('0' + digits[total - 1 - at]) : '0';
    }
    mantissa[count] = '\0';
    /* Only the deciding digit is consulted: at or above five the answer rounds up whatever
     * follows it. A place past the end of the exact expansion is a zero and decides nothing. */
    if (first + count < total && digits[total - 1 - (first + count)] >= 5) {
        carry_at = count;
        for (;;) {
            if (carry_at == 0) {
                /* The carry ran off the front: every digit was a nine, so the mantissa is a one
                 * followed by zeros and the exponent moved up one. */
                mantissa[0] = '1';
                for (index = 1; index < count; ++index) {
                    mantissa[index] = '0';
                }
                *exponent += 1;
                break;
            }
            carry_at -= 1;
            if (mantissa[carry_at] == '9') {
                mantissa[carry_at] = '0';
                continue;
            }
            mantissa[carry_at] += 1;
            break;
        }
    }
}

/* The *shortest* mantissa and decimal exponent that name a finite non-negative value exactly,
 * which is what `toExponential` with no argument asks for.
 *
 * Read off the number provider's own shortest spelling rather than decided again here. That
 * spelling is already the fewest digits that round-trip (ADR 0018), so reading it is the only
 * way `(1234.5678).toExponential()` and `String(1234.5678)` cannot come to disagree about how
 * many digits the value needs. */
static int kiln_c99_shortest_digits(double value, char *mantissa, size_t capacity, int *exponent) {
    kiln_value spelled = kiln_c99_text(kiln_c99_number(value));
    const char *bytes;
    size_t len;
    size_t body;
    size_t index;
    size_t produced = 0;
    size_t point;
    size_t first;
    size_t last;
    char raw[64];
    int suffix = 0;
    int suffix_sign = 1;
    if (spelled.tag != KILN_VALUE_STRING) {
        return KILN_C99_OUT_OF_MEMORY;
    }
    bytes = (const char *)spelled.ptr;
    len = spelled.len;
    /* `1e+21` and above, and `1e-7` and below, are spelled with an exponent already; everything
     * between is spelled positionally. Both forms reduce to the same digits-and-a-place. */
    body = len;
    for (index = 0; index < len; ++index) {
        if (bytes[index] == 'e' || bytes[index] == 'E') {
            size_t at = index + 1;
            body = index;
            if (at < len && (bytes[at] == '+' || bytes[at] == '-')) {
                suffix_sign = bytes[at] == '-' ? -1 : 1;
                at += 1;
            }
            for (; at < len; ++at) {
                if (bytes[at] < '0' || bytes[at] > '9') {
                    break;
                }
                suffix = suffix * 10 + (bytes[at] - '0');
            }
            suffix *= suffix_sign;
            break;
        }
    }
    point = (size_t)-1;
    for (index = 0; index < body && produced < sizeof raw; ++index) {
        if (bytes[index] == '.') {
            point = produced;
            continue;
        }
        if (bytes[index] < '0' || bytes[index] > '9') {
            /* A sign; the caller asked about a magnitude. */
            continue;
        }
        raw[produced++] = bytes[index];
    }
    if (point == (size_t)-1) {
        point = produced;
    }
    for (first = 0; first < produced && raw[first] == '0'; ++first) {
    }
    if (first == produced) {
        if (capacity < 2) {
            return KILN_C99_OUT_OF_MEMORY;
        }
        mantissa[0] = '0';
        mantissa[1] = '\0';
        *exponent = 0;
        return KILN_C99_OK;
    }
    /* Trailing zeros are place-holding, not significant: `100` is `1e+2`, not `100e+0`. */
    for (last = produced; last > first + 1 && raw[last - 1] == '0'; --last) {
    }
    if (last - first + 1 > capacity) {
        return KILN_C99_OUT_OF_MEMORY;
    }
    for (index = first; index < last; ++index) {
        mantissa[index - first] = raw[index];
    }
    mantissa[last - first] = '\0';
    *exponent = (int)point - 1 - (int)first + suffix;
    return KILN_C99_OK;
}

/* One mantissa and exponent as both exponential formats write them: `1.25e+2`. */
static int kiln_c99_exponential_text(kiln_value *out, int negative, const char *mantissa,
                                     int exponent) {
    kiln_text text = {NULL, 0, 0};
    char rendered[16];
    size_t length = strlen(mantissa);
    unsigned magnitude = exponent < 0 ? (unsigned)-(long)exponent : (unsigned)exponent;
    size_t at = sizeof rendered;
    int failed = 0;
    if (negative) {
        failed = failed || kiln_c99_text_append_c(&text, "-") != KILN_C99_OK;
    }
    failed = failed || kiln_c99_text_append(&text, mantissa, 1) != KILN_C99_OK;
    if (length > 1) {
        failed = failed || kiln_c99_text_append_c(&text, ".") != KILN_C99_OK;
        failed = failed || kiln_c99_text_append(&text, mantissa + 1, length - 1) != KILN_C99_OK;
    }
    failed = failed || kiln_c99_text_append_c(&text, "e") != KILN_C99_OK;
    /* The sign is always written, which is the whole difference from `String(n)`'s own
     * exponential form for a positive exponent. */
    failed = failed || kiln_c99_text_append_c(&text, exponent < 0 ? "-" : "+") != KILN_C99_OK;
    do {
        rendered[--at] = (char)('0' + magnitude % 10u);
        magnitude /= 10u;
    } while (magnitude != 0 && at > 0);
    failed = failed ||
             kiln_c99_text_append(&text, rendered + at, sizeof rendered - at) != KILN_C99_OK;
    if (failed) {
        free(text.bytes);
        return KILN_C99_OUT_OF_MEMORY;
    }
    *out = kiln_c99_value(KILN_VALUE_STRING, 0, text.bytes, text.len);
    return KILN_C99_OK;
}

/* `toPrecision`'s positional form, and its own window for when to give up on one.
 *
 * The switch to the exponential form is the specification's, and it is what keeps
 * `(0.0000001).toPrecision(2)` from being a string of zeros with no digits in it. */
static int kiln_c99_precision_text(kiln_value *out, int negative, const char *mantissa,
                                   size_t digits, int exponent) {
    kiln_text text = {NULL, 0, 0};
    size_t length = strlen(mantissa);
    size_t split;
    int index;
    int failed = 0;
    if (exponent < -6 || exponent >= (int)digits) {
        return kiln_c99_exponential_text(out, negative, mantissa, exponent);
    }
    if (negative) {
        failed = failed || kiln_c99_text_append_c(&text, "-") != KILN_C99_OK;
    }
    if (exponent == (int)digits - 1) {
        /* Every significant digit is above the point, so there is no point. */
        failed = failed || kiln_c99_text_append(&text, mantissa, length) != KILN_C99_OK;
    } else if (exponent >= 0) {
        split = (size_t)exponent + 1;
        failed = failed || kiln_c99_text_append(&text, mantissa, split) != KILN_C99_OK;
        failed = failed || kiln_c99_text_append_c(&text, ".") != KILN_C99_OK;
        failed = failed ||
                 kiln_c99_text_append(&text, mantissa + split, length - split) != KILN_C99_OK;
    } else {
        failed = failed || kiln_c99_text_append_c(&text, "0.") != KILN_C99_OK;
        for (index = 0; index < -exponent - 1; ++index) {
            failed = failed || kiln_c99_text_append_c(&text, "0") != KILN_C99_OK;
        }
        failed = failed || kiln_c99_text_append(&text, mantissa, length) != KILN_C99_OK;
    }
    if (failed) {
        free(text.bytes);
        return KILN_C99_OUT_OF_MEMORY;
    }
    *out = kiln_c99_value(KILN_VALUE_STRING, 0, text.bytes, text.len);
    return KILN_C99_OK;
}

/* A number written in a base other than ten, which `Number.prototype.toString` is the only way
 * to ask for. Digits are produced by repeated division rather than by the number formatter,
 * because the formatter only knows base ten -- and the fraction is bounded at 52 digits, which is
 * every bit a double has to say. */
static int kiln_c99_number_in_radix(kiln_value *out, double value, int radix) {
    kiln_text text = {NULL, 0, 0};
    static const char alphabet[] = "0123456789abcdefghijklmnopqrstuvwxyz";
    char digits[1120];
    size_t count = 0;
    double whole;
    double fraction;
    int negative;
    if (!isfinite(value)) {
        *out = kiln_c99_text(kiln_c99_number(value));
        return KILN_C99_OK;
    }
    negative = value < 0.0;
    value = fabs(value);
    whole = trunc(value);
    fraction = value - whole;
    while (whole >= 1.0 && count < sizeof digits) {
        digits[count++] = alphabet[(int)fmod(whole, (double)radix)];
        whole = trunc(whole / (double)radix);
    }
    if (negative && kiln_c99_text_append_c(&text, "-") != KILN_C99_OK) {
        free(text.bytes);
        return KILN_C99_OUT_OF_MEMORY;
    }
    if (count == 0 && kiln_c99_text_append_c(&text, "0") != KILN_C99_OK) {
        free(text.bytes);
        return KILN_C99_OUT_OF_MEMORY;
    }
    while (count > 0) {
        count -= 1;
        if (kiln_c99_text_append(&text, digits + count, 1) != KILN_C99_OK) {
            free(text.bytes);
            return KILN_C99_OUT_OF_MEMORY;
        }
    }
    if (fraction > 0.0) {
        int place;
        if (kiln_c99_text_append_c(&text, ".") != KILN_C99_OK) {
            free(text.bytes);
            return KILN_C99_OUT_OF_MEMORY;
        }
        for (place = 0; place < 52 && fraction > 0.0; ++place) {
            double digit;
            char character;
            fraction *= (double)radix;
            digit = trunc(fraction);
            character = alphabet[(int)digit];
            fraction -= digit;
            if (kiln_c99_text_append(&text, &character, 1) != KILN_C99_OK) {
                free(text.bytes);
                return KILN_C99_OUT_OF_MEMORY;
            }
        }
    }
    *out = kiln_c99_text_value(&text);
    return KILN_C99_OK;
}

/* The JavaScript name of a runtime-supplied value, which is what a console shows and what
 * `String(x)` produces. The interpreter derives its equivalent from the variant's spelling; here
 * the tags carry no spelling to derive from, so this is the derivation -- and
 * `conformance/differential/builtin-names.js` is what checks the two agree with node. */
static const char *kiln_c99_builtin_name(uint64_t builtin) {
    switch (builtin) {
    /* The globals this backend has not implemented still have to name themselves, because the
     * name is what the refusal reports and what `X.name` answers. */
    case KILN_C99_BUILTIN_INTL: return "Intl";
    case KILN_C99_BUILTIN_SHARED_ARRAY_BUFFER: return "SharedArrayBuffer";
    case KILN_C99_BUILTIN_WEAK_REF: return "WeakRef";
    case KILN_C99_BUILTIN_FINALIZATION_REGISTRY: return "FinalizationRegistry";
    case KILN_C99_BUILTIN_SHADOW_REALM: return "ShadowRealm";
    case KILN_C99_BUILTIN_FLOAT16_ARRAY: return "Float16Array";
    case KILN_C99_BUILTIN_BIG_INT64_ARRAY: return "BigInt64Array";
    case KILN_C99_BUILTIN_BIG_UINT64_ARRAY: return "BigUint64Array";
    case KILN_C99_BUILTIN_DISPOSABLE_STACK: return "DisposableStack";
    case KILN_C99_BUILTIN_ASYNC_DISPOSABLE_STACK: return "AsyncDisposableStack";
    case KILN_C99_BUILTIN_SUPPRESSED_ERROR: return "SuppressedError";
    case KILN_C99_BUILTIN_EVAL_ERROR: return "EvalError";
    case KILN_C99_BUILTIN_TEXT_DECODER: return "TextDecoder";
    case KILN_C99_BUILTIN_EVAL: return "eval";
    /* Lower-case because that is what the language calls it: `performance` is an object a
     * program reads `now` from, not a constructor it names. */
    case KILN_C99_BUILTIN_PERFORMANCE: return "performance";
    case KILN_C99_BUILTIN_STRING: return "String";
    case KILN_C99_BUILTIN_MATH: return "Math";
    case KILN_C99_BUILTIN_NUMBER: return "Number";
    case KILN_C99_BUILTIN_BIGINT: return "BigInt";
    case KILN_C99_BUILTIN_OBJECT: return "Object";
    case KILN_C99_BUILTIN_ARRAY: return "Array";
    case KILN_C99_BUILTIN_JSON: return "JSON";
    case KILN_C99_BUILTIN_DATE: return "Date";
    case KILN_C99_BUILTIN_MAP: return "Map";
    case KILN_C99_BUILTIN_SET: return "Set";
    case KILN_C99_BUILTIN_WEAK_MAP: return "WeakMap";
    case KILN_C99_BUILTIN_WEAK_SET: return "WeakSet";
    case KILN_C99_BUILTIN_PROMISE: return "Promise";
    case KILN_C99_BUILTIN_BOOLEAN: return "Boolean";
    case KILN_C99_BUILTIN_REFLECT: return "Reflect";
    case KILN_C99_BUILTIN_PROXY: return "Proxy";
    case KILN_C99_BUILTIN_URL: return "URL";
    case KILN_C99_BUILTIN_BUFFER: return "Buffer";
    /* The three web constructors this realm publishes. Absent from this switch they fell through
     * to the error-name lookup and reported "anonymous" -- a name no program can have asked for
     * and the very thing `X.name` is read to distinguish. `Object.getOwnPropertyNames(globalThis)`
     * publishes them; `every_global_reports_the_name_it_is_published_under` now proves each one
     * answers the name it went out under. */
    case KILN_C99_BUILTIN_ABORT_CONTROLLER: return "AbortController";
    case KILN_C99_BUILTIN_ABORT_SIGNAL: return "AbortSignal";
    case KILN_C99_BUILTIN_TEXT_ENCODER: return "TextEncoder";
    case KILN_C99_BUILTIN_TEMPORAL: return "Temporal";
    case KILN_C99_BUILTIN_TEMPORAL_NOW: return "Now";
    case KILN_C99_BUILTIN_TEMPORAL_PLAIN_DATE: return "PlainDate";
    case KILN_C99_BUILTIN_TEMPORAL_PLAIN_TIME: return "PlainTime";
    case KILN_C99_BUILTIN_TEMPORAL_PLAIN_DATE_TIME: return "PlainDateTime";
    case KILN_C99_BUILTIN_TEMPORAL_PLAIN_YEAR_MONTH: return "PlainYearMonth";
    case KILN_C99_BUILTIN_TEMPORAL_PLAIN_MONTH_DAY: return "PlainMonthDay";
    case KILN_C99_BUILTIN_TEMPORAL_DURATION: return "Duration";
    case KILN_C99_BUILTIN_TEMPORAL_INSTANT: return "Instant";
    case KILN_C99_BUILTIN_TEMPORAL_ZONED_DATE_TIME: return "ZonedDateTime";
    case KILN_C99_BUILTIN_OBJECT_PROTOTYPE: return "Object";
    case KILN_C99_BUILTIN_ARRAY_PROTOTYPE: return "Array";
    case KILN_C99_BUILTIN_ERROR_PROTOTYPE: return "Error";
    case KILN_C99_BUILTIN_AGGREGATE_ERROR_PROTOTYPE: return "AggregateError";
    case KILN_C99_BUILTIN_MAP_PROTOTYPE: return "Map";
    case KILN_C99_BUILTIN_SET_PROTOTYPE: return "Set";
    case KILN_C99_BUILTIN_WEAK_MAP_PROTOTYPE: return "WeakMap";
    case KILN_C99_BUILTIN_WEAK_SET_PROTOTYPE: return "WeakSet";
    case KILN_C99_BUILTIN_STRING_PROTOTYPE: return "String";
    /* Both spell "Iterator": the constructor answers it for `Iterator.name`, and the table
     * answers it because a refusal naming the table has to name the global a reader can find. */
    case KILN_C99_BUILTIN_ITERATOR: return "Iterator";
    case KILN_C99_BUILTIN_ITERATOR_PROTOTYPE: return "Iterator";
    case KILN_C99_BUILTIN_SYMBOL: return "Symbol";
    case KILN_C99_BUILTIN_REGEXP: return "RegExp";
    case KILN_C99_BUILTIN_REGEXP_PROTOTYPE: return "RegExp";
    case KILN_C99_BUILTIN_DATE_PROTOTYPE: return "Date";
    case KILN_C99_BUILTIN_NUMBER_PROTOTYPE: return "Number";
    case KILN_C99_BUILTIN_FUNCTION_PROTOTYPE: return "Function";
    case KILN_C99_BUILTIN_FUNCTION: return "Function";
    case KILN_C99_BUILTIN_ARRAY_BUFFER: return "ArrayBuffer";
    case KILN_C99_BUILTIN_DATA_VIEW: return "DataView";
    /* A prototype table answers with its constructor's name, as every other pair here does. */
    case KILN_C99_BUILTIN_DATA_VIEW_PROTOTYPE: return "DataView";
    case KILN_C99_BUILTIN_INT8_ARRAY: return "Int8Array";
    case KILN_C99_BUILTIN_UINT8_ARRAY: return "Uint8Array";
    case KILN_C99_BUILTIN_UINT8_CLAMPED_ARRAY: return "Uint8ClampedArray";
    case KILN_C99_BUILTIN_INT16_ARRAY: return "Int16Array";
    case KILN_C99_BUILTIN_UINT16_ARRAY: return "Uint16Array";
    case KILN_C99_BUILTIN_INT32_ARRAY: return "Int32Array";
    case KILN_C99_BUILTIN_UINT32_ARRAY: return "Uint32Array";
    case KILN_C99_BUILTIN_FLOAT32_ARRAY: return "Float32Array";
    case KILN_C99_BUILTIN_FLOAT64_ARRAY: return "Float64Array";
    case KILN_C99_BUILTIN_TYPED_ARRAY_PROTOTYPE: return "TypedArray";
    case KILN_C99_BUILTIN_INT8_ARRAY_PROTOTYPE: return "Int8Array";
    case KILN_C99_BUILTIN_UINT8_ARRAY_PROTOTYPE: return "Uint8Array";
    case KILN_C99_BUILTIN_UINT8_CLAMPED_ARRAY_PROTOTYPE: return "Uint8ClampedArray";
    case KILN_C99_BUILTIN_INT16_ARRAY_PROTOTYPE: return "Int16Array";
    case KILN_C99_BUILTIN_UINT16_ARRAY_PROTOTYPE: return "Uint16Array";
    case KILN_C99_BUILTIN_INT32_ARRAY_PROTOTYPE: return "Int32Array";
    case KILN_C99_BUILTIN_UINT32_ARRAY_PROTOTYPE: return "Uint32Array";
    case KILN_C99_BUILTIN_FLOAT32_ARRAY_PROTOTYPE: return "Float32Array";
    case KILN_C99_BUILTIN_FLOAT64_ARRAY_PROTOTYPE: return "Float64Array";
    default: {
        /* Every other tag is an error constructor; a tag with no name at all is a tag added
         * without one, which shows as anonymous rather than dereferencing a null. */
        const char *name = kiln_c99_error_name(builtin);
        return name == NULL ? "anonymous" : name;
    }
    }
}

/* Expand a replacement template against one match.
 *
 * `$$` is a literal dollar, `$&` the match, `` $` `` what came before it, `$'` what came after,
 * and `$1`..`$99` a capture group -- and a `$` in front of anything else stands for itself. The
 * template is a template even when the pattern was a plain string, which is why this is shared
 * rather than living in the regexp path: `"a".replace("a", "$&$&")` is `"aa"`.
 *
 * `ranges` is the group offsets a search reported, with the whole match at index zero. A literal
 * replacement passes the occurrence as the only group, which is what makes one function serve. */
static int kiln_c99_expand_replacement(kiln_text *out, const uint16_t *pattern,
                                       size_t pattern_count, const uint16_t *subject,
                                       size_t subject_count, const int64_t *ranges,
                                       size_t groups) {
    size_t at = 0;
    while (at < pattern_count) {
        uint16_t unit = pattern[at];
        size_t taken = 1;
        const uint16_t *piece = NULL;
        size_t piece_count = 0;
        uint16_t literal = unit;
        if (unit == (uint16_t)'$' && at + 1 < pattern_count) {
            uint16_t next = pattern[at + 1];
            if (next == (uint16_t)'$') {
                taken = 2;
            } else if (next == (uint16_t)'&') {
                piece = subject + (size_t)ranges[0];
                piece_count = (size_t)(ranges[1] - ranges[0]);
                taken = 2;
            } else if (next == (uint16_t)'`') {
                piece = subject;
                piece_count = (size_t)ranges[0];
                taken = 2;
            } else if (next == (uint16_t)'\'') {
                piece = subject + (size_t)ranges[1];
                piece_count = subject_count - (size_t)ranges[1];
                taken = 2;
            } else if (next >= (uint16_t)'0' && next <= (uint16_t)'9') {
                /* Two digits when two would name a group that exists, one otherwise -- so `$12`
                 * is group 12 in a pattern with twelve and group 1 followed by a `2` in one
                 * without. */
                size_t wanted = (size_t)(next - (uint16_t)'0');
                taken = 2;
                if (at + 2 < pattern_count && pattern[at + 2] >= (uint16_t)'0' &&
                    pattern[at + 2] <= (uint16_t)'9') {
                    size_t wider = wanted * 10 + (size_t)(pattern[at + 2] - (uint16_t)'0');
                    if (wider < groups) {
                        wanted = wider;
                        taken = 3;
                    }
                }
                if (wanted == 0 || wanted >= groups) {
                    /* Not a group: the text stands for itself. */
                    taken = 1;
                } else if (ranges[wanted * 2] < 0) {
                    /* A group that did not participate contributes nothing. */
                    piece = subject;
                    piece_count = 0;
                } else {
                    piece = subject + (size_t)ranges[wanted * 2];
                    piece_count = (size_t)(ranges[wanted * 2 + 1] - ranges[wanted * 2]);
                }
            }
        }
        if (piece != NULL) {
            kiln_value text = kiln_c99_from_units(piece, piece_count);
            if (text.tag != KILN_VALUE_STRING ||
                kiln_c99_text_append(out, text.ptr, text.len) != KILN_C99_OK) {
                return KILN_C99_OUT_OF_MEMORY;
            }
        } else {
            kiln_value text = kiln_c99_from_units(&literal, 1);
            if (text.tag != KILN_VALUE_STRING ||
                kiln_c99_text_append(out, text.ptr, text.len) != KILN_C99_OK) {
                return KILN_C99_OUT_OF_MEMORY;
            }
        }
        at += taken;
    }
    return KILN_C99_OK;
}

/* The constructor the running call was reached through, or undefined.
 *
 * A module-level value with save-and-restore around every call, rather than a field on a frame:
 * the body signature is fixed by the embedding ABI, and the C stack already gives the discipline
 * this needs -- each call restores what it found. */
static kiln_value kiln_c99_new_target;

/* Set when a `super(...)` is about to call, and cleared by the call that honours it.
 *
 * `super()` keeps the `new.target` of the call that is building the instance, which is how a base
 * constructor can name the subclass it is being built for -- so exactly one call has to *not*
 * compute a fresh one. */
static int kiln_c99_inheriting_new_target;

/* What a derived constructor's `super()` produced, when a built-in parent produced something
 * other than the instance `new` allocated.
 *
 * A constructor body reassigns its own `receiver` when that happens, but the *caller* -- the
 * `new` that allocated the abandoned instance, or an outer `super()` one link down the chain --
 * has no other way to hear about it: a constructor returns undefined, so nothing in the value it
 * hands back says which object `this` ended up being. This is that channel. Saved and restored
 * around every construction and every `super()` so a nested one cannot be mistaken for this
 * one. */
static kiln_value kiln_c99_adopted_this;

/* The running call's own arguments, and the list an arrow sees instead.
 *
 * An arrow has no `arguments` of its own -- `arguments` inside one is the enclosing function's,
 * which is a *different list* from the arrow's own rest parameter. So an arrow captures the
 * visible list where it was written, exactly as it captures `this` and `new.target`, and the
 * three are saved and restored together around every call for the same reason.
 *
 * The raw pointer and count rather than an array, because a call must not pay to materialise an
 * `arguments` nothing asked for; the array is built where one is actually read. */
static const kiln_value *kiln_c99_frame_args;
static size_t kiln_c99_frame_argc;
static kiln_value kiln_c99_captured_arguments;
static int kiln_c99_frame_is_arrow;
/* The one `arguments` object this call has handed out, built on first mention and undefined
 * until then.
 *
 * `arguments` names a single object per call, not a snapshot: `arguments[0] = 1` has to be
 * visible to the next read of `arguments`, and `arguments === arguments` is true. Materialising
 * a fresh array per mention made both false, and every definition made on one was discarded by
 * the statement after it. Still built lazily, so a call nothing asks it of pays nothing. */
static kiln_value kiln_c99_frame_arguments_object;
/* Whether this call's `arguments` is the *mapped* kind: sloppy code with a simple parameter
 * list. Written by the body's own prologue, because it is a fact about the function that the
 * compiler decided and the emitted code carries as a constant. */
static int kiln_c99_frame_mapped_arguments;
/* The function this call is of, for a mapped `arguments` object's `callee`. */
static kiln_value kiln_c99_frame_callee;

/* `CreateUnmappedArgumentsObject`: an ordinary object carrying the call's arguments.
 *
 * It used to be an array, which is the wrong answer to four questions a program actually asks:
 * `Array.isArray(arguments)` said true, `Object.prototype.toString.call(arguments)` said
 * `[object Array]`, `Object.getPrototypeOf(arguments)` was not `Object.prototype`, and `length`
 * was the array's own rather than an ordinary writable property nothing keeps in step with the
 * indices. Every array-generic idiom that reaches for `arguments` -- `slice.call`, `apply`,
 * `Array.from`, spread, `for-of` -- goes through the array-like path instead, which is where the
 * language defines them.
 *
 * `@@iterator` is `%Array.prototype.values%` as an *own* property: `arguments` inherits from
 * `Object.prototype`, which has no iterator, so without it a spread would stop working the
 * moment this stopped being an array.
 *
 * The parameter *map* of a mapped `arguments` -- where `arguments[0]` and the first parameter
 * are two names for one binding -- is deliberately not here; the interpreter does not have it
 * either, and the two engines have to gain it together. `callee` still tells the two objects
 * apart, because that much is decidable from the function alone. */
static int kiln_c99_build_arguments_object(kiln_value *out) {
    size_t at;
    int status = kiln_c99_new_object(out);
    kiln_property *slot;
    if (status != KILN_C99_OK) {
        return status;
    }
    kiln_c99_object_of(*out)->arguments_brand = 1;
    /* `length` first, then the indices: integer keys sort ahead of string ones however they were
     * inserted, so what this order decides is that `length` precedes `callee`. */
    status = kiln_c99_own_slot(*out, "length", 6, &slot);
    if (status != KILN_C99_OK) {
        return status;
    }
    slot->accessor = 0;
    slot->value = kiln_c99_number((double)kiln_c99_frame_argc);
    slot->enumerable = 0;
    slot->writable = 1;
    slot->configurable = 1;
    for (at = 0; at < kiln_c99_frame_argc; ++at) {
        kiln_value key = kiln_c99_property_key(kiln_c99_number((double)at));
        if (key.tag != KILN_VALUE_STRING) {
            return KILN_C99_OUT_OF_MEMORY;
        }
        status = kiln_c99_own_slot(*out, key.ptr, key.len, &slot);
        if (status != KILN_C99_OK) {
            return status;
        }
        slot->accessor = 0;
        slot->value = kiln_c99_frame_args[at];
        slot->enumerable = 1;
        slot->writable = 1;
        slot->configurable = 1;
    }
    status = kiln_c99_own_slot(*out, "callee", 6, &slot);
    if (status != KILN_C99_OK) {
        return status;
    }
    if (kiln_c99_frame_mapped_arguments) {
        slot->accessor = 0;
        slot->value = kiln_c99_frame_callee;
        slot->enumerable = 0;
        slot->writable = 1;
        slot->configurable = 1;
    } else {
        /* An unmapped `arguments` refuses to name its function: reading `callee` is a TypeError
         * rather than `undefined`, because a program reaching for it wants the caller identity
         * strict mode exists to withhold. */
        slot->accessor = 1;
        kiln_c99_object_of(*out)->accessor_count += 1;
        slot->getter = kiln_c99_method(KILN_C99_METHOD_THROW_TYPE_ERROR);
        slot->setter = kiln_c99_method(KILN_C99_METHOD_THROW_TYPE_ERROR);
        slot->enumerable = 0;
        slot->writable = 0;
        slot->configurable = 0;
    }
    status = kiln_c99_own_slot(*out, "@@iterator", 10, &slot);
    if (status != KILN_C99_OK) {
        return status;
    }
    slot->accessor = 0;
    slot->value = kiln_c99_method(KILN_C99_METHOD_ARRAY_VALUES);
    slot->enumerable = 0;
    slot->writable = 1;
    slot->configurable = 1;
    return KILN_C99_OK;
}

/* The `arguments` this frame can see: an arrow's captured object, or this call's own. */
static int kiln_c99_visible_arguments(kiln_value *out) {
    int status;
    if (kiln_c99_frame_is_arrow) {
        *out = kiln_c99_captured_arguments;
        return KILN_C99_OK;
    }
    if (kiln_c99_frame_arguments_object.tag == KILN_VALUE_OBJECT) {
        *out = kiln_c99_frame_arguments_object;
        return KILN_C99_OK;
    }
    status = kiln_c99_build_arguments_object(out);
    if (status != KILN_C99_OK) {
        return status;
    }
    kiln_c99_frame_arguments_object = *out;
    return KILN_C99_OK;
}

/* The realm's global object, created on first use.
 *
 * A real object rather than a tag, because a program *writes* to it: `globalThis.x = 1` has to be
 * found again by `globalThis.x`, and a builtin has nowhere to keep it. The names it starts with
 * are non-enumerable, which is what node does and what stops `Object.keys(globalThis)` from being
 * a list of the realm's furniture -- a program's own additions are enumerable, which is the
 * distinction that makes the question worth asking. */
#define KILN_C99_REALM_SLOT_GLOBALS UINT32_C(1)
#define KILN_C99_REALM_SLOT_PROCESS UINT32_C(2)
#define KILN_C99_REALM_SLOT_ATOMICS UINT32_C(3)
#define KILN_C99_REALM_SLOT_CONSOLE UINT32_C(4)
/* The prototype every `node:http` request object links to. A realm slot rather than a static,
 * because a realm owns it and the runtime archive already enumerates these slots as GC roots
 * (ADR 0041) -- and because one identity across translation units is what a prototype is for. */
#define KILN_C99_REALM_SLOT_INCOMING_MESSAGE UINT32_C(5)

static int kiln_c99_realm_value_slot(uint32_t key, kiln_value **out) {
    void *slot = NULL;
    int status;
    if (out == NULL) return KILN_C99_INVALID_ARGUMENT;
    status = kiln_realm_value_slot(key, &slot);
    if (status != KILN_C99_OK || slot == NULL) return status;
    *out = (kiln_value *)slot;
    return KILN_C99_OK;
}

static int kiln_c99_atomics_object(kiln_value *out) {
    static const char *const names[] = {
        "add", "and", "compareExchange", "exchange", "isLockFree", "load", "notify",
        "or", "pause", "store", "sub", "wait", "waitAsync", "xor",
    };
    kiln_value *atomics;
    size_t index;
    int status = kiln_c99_realm_value_slot(KILN_C99_REALM_SLOT_ATOMICS, &atomics);
    if (status != KILN_C99_OK) return status;
    if (atomics->tag == KILN_VALUE_OBJECT) {
        *out = *atomics;
        return KILN_C99_OK;
    }
    status = kiln_c99_new_object(atomics);
    for (index = 0; status == KILN_C99_OK && index < sizeof names / sizeof *names; ++index)
        status = kiln_c99_define_method(
            *atomics, names[index], strlen(names[index]),
            kiln_c99_method(KILN_C99_METHOD_ATOMICS_UNAVAILABLE));
    /* `Atomics[Symbol.toStringTag]` is a string, not a shared-memory operation, so nothing about
     * the missing `SharedArrayBuffer` stops this backend from answering it -- and it read
     * `undefined` here against the interpreter's "Atomics", which is what
     * `Object.prototype.toString.call(Atomics)` reads and what a `@@toStringTag` sniff branches
     * on. The backlog row that says Atomics refuses by name is about the fourteen operations
     * above; this is the one member of the surface that is only a name. */
    if (status == KILN_C99_OK)
        status = kiln_c99_define_method(*atomics, "@@toStringTag", 13,
                                        kiln_c99_copy_text("Atomics", 7));
    if (status != KILN_C99_OK) return status;
    *out = *atomics;
    return KILN_C99_OK;
}

/* The realm's `console`, created on first use.
 *
 * `console.log(x)` written out in full is a compile-time shape -- the compiler turns it into a
 * host call and no `console` value exists at all -- so this object is what the *other* spellings
 * reach: `globalThis.console`, `const c = console`, `[1, 2].forEach(console.log)`. Without it
 * `typeof globalThis.console` answered "undefined" here and "object" in the interpreter, which is
 * the shape of feature-detection every bundle does before it decides whether it can log at all.
 *
 * `log` and `info` are the same host operation the compiled shape uses. `error` and `warn` are
 * not: the interpreter sends those to standard *error*, and the host ABI publishes one captured
 * output operation, so there is nothing here that could put bytes on the other stream. They
 * refuse by name rather than quietly printing to stdout, which would be the two engines
 * disagreeing about which stream a line went to -- invisible in a terminal and wrong in a pipe. */
static int kiln_c99_console_object(kiln_value *out) {
    kiln_value *console;
    int status = kiln_c99_realm_value_slot(KILN_C99_REALM_SLOT_CONSOLE, &console);
    if (status != KILN_C99_OK) return status;
    if (console->tag == KILN_VALUE_OBJECT) {
        *out = *console;
        return KILN_C99_OK;
    }
    status = kiln_c99_new_object(console);
    if (status == KILN_C99_OK)
        status = kiln_c99_define_method(*console, "log", 3,
                                        kiln_c99_method(KILN_C99_METHOD_CONSOLE_LOG));
    if (status == KILN_C99_OK)
        status = kiln_c99_define_method(*console, "info", 4,
                                        kiln_c99_method(KILN_C99_METHOD_CONSOLE_LOG));
    if (status == KILN_C99_OK)
        status = kiln_c99_define_method(*console, "error", 5,
                                        kiln_c99_method(KILN_C99_METHOD_CONSOLE_ERROR));
    if (status == KILN_C99_OK)
        status = kiln_c99_define_method(*console, "warn", 4,
                                        kiln_c99_method(KILN_C99_METHOD_CONSOLE_ERROR));
    if (status != KILN_C99_OK) return status;
    *out = *console;
    return KILN_C99_OK;
}

static int kiln_c99_process_object(kiln_value *out) {
    kiln_value *process;
    kiln_value env;
    kiln_value argv;
    kiln_value exec_argv;
    kiln_value versions;
    kiln_value stream;
    int status = kiln_c99_realm_value_slot(KILN_C99_REALM_SLOT_PROCESS, &process);
    if (status != KILN_C99_OK) return status;
    if (process->tag == KILN_VALUE_OBJECT) {
        *out = *process;
        return KILN_C99_OK;
    }
    status = kiln_c99_new_object(process);
    if (status == KILN_C99_OK) status = kiln_c99_new_object(&env);
    if (status == KILN_C99_OK) status = kiln_c99_new_array(&argv);
    if (status == KILN_C99_OK) status = kiln_c99_new_array(&exec_argv);
    if (status == KILN_C99_OK) status = kiln_c99_new_object(&versions);
    if (status == KILN_C99_OK)
        status = kiln_c99_put(NULL, versions, "node", 4,
                              kiln_c99_copy_text("22.0.0", 6), 0, NULL);
    if (status == KILN_C99_OK)
        status = kiln_c99_put(NULL, *process, "env", 3, env, 0, NULL);
    if (status == KILN_C99_OK)
        status = kiln_c99_put(NULL, *process, "argv", 4, argv, 0, NULL);
    if (status == KILN_C99_OK)
        status = kiln_c99_put(NULL, *process, "execArgv", 8, exec_argv, 0, NULL);
    if (status == KILN_C99_OK)
        status = kiln_c99_put(NULL, *process, "platform", 8,
                              kiln_c99_copy_text("kiln", 4), 0, NULL);
    if (status == KILN_C99_OK)
        status = kiln_c99_put(NULL, *process, "version", 7,
                              kiln_c99_copy_text("kiln", 4), 0, NULL);
    if (status == KILN_C99_OK)
        status = kiln_c99_put(NULL, *process, "versions", 8, versions, 0, NULL);
    if (status == KILN_C99_OK)
        status = kiln_c99_put(NULL, *process, "pid", 3,
                              kiln_c99_number(1.0), 0, NULL);
    /* `process.argv` follows node's single-executable-application convention, because a compiled
     * kiln program is one: there is no separate interpreter to name in slot 0 and no script file
     * to name in slot 1, so both name the executable and the program's own arguments still start
     * at index 2 -- which is where every argv-parsing library, and `const [, , first] =
     * process.argv`, already looks. Reporting a one-element argv would be more literal and would
     * silently shift every argument by one. */
    if (status == KILN_C99_OK) {
        uint64_t argv_count = 0;
        if (kiln_realm_argv_count(&argv_count) == KILN_C99_OK && argv_count != 0) {
            const uint8_t *bytes = NULL;
            uint64_t bytes_len = 0;
            uint64_t index;
            status = kiln_realm_argv_at(0, &bytes, &bytes_len);
            /* Three copies rather than one shared allocation: a `kiln_value` string owns its
             * bytes, and two array slots plus a property naming the same buffer would be three
             * owners of one allocation. */
            for (index = 0; status == KILN_C99_OK && index < 2; ++index) {
                kiln_value executable = kiln_c99_copy_text((const char *)bytes,
                                                           (size_t)bytes_len);
                if (executable.tag != KILN_VALUE_STRING) status = KILN_C99_OUT_OF_MEMORY;
                else status = kiln_c99_array_push(argv, executable);
            }
            if (status == KILN_C99_OK) {
                kiln_value executable = kiln_c99_copy_text((const char *)bytes,
                                                           (size_t)bytes_len);
                if (executable.tag != KILN_VALUE_STRING) status = KILN_C99_OUT_OF_MEMORY;
                else status = kiln_c99_put(NULL, *process, "execPath", 8, executable, 0, NULL);
            }
            for (index = 1; status == KILN_C99_OK && index < argv_count; ++index) {
                kiln_value argument;
                status = kiln_realm_argv_at(index, &bytes, &bytes_len);
                if (status != KILN_C99_OK) break;
                argument = kiln_c99_copy_text((const char *)bytes, (size_t)bytes_len);
                if (argument.tag != KILN_VALUE_STRING) {
                    status = KILN_C99_OUT_OF_MEMORY;
                    break;
                }
                status = kiln_c99_array_push(argv, argument);
            }
        }
    }
    if (status == KILN_C99_OK) {
        status = kiln_c99_new_object(&stream);
        if (status == KILN_C99_OK)
            status = kiln_c99_put(NULL, stream, "fd", 2, kiln_c99_number(0.0), 0, NULL);
        if (status == KILN_C99_OK)
            status = kiln_c99_put(NULL, *process, "stdin", 5, stream, 0, NULL);
    }
    if (status == KILN_C99_OK) {
        status = kiln_c99_new_object(&stream);
        if (status == KILN_C99_OK)
            status = kiln_c99_put(NULL, stream, "fd", 2, kiln_c99_number(1.0), 0, NULL);
        if (status == KILN_C99_OK)
            status = kiln_c99_put(NULL, *process, "stdout", 6, stream, 0, NULL);
    }
    if (status == KILN_C99_OK) {
        status = kiln_c99_new_object(&stream);
        if (status == KILN_C99_OK)
            status = kiln_c99_put(NULL, stream, "fd", 2, kiln_c99_number(2.0), 0, NULL);
        if (status == KILN_C99_OK)
            status = kiln_c99_put(NULL, *process, "stderr", 6, stream, 0, NULL);
    }
    if (status == KILN_C99_OK)
        status = kiln_c99_define_method(*process, "nextTick", 8,
                                        kiln_c99_method(KILN_C99_METHOD_PROCESS_NEXT_TICK));
    if (status == KILN_C99_OK)
        status = kiln_c99_define_method(*process, "hrtime", 6,
                                        kiln_c99_method(KILN_C99_METHOD_PROCESS_HRTIME));
    if (status == KILN_C99_OK)
        status = kiln_c99_define_method(*process, "cwd", 3,
                                        kiln_c99_method(KILN_C99_METHOD_PROCESS_CWD));
    if (status != KILN_C99_OK) return status;
    *out = *process;
    return KILN_C99_OK;
}

static int kiln_c99_global_object(kiln_value *out) {
    kiln_value *globals;
    static const struct {
        const char *name;
        uint64_t builtin;
    } constructors[] = {{"String", KILN_C99_BUILTIN_STRING},
                        {"Number", KILN_C99_BUILTIN_NUMBER},
                        {"BigInt", KILN_C99_BUILTIN_BIGINT},
                        {"Boolean", KILN_C99_BUILTIN_BOOLEAN},
                        {"Object", KILN_C99_BUILTIN_OBJECT},
                        {"Array", KILN_C99_BUILTIN_ARRAY},
                        {"Symbol", KILN_C99_BUILTIN_SYMBOL},
                        {"RegExp", KILN_C99_BUILTIN_REGEXP},
                        {"Date", KILN_C99_BUILTIN_DATE},
                        {"Map", KILN_C99_BUILTIN_MAP},
                        {"Set", KILN_C99_BUILTIN_SET},
                        {"Promise", KILN_C99_BUILTIN_PROMISE},
                        {"Math", KILN_C99_BUILTIN_MATH},
                        {"JSON", KILN_C99_BUILTIN_JSON},
                        {"Reflect", KILN_C99_BUILTIN_REFLECT},
                        {"Temporal", KILN_C99_BUILTIN_TEMPORAL},
                        {"Proxy", KILN_C99_BUILTIN_PROXY},
                        {"Error", KILN_C99_BUILTIN_ERROR},
                        {"TypeError", KILN_C99_BUILTIN_TYPE_ERROR},
                        {"RangeError", KILN_C99_BUILTIN_RANGE_ERROR},
                        {"ReferenceError", KILN_C99_BUILTIN_REFERENCE_ERROR},
                        {"SyntaxError", KILN_C99_BUILTIN_SYNTAX_ERROR},
                        {"URIError", KILN_C99_BUILTIN_URI_ERROR},
                        {"AggregateError", KILN_C99_BUILTIN_AGGREGATE_ERROR},
                        {"AbortController", KILN_C99_BUILTIN_ABORT_CONTROLLER},
                        {"AbortSignal", KILN_C99_BUILTIN_ABORT_SIGNAL},
                        {"TextEncoder", KILN_C99_BUILTIN_TEXT_ENCODER},
                        /* Not a constructor, and here for the same reason the constructors are:
                         * `globalThis.performance` is the spelling a bundle's feature detection
                         * uses, and it has to find the same value the bare name does. */
                        {"performance", KILN_C99_BUILTIN_PERFORMANCE},
                        /* THE REST OF THE REALM'S FURNITURE. Everything below this line was
                         * reachable by its bare name -- `new WeakMap()` compiles and runs -- and
                         * absent from the global *object*, so `globalThis.WeakMap` answered
                         * `undefined` while `WeakMap` answered a constructor. Two lists for one
                         * question is how they drifted, and `globalThis.X` is the spelling a
                         * bundle's feature detection actually uses. One list now. */
                        {"Function", KILN_C99_BUILTIN_FUNCTION},
                        {"ArrayBuffer", KILN_C99_BUILTIN_ARRAY_BUFFER},
                        {"DataView", KILN_C99_BUILTIN_DATA_VIEW},
                        {"WeakMap", KILN_C99_BUILTIN_WEAK_MAP},
                        {"WeakSet", KILN_C99_BUILTIN_WEAK_SET},
                        {"URL", KILN_C99_BUILTIN_URL},
                        {"Buffer", KILN_C99_BUILTIN_BUFFER},
                        {"Int8Array", KILN_C99_BUILTIN_INT8_ARRAY},
                        {"Uint8Array", KILN_C99_BUILTIN_UINT8_ARRAY},
                        {"Uint8ClampedArray", KILN_C99_BUILTIN_UINT8_CLAMPED_ARRAY},
                        {"Int16Array", KILN_C99_BUILTIN_INT16_ARRAY},
                        {"Uint16Array", KILN_C99_BUILTIN_UINT16_ARRAY},
                        {"Int32Array", KILN_C99_BUILTIN_INT32_ARRAY},
                        {"Uint32Array", KILN_C99_BUILTIN_UINT32_ARRAY},
                        {"Float32Array", KILN_C99_BUILTIN_FLOAT32_ARRAY},
                        {"Float64Array", KILN_C99_BUILTIN_FLOAT64_ARRAY},
                        {"Iterator", KILN_C99_BUILTIN_ITERATOR},
                        /* AND THE ONES THIS BACKEND DOES NOT IMPLEMENT. Present so that `typeof`
                         * answers what the interpreter answers; every use of one refuses and
                         * names itself. `undefined` here would not stop a compiled program -- it
                         * would send it down the branch written for a realm that never had the
                         * feature, which is a different program from the one the author wrote. */
                        {"Intl", KILN_C99_BUILTIN_INTL},
                        {"SharedArrayBuffer", KILN_C99_BUILTIN_SHARED_ARRAY_BUFFER},
                        {"WeakRef", KILN_C99_BUILTIN_WEAK_REF},
                        {"FinalizationRegistry", KILN_C99_BUILTIN_FINALIZATION_REGISTRY},
                        {"ShadowRealm", KILN_C99_BUILTIN_SHADOW_REALM},
                        {"Float16Array", KILN_C99_BUILTIN_FLOAT16_ARRAY},
                        {"BigInt64Array", KILN_C99_BUILTIN_BIG_INT64_ARRAY},
                        {"BigUint64Array", KILN_C99_BUILTIN_BIG_UINT64_ARRAY},
                        {"DisposableStack", KILN_C99_BUILTIN_DISPOSABLE_STACK},
                        {"AsyncDisposableStack", KILN_C99_BUILTIN_ASYNC_DISPOSABLE_STACK},
                        {"SuppressedError", KILN_C99_BUILTIN_SUPPRESSED_ERROR},
                        {"EvalError", KILN_C99_BUILTIN_EVAL_ERROR},
                        {"TextDecoder", KILN_C99_BUILTIN_TEXT_DECODER},
                        {"eval", KILN_C99_BUILTIN_EVAL}};
    static const struct {
        const char *name;
        uint64_t code;
    } functions[] = {{"parseInt", KILN_C99_METHOD_PARSE_INT},
                     {"parseFloat", KILN_C99_METHOD_PARSE_FLOAT},
                     {"isNaN", KILN_C99_METHOD_GLOBAL_IS_NAN},
                     {"isFinite", KILN_C99_METHOD_GLOBAL_IS_FINITE},
                     {"escape", KILN_C99_METHOD_ESCAPE},
                     {"unescape", KILN_C99_METHOD_UNESCAPE},
                     {"encodeURI", KILN_C99_METHOD_ENCODE_URI},
                     {"encodeURIComponent", KILN_C99_METHOD_ENCODE_URI_COMPONENT},
                     {"decodeURI", KILN_C99_METHOD_DECODE_URI},
                     {"decodeURIComponent", KILN_C99_METHOD_DECODE_URI_COMPONENT},
                     {"queueMicrotask", KILN_C99_METHOD_QUEUE_MICROTASK},
                     {"setTimeout", KILN_C99_METHOD_SET_TIMEOUT},
                     {"setInterval", KILN_C99_METHOD_SET_INTERVAL},
                     {"clearTimeout", KILN_C99_METHOD_CLEAR_TIMEOUT},
                     {"clearInterval", KILN_C99_METHOD_CLEAR_INTERVAL},
                     {"setImmediate", KILN_C99_METHOD_SET_IMMEDIATE},
                     {"clearImmediate", KILN_C99_METHOD_CLEAR_IMMEDIATE}};
    size_t index;
    int status = kiln_c99_realm_value_slot(KILN_C99_REALM_SLOT_GLOBALS, &globals);
    if (status != KILN_C99_OK) return status;
    if (globals->tag == KILN_VALUE_OBJECT) {
        *out = *globals;
        return KILN_C99_OK;
    }
    status = kiln_c99_new_object(globals);
    if (status != KILN_C99_OK) {
        return status;
    }
    for (index = 0; index < sizeof constructors / sizeof *constructors && status == KILN_C99_OK;
         ++index) {
        status = kiln_c99_put(NULL, *globals, constructors[index].name,
                              strlen(constructors[index].name),
                              kiln_c99_builtin(constructors[index].builtin), 0, NULL);
    }
    for (index = 0; index < sizeof functions / sizeof *functions && status == KILN_C99_OK;
         ++index) {
        status = kiln_c99_put(NULL, *globals, functions[index].name,
                              strlen(functions[index].name),
                              kiln_c99_method(functions[index].code), 0, NULL);
    }
    /* The two the realm builds rather than names: each is a real object with its own slot, so
     * `globalThis.console === console` and a second read finds the same one. */
    if (status == KILN_C99_OK) {
        kiln_value built;
        status = kiln_c99_atomics_object(&built);
        if (status == KILN_C99_OK) {
            status = kiln_c99_put(NULL, *globals, "Atomics", 7, built, 0, NULL);
        }
    }
    if (status == KILN_C99_OK) {
        kiln_value built;
        status = kiln_c99_console_object(&built);
        if (status == KILN_C99_OK) {
            status = kiln_c99_put(NULL, *globals, "console", 7, built, 0, NULL);
        }
    }
    /* The three value properties of the global object are the only ones the specification locks
     * on every axis: `{ writable: false, enumerable: false, configurable: false }`. An ordinary
     * put would leave them configurable, and `delete NaN` would then answer `true` here and
     * `false` in the interpreter -- the same bytecode, two answers. */
    {
        static const struct {
            const char *name;
            size_t length;
            double value;
            int is_undefined;
        } locked[] = {
            {"NaN", 3, 0.0, 0},
            {"Infinity", 8, 0.0, 0},
            {"undefined", 9, 0.0, 1},
        };
        double values[3];
        values[0] = 0.0 / 0.0;
        values[1] = 1.0 / 0.0;
        values[2] = 0.0;
        for (index = 0; index < sizeof locked / sizeof *locked && status == KILN_C99_OK; ++index) {
            kiln_property *slot;
            status = kiln_c99_own_slot(*globals, locked[index].name, locked[index].length, &slot);
            if (status != KILN_C99_OK) {
                break;
            }
            slot->accessor = 0;
            slot->value = locked[index].is_undefined ? kiln_c99_undefined()
                                                     : kiln_c99_number(values[index]);
            slot->enumerable = 0;
            slot->writable = 0;
            slot->configurable = 0;
            kiln_c99_object_of(*globals)->read_only_seen = 1;
        }
    }
    /* It holds itself under both of its names, which is how a program reaches it without a
     * keyword: `global.Object === Object` is the standard test for "am I in a realm that has
     * one". */
    if (status == KILN_C99_OK) {
        status = kiln_c99_put(NULL, *globals, "globalThis", 10, *globals, 0, NULL);
    }
    if (status == KILN_C99_OK) {
        status = kiln_c99_put(NULL, *globals, "global", 6, *globals, 0, NULL);
    }
    if (status != KILN_C99_OK) {
        return status;
    }
    {
        kiln_object *object = kiln_c99_object_of(*globals);
        size_t slot;
        for (slot = 0; slot < object->count; ++slot) {
            object->properties[slot].enumerable = 0;
        }
    }
    *out = *globals;
    return KILN_C99_OK;
}

/* One binding of the realm's global object, for a name the compiler could not place.
 *
 * `found` stays 0 for a name that is genuinely absent, which is the caller's cue to throw. Only
 * *own* properties count: the global object is the last place identifier resolution looks, and
 * inheriting `toString` from `Object.prototype` would make a bare `toString` resolve. */
static int kiln_c99_global_binding(kiln_value *out, const char *name, size_t name_len, int *found) {
    kiln_value globals;
    kiln_object *object;
    size_t slot;
    int status = kiln_c99_global_object(&globals);
    *found = 0;
    if (status != KILN_C99_OK) {
        return status;
    }
    object = kiln_c99_object_of(globals);
    for (slot = 0; slot < object->count; ++slot) {
        if (object->properties[slot].key_len == name_len &&
            memcmp(object->properties[slot].key, name, name_len) == 0) {
            *out = object->properties[slot].value;
            *found = 1;
            return KILN_C99_OK;
        }
    }
    return KILN_C99_OK;
}

/* `CreateGlobalVarBinding` / `CreateGlobalFunctionBinding`: declare a name on the global object,
 * leaving an existing property alone.
 *
 * Global code's variable environment *is* the global object, so a `var` at the top of a script is
 * a property of it. Creating rather than assigning is the whole point: `var x;` over an `x` the
 * realm already has keeps the value it had. `configurable` is the specification's `D`, false for
 * a script and true for an `eval`. */
static int kiln_c99_declare_global(const char *name, size_t name_len, int configurable) {
    kiln_value globals;
    kiln_value existing;
    kiln_property *slot;
    int found = 0;
    int status = kiln_c99_global_binding(&existing, name, name_len, &found);
    if (status != KILN_C99_OK || found) {
        return status;
    }
    status = kiln_c99_global_object(&globals);
    if (status != KILN_C99_OK) {
        return status;
    }
    status = kiln_c99_own_slot(globals, name, name_len, &slot);
    if (status != KILN_C99_OK) {
        return status;
    }
    slot->accessor = 0;
    slot->value = kiln_c99_undefined();
    slot->enumerable = 1;
    slot->writable = 1;
    slot->configurable = configurable;
    return KILN_C99_OK;
}

/* A BigInt written out in `radix`.
 *
 * The provider owns the digits, so this asks twice: once for the length and once for the bytes.
 * A negative answer is the length it wanted, which is what makes one retry enough. */
static kiln_value kiln_c99_bigint_text(kiln_value value, uint32_t radix) {
    const void *handle = (const void *)(uintptr_t)value.bits;
    char inline_buffer[64];
    int32_t written = kiln_bigint_to_string(handle, radix, (unsigned char *)inline_buffer,
                                            sizeof inline_buffer);
    if (written >= 0) {
        return kiln_c99_copy_text(inline_buffer, (size_t)written);
    }
    {
        size_t needed = (size_t)(-(int64_t)written);
        char *wide = (char *)malloc(needed == 0 ? 1 : needed);
        kiln_value text;
        if (wide == NULL) {
            return kiln_c99_undefined();
        }
        written = kiln_bigint_to_string(handle, radix, (unsigned char *)wide, needed);
        if (written < 0) {
            free(wide);
            return kiln_c99_undefined();
        }
        text = kiln_c99_copy_text(wide, (size_t)written);
        free(wide);
        return text;
    }
}

/* A BigInt literal, parsed on first use and kept.
 *
 * The handle is shared by every load of the same literal, which is sound because a BigInt is
 * immutable: the provider's arithmetic always allocates its answer rather than writing back. */
static int kiln_c99_bigint_constant(void **cache, const char *digits, size_t len,
                                    kiln_value *out) {
    if (*cache == NULL) {
        void *handle = NULL;
        if (kiln_bigint_parse((const unsigned char *)digits, len, 10, &handle) != 0) {
            KILN_C99_REFUSE("a BigInt literal the provider would not parse");
        }
        *cache = handle;
    }
    *out = kiln_c99_bigint_value(*cache);
    return KILN_C99_OK;
}

/* `1n`, built once. Nothing frees a BigInt handle here, so a shared one is safe to keep. */
static const void *kiln_c99_bigint_one(void) {
    static void *one = NULL;
    if (one == NULL) {
        static const unsigned char digit[] = {'1'};
        if (kiln_bigint_parse(digit, sizeof digit, 10, &one) != 0) {
            return NULL;
        }
    }
    return one;
}

/* Wrap a handle the provider just produced. */
static kiln_value kiln_c99_bigint_value(void *handle) {
    return kiln_c99_value(KILN_VALUE_BIGINT, (uint64_t)(uintptr_t)handle, NULL, 0);
}

/* The `Temporal` handle, and the readback that keeps every crossing scalar.
 *
 * The engine is the same `kiln-temporal` the interpreter links (ADR 0025), so the two cannot
 * disagree about a calendar. What is here is the object model around it, which stays in C for the
 * reason the regexp match result does: it is the part that is already written once. */
static kiln_value kiln_c99_temporal_value(void *handle) {
    return kiln_c99_value(KILN_VALUE_TEMPORAL, (uint64_t)(uintptr_t)handle, NULL, 0);
}

static void *kiln_c99_temporal_of(kiln_value value) {
    return (void *)(uintptr_t)value.bits;
}

/* Turn a provider failure into the error the engine chose.
 *
 * The class travels in the return code rather than being guessed from the message: `Temporal`
 * distinguishes RangeError from TypeError from SyntaxError on nearly every path, and Test262
 * checks the distinction, so a caller that collapsed them would be wrong in a way no probe that
 * only prints values would notice. */
static int kiln_c99_temporal_raise(kiln_value *thrown, int32_t code) {
    char inline_buffer[192];
    int32_t class_code = 0;
    int32_t written = kiln_temporal_last_error(&class_code, (unsigned char *)inline_buffer,
                                               sizeof inline_buffer - 1);
    const char *message = "Temporal rejected this operation";
    uint64_t kind = KILN_C99_BUILTIN_RANGE_ERROR;
    char *spilled = NULL;
    if (written < 0) {
        /* The one negative answer says exactly how much it wanted, so a second call always fits. */
        size_t wanted = (size_t)(-written);
        spilled = (char *)malloc(wanted + 1);
        if (spilled != NULL) {
            written = kiln_temporal_last_error(&class_code, (unsigned char *)spilled, wanted);
            if (written >= 0) {
                spilled[written] = '\0';
                message = spilled;
            }
        }
    } else if (written > 0) {
        inline_buffer[written] = '\0';
        message = inline_buffer;
    }
    /* -1 is a violated argument contract rather than a rejection the program can see, and it has
     * no message of its own; the classes below are the engine's own classification. */
    if (class_code == -3) {
        kind = KILN_C99_BUILTIN_TYPE_ERROR;
    } else if (class_code == -4) {
        kind = KILN_C99_BUILTIN_SYNTAX_ERROR;
    }
    (void)code;
    {
        int status = kiln_c99_throw_error(thrown, kind, message);
        free(spilled);
        return status;
    }
}

/* A value's own ISO text, under the provider's one-retry length protocol. */
static int kiln_c99_temporal_string(kiln_value *out, kiln_value value, kiln_value *thrown) {
    char inline_buffer[128];
    void *handle = kiln_c99_temporal_of(value);
    int32_t written =
        kiln_temporal_to_string(handle, (unsigned char *)inline_buffer, sizeof inline_buffer);
    if (written >= 0) {
        *out = kiln_c99_copy_text(inline_buffer, (size_t)written);
        return KILN_C99_OK;
    }
    {
        size_t wanted = (size_t)(-written);
        char *spilled = (char *)malloc(wanted);
        int status;
        if (spilled == NULL) {
            return kiln_c99_temporal_raise(thrown, written);
        }
        written = kiln_temporal_to_string(handle, (unsigned char *)spilled, wanted);
        if (written < 0) {
            free(spilled);
            return kiln_c99_temporal_raise(thrown, written);
        }
        *out = kiln_c99_copy_text(spilled, (size_t)written);
        status = KILN_C99_OK;
        free(spilled);
        return status;
    }
}

/* Which of the eight types a constructor builtin stands for, or -1 if it is not one. */
static int32_t kiln_c99_temporal_kind_of_builtin(uint64_t builtin) {
    if (builtin < KILN_C99_BUILTIN_TEMPORAL_PLAIN_DATE ||
        builtin > KILN_C99_BUILTIN_TEMPORAL_ZONED_DATE_TIME) {
        return -1;
    }
    return (int32_t)(builtin - KILN_C99_BUILTIN_TEMPORAL_PLAIN_DATE);
}

/* The constructor builtin for a live handle, which is what `constructor` and the tag both want. */
static uint64_t kiln_c99_temporal_builtin_of(kiln_value value) {
    int32_t kind = kiln_temporal_kind(kiln_c99_temporal_of(value));
    if (kind < 0) {
        return KILN_C99_BUILTIN_TEMPORAL;
    }
    return KILN_C99_BUILTIN_TEMPORAL_PLAIN_DATE + (uint64_t)kind;
}

/* `Symbol.toStringTag`, which every one of the eight carries and which decides how it prints. */
static const char *kiln_c99_temporal_tag(kiln_value value) {
    switch (kiln_c99_temporal_builtin_of(value)) {
    case KILN_C99_BUILTIN_TEMPORAL_PLAIN_DATE: return "Temporal.PlainDate";
    case KILN_C99_BUILTIN_TEMPORAL_PLAIN_TIME: return "Temporal.PlainTime";
    case KILN_C99_BUILTIN_TEMPORAL_PLAIN_DATE_TIME: return "Temporal.PlainDateTime";
    case KILN_C99_BUILTIN_TEMPORAL_PLAIN_YEAR_MONTH: return "Temporal.PlainYearMonth";
    case KILN_C99_BUILTIN_TEMPORAL_PLAIN_MONTH_DAY: return "Temporal.PlainMonthDay";
    case KILN_C99_BUILTIN_TEMPORAL_DURATION: return "Temporal.Duration";
    case KILN_C99_BUILTIN_TEMPORAL_INSTANT: return "Temporal.Instant";
    case KILN_C99_BUILTIN_TEMPORAL_ZONED_DATE_TIME: return "Temporal.ZonedDateTime";
    default: return "Temporal";
    }
}

/* The name a `Temporal` constructor answers to, for `.name` and for a refusal that says which. */
static const char *kiln_c99_temporal_builtin_name(uint64_t builtin) {
    switch (builtin) {
    case KILN_C99_BUILTIN_TEMPORAL: return "Temporal";
    case KILN_C99_BUILTIN_TEMPORAL_NOW: return "Temporal.Now";
    case KILN_C99_BUILTIN_TEMPORAL_PLAIN_DATE: return "Temporal.PlainDate";
    case KILN_C99_BUILTIN_TEMPORAL_PLAIN_TIME: return "Temporal.PlainTime";
    case KILN_C99_BUILTIN_TEMPORAL_PLAIN_DATE_TIME: return "Temporal.PlainDateTime";
    case KILN_C99_BUILTIN_TEMPORAL_PLAIN_YEAR_MONTH: return "Temporal.PlainYearMonth";
    case KILN_C99_BUILTIN_TEMPORAL_PLAIN_MONTH_DAY: return "Temporal.PlainMonthDay";
    case KILN_C99_BUILTIN_TEMPORAL_DURATION: return "Temporal.Duration";
    case KILN_C99_BUILTIN_TEMPORAL_INSTANT: return "Temporal.Instant";
    case KILN_C99_BUILTIN_TEMPORAL_ZONED_DATE_TIME: return "Temporal.ZonedDateTime";
    default: return NULL;
    }
}

/* A member that is there in the language and not here yet.
 *
 * It refuses *by name* rather than reading as `undefined`, because an undefined member turns into
 * "x is not a function" three frames away, and the whole point of the two-engine arrangement is
 * that a gap announces itself as a gap. */
static int kiln_c99_temporal_unsupported(kiln_value *thrown, const char *owner, const char *member,
                                         size_t member_len) {
    char message[192];
    size_t at = 0;
    const char *prefix = "unsupported Temporal member: ";
    while (*prefix != '\0' && at + 1 < sizeof message) {
        message[at++] = *prefix++;
    }
    while (*owner != '\0' && at + 1 < sizeof message) {
        message[at++] = *owner++;
    }
    if (at + 1 < sizeof message) {
        message[at++] = '.';
    }
    {
        size_t index;
        for (index = 0; index < member_len && at + 1 < sizeof message; ++index) {
            message[at++] = member[index];
        }
    }
    message[at] = '\0';
    return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR, message);
}

static kiln_regexp *kiln_c99_regexp_of(kiln_value value) {
    return (kiln_regexp *)(uintptr_t)value.bits;
}

/* Compile a pattern, or report that the engine rejected it.
 *
 * A rejection is a SyntaxError rather than a refusal, because that is what `new RegExp` with the
 * same text produces -- and a literal the frontend parsed can still be one the engine will not
 * take, so the program has to be able to catch it. */
static int kiln_c99_new_regexp(kiln_value *out, const char *source, size_t source_len,
                               const char *flags, size_t flags_len, kiln_value *thrown) {
    kiln_regexp *regexp;
    void *handle = NULL;
    size_t index;
    if (kiln_regexp_compile((const unsigned char *)source, source_len,
                            (const unsigned char *)flags, flags_len, &handle) != 0) {
        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_SYNTAX_ERROR,
                                    "Invalid regular expression");
    }
    regexp = (kiln_regexp *)kiln_c99_gc_alloc(KILN_GC_REGEXP, sizeof *regexp);
    if (regexp == NULL) {
        kiln_regexp_free(handle);
        return KILN_C99_OUT_OF_MEMORY;
    }
    regexp->handle = handle;
    regexp->source = source;
    regexp->source_len = source_len;
    regexp->flags = flags;
    regexp->flags_len = flags_len;
    regexp->global = 0;
    regexp->sticky = 0;
    regexp->last_index = 0.0;
    for (index = 0; index < flags_len; ++index) {
        if (flags[index] == 'g') {
            regexp->global = 1;
        } else if (flags[index] == 'y') {
            regexp->sticky = 1;
        }
    }
    *out = kiln_c99_value(KILN_VALUE_REGEXP, (uint64_t)(uintptr_t)regexp, NULL, 0);
    return KILN_C99_OK;
}

/* One search, and the match it found.
 *
 * Returns 1 for a match, 0 for none. A sticky pattern must match exactly at the cursor, not merely
 * at or after it -- the engine reports the first match from `start`, and that check is what turns
 * it into the stickier promise. */
static int kiln_c99_regexp_find(kiln_regexp *regexp, const uint16_t *units, size_t count,
                                size_t start, int64_t *ranges, size_t capacity, size_t *groups,
                                int *matched) {
    *matched = 0;
    *groups = 0;
    if (start > count) {
        return KILN_C99_OK;
    }
    if (kiln_regexp_search(regexp->handle, units, count, start, ranges, capacity) != 1) {
        return KILN_C99_OK;
    }
    if (regexp->sticky && ranges[0] != (int64_t)start) {
        return KILN_C99_OK;
    }
    *groups = kiln_regexp_group_count(regexp->handle);
    if (*groups > capacity) {
        *groups = capacity;
    }
    *matched = 1;
    return KILN_C99_OK;
}

static kiln_symbol *kiln_c99_symbol_of(kiln_value value) {
    return (kiln_symbol *)(uintptr_t)value.ptr;
}

/* The specification's own symbols. One instance each, so equality is the pointer's exactly as it
 * is for a unique symbol -- there is no second kind of comparison to keep in step. */
static const kiln_symbol kiln_c99_well_known[] = {
    {"@@iterator", 10, "Symbol.iterator", 15},
    {"@@asyncIterator", 15, "Symbol.asyncIterator", 20},
    {"@@hasInstance", 13, "Symbol.hasInstance", 18},
    {"@@toStringTag", 13, "Symbol.toStringTag", 18},
    {"@@toPrimitive", 13, "Symbol.toPrimitive", 18},
    {"@@isConcatSpreadable", 20, "Symbol.isConcatSpreadable", 25},
    {"@@match", 7, "Symbol.match", 12},
    {"@@matchAll", 10, "Symbol.matchAll", 15},
    {"@@replace", 9, "Symbol.replace", 14},
    {"@@search", 8, "Symbol.search", 13},
    {"@@species", 9, "Symbol.species", 14},
    {"@@split", 7, "Symbol.split", 12},
    {"@@unscopables", 13, "Symbol.unscopables", 18}};

/* Every symbol this run has made, by the value that *is* it.
 *
 * A symbol-keyed property stores only the reserved key, so nothing on the object can hand the
 * symbol back -- and `Reflect.ownKeys` has to answer with the same symbol, by identity, not a
 * fresh one carrying the same description. The whole `kiln_value` is kept rather than the
 * `kiln_symbol *` inside it, because `Symbol.for` and `Symbol()` fill `bits` differently and
 * equality reads what was stored.
 *
 * Held for the life of the process, because the last reference to a symbol can legitimately be
 * the property key it is stored under: `o[Symbol("k")] = 1` keeps nothing else, and the key must
 * still name a symbol afterwards. */
static kiln_value *kiln_c99_symbols;
static size_t kiln_c99_symbol_count;
static size_t kiln_c99_symbol_capacity;

static int kiln_c99_remember_symbol(kiln_value symbol) {
    if (kiln_c99_symbol_count == kiln_c99_symbol_capacity) {
        size_t wanted = kiln_c99_symbol_capacity == 0 ? 8 : kiln_c99_symbol_capacity * 2;
        kiln_value *grown =
            (kiln_value *)realloc(kiln_c99_symbols, wanted * sizeof *kiln_c99_symbols);
        if (grown == NULL) {
            return KILN_C99_OUT_OF_MEMORY;
        }
        kiln_c99_symbols = grown;
        kiln_c99_symbol_capacity = wanted;
    }
    kiln_c99_symbols[kiln_c99_symbol_count++] = symbol;
    return KILN_C99_OK;
}

/* The symbol a reserved store key names, reporting whether the key names one at all.
 *
 * Both halves are the same question. A symbol key and an internal slot are both `@@` strings
 * here -- `@@iterator` beside `@@error`, `@@symbol:3` beside `@@iterpart` -- and reflection has
 * to report the first kind and must never report the second. This is the one place that line is
 * drawn on this side. */
static int kiln_c99_symbol_of_key(const char *key, size_t key_len, kiln_value *out) {
    size_t index;
    size_t known = sizeof kiln_c99_well_known / sizeof *kiln_c99_well_known;
    for (index = 0; index < known; ++index) {
        if (kiln_c99_well_known[index].key_len == key_len &&
            memcmp(kiln_c99_well_known[index].key, key, key_len) == 0) {
            *out = kiln_c99_value(KILN_VALUE_SYMBOL,
                                  (uint64_t)(uintptr_t)&kiln_c99_well_known[index],
                                  (const char *)(uintptr_t)&kiln_c99_well_known[index], 0);
            return 1;
        }
    }
    for (index = 0; index < kiln_c99_symbol_count; ++index) {
        const kiln_symbol *symbol = kiln_c99_symbol_of(kiln_c99_symbols[index]);
        if (symbol->key_len == key_len && memcmp(symbol->key, key, key_len) == 0) {
            *out = kiln_c99_symbols[index];
            return 1;
        }
    }
    return 0;
}

/* The string a value names a property under.
 *
 * Distinct from `kiln_c99_text`: a symbol keys a property under its reserved slot, not under its
 * description, and conflating the two would make `obj[Symbol.iterator]` and
 * `obj["Symbol(Symbol.iterator)"]` the same property. */
static kiln_value kiln_c99_property_key(kiln_value key) {
    if (key.tag == KILN_VALUE_SYMBOL) {
        kiln_symbol *symbol = kiln_c99_symbol_of(key);
        return kiln_c99_value(KILN_VALUE_STRING, 0, symbol->key, symbol->key_len);
    }
    return kiln_c99_text(key);
}

/* `Symbol(description)`. Two calls make two different symbols even given the same description,
 * so the identity is the allocation's and the reserved key carries a serial. */
static int kiln_c99_new_symbol(kiln_value *out, kiln_value description) {
    static uint64_t next_serial;
    kiln_symbol *symbol = (kiln_symbol *)kiln_c99_gc_alloc(KILN_GC_SYMBOL, sizeof *symbol);
    kiln_text key = {NULL, 0, 0};
    char digits[24];
    int written;
    if (symbol == NULL) {
        return KILN_C99_OUT_OF_MEMORY;
    }
    next_serial += 1;
    written = kiln_number_to_js_string((double)next_serial, digits, sizeof digits);
    if (written < 0 || kiln_c99_text_append_c(&key, "@@symbol:") != KILN_C99_OK ||
        kiln_c99_text_append(&key, digits, (size_t)written) != KILN_C99_OK) {
        free(key.bytes);
        kiln_c99_gc_release(symbol);
        return KILN_C99_OUT_OF_MEMORY;
    }
    /* The reserved key stays malloc-backed and is never released: a symbol lives as long as the
     * realm does (`kiln_c99_symbols` roots every one of them), and the key bytes are borrowed by
     * every property stored under this symbol. The tracer still marks both `key` and
     * `description` as candidate blocks, so a description that came from a collected string --
     * `Symbol(String(x))` -- is kept alive by the symbol that carries it. */
    symbol->key = key.bytes;
    symbol->key_len = key.len;
    /* Registered as immortal so a property stored under the symbol carries these very bytes
     * rather than a copy; the malloc above is the reason `intern_static` is the right form. */
    {
        const char *canonical = kiln_c99_intern_static(symbol->key, symbol->key_len);
        if (canonical != NULL) symbol->key = canonical;
    }
    if (description.tag == KILN_VALUE_UNDEFINED) {
        /* `Symbol()` has no description, which prints as `Symbol()` rather than
         * `Symbol(undefined)`. */
        symbol->description = NULL;
        symbol->description_len = 0;
    } else {
        kiln_value text = kiln_c99_text(description);
        if (text.tag != KILN_VALUE_STRING) {
            free(key.bytes);
            kiln_c99_gc_release(symbol);
            return KILN_C99_OUT_OF_MEMORY;
        }
        symbol->description = text.ptr;
        symbol->description_len = text.len;
    }
    symbol->registry_id = 0;
    *out = kiln_c99_value(KILN_VALUE_SYMBOL, 0, (const char *)(uintptr_t)symbol, 0);
    return kiln_c99_remember_symbol(*out);
}

static int kiln_c99_symbol_for(kiln_value *out, kiln_value key) {
    kiln_value text = kiln_c99_text(key);
    kiln_symbol *symbol;
    const uint8_t *registered_key;
    uint64_t registered_key_len;
    char *property_key;
    static const char property_prefix[] = "@@symbol-for:";
    uint64_t id;
    if (text.tag != KILN_VALUE_STRING) return KILN_C99_OUT_OF_MEMORY;
    if (kiln_realm_symbol_for((const uint8_t *)text.ptr, (uint64_t)text.len, &id) != KILN_C99_OK)
        KILN_C99_REFUSE("the realm would not register this symbol key");
    if (kiln_realm_symbol_key_for(id, &registered_key, &registered_key_len) != KILN_C99_OK ||
        registered_key_len > SIZE_MAX)
        KILN_C99_REFUSE("the realm would not answer with the symbol key it registered");
    symbol = (kiln_symbol *)kiln_c99_gc_alloc(KILN_GC_SYMBOL, sizeof *symbol);
    if (symbol == NULL) return KILN_C99_OUT_OF_MEMORY;
    /* Registry key bytes, like `Symbol()`'s, stay malloc-backed and outlive the realm's heap:
     * the symbol they belong to is rooted for the realm's whole life. */
    property_key = (char *)malloc((sizeof property_prefix - 1) + sizeof id);
    if (property_key == NULL) {
        kiln_c99_gc_release(symbol);
        return KILN_C99_OUT_OF_MEMORY;
    }
    memcpy(property_key, property_prefix, sizeof property_prefix - 1);
    memcpy(property_key + sizeof property_prefix - 1, &id, sizeof id);
    symbol->key = property_key;
    symbol->key_len = (sizeof property_prefix - 1) + sizeof id;
    /* Two `Symbol.for("x")` calls make two of these structs with one key: interning is what
     * makes a property stored through either findable through the other by pointer. */
    {
        const char *canonical = kiln_c99_intern_static(symbol->key, symbol->key_len);
        if (canonical != NULL) symbol->key = canonical;
    }
    symbol->description = (const char *)registered_key;
    symbol->description_len = (size_t)registered_key_len;
    symbol->registry_id = id;
    *out = kiln_c99_value(KILN_VALUE_SYMBOL, 0, (const char *)(uintptr_t)symbol, 0);
    return kiln_c99_remember_symbol(*out);
}

static kiln_value kiln_c99_symbol_key_for(kiln_value value) {
    const uint8_t *key;
    uint64_t key_len;
    if (value.tag != KILN_VALUE_SYMBOL) return kiln_c99_undefined();
    if (kiln_realm_symbol_key_for(kiln_c99_symbol_of(value)->registry_id, &key, &key_len) !=
        KILN_C99_OK)
        return kiln_c99_undefined();
    return kiln_c99_copy_text((const char *)key, (size_t)key_len);
}

/* `call`, `apply`, `bind` and `toString`, which every callable answers to whatever kind of
 * callable it is. Not a range lookup, because the four are not contiguous with anything and a
 * function's *own* property of the same name has to win -- so each caller asks after its own
 * store.
 *
 * `toString` here is `Function.prototype.toString`, which is why it belongs beside the other
 * three rather than in the `Object.prototype` fallback below -- that one would answer
 * `[object Function]` for `String(Object)`, and refusing was rightly preferred to saying that.
 * This answers `function Object() { [native code] }`, which is what the interpreter says and what
 * node says. Without it a program reading `f.toString()` found no property at all and got
 * `undefined is not a function`: a wrong answer where `String(f)` on the very same value had been
 * right all along, because that path goes through `kiln_c99_text` instead. */
static uint64_t kiln_c99_callable_method(const char *key, size_t key_len) {
    if (kiln_c99_same_name(key, key_len, "call")) {
        return KILN_C99_METHOD_FUNCTION_CALL;
    }
    if (kiln_c99_same_name(key, key_len, "apply")) {
        return KILN_C99_METHOD_FUNCTION_APPLY;
    }
    if (kiln_c99_same_name(key, key_len, "bind")) {
        return KILN_C99_METHOD_FUNCTION_BIND;
    }
    if (kiln_c99_same_name(key, key_len, "toString")) {
        return KILN_C99_METHOD_FUNCTION_TO_STRING;
    }
    return 0;
}

/* A namespace is an object holding functions rather than a callable, which decides both how it
 * prints and what `typeof` answers about it. */
static int kiln_c99_is_namespace(uint64_t builtin) {
    /* `Temporal` and `Temporal.Now` hold functions rather than being callable; the eight types are
     * constructors and are not namespaces. */
    if (builtin == KILN_C99_BUILTIN_TEMPORAL || builtin == KILN_C99_BUILTIN_TEMPORAL_NOW) {
        return 1;
    }
    /* `Intl` is a namespace this backend does not carry: `typeof Intl === "object"` is what a
     * library feature-detects on, and it has to be the same word here as in the interpreter even
     * though every member behind it refuses. */
    if (builtin == KILN_C99_BUILTIN_INTL) {
        return 1;
    }
    /* `performance` is a `Performance` *object* holding `now`, not something a program calls --
     * node answers `typeof performance === "object"` and gives it no `name`. It was a callable
     * here, so `typeof performance` read "function" and `performance.name` read "performance",
     * which is an implementation detail leaking through a property a program reads to tell one
     * function from another. `require("node:perf_hooks").performance` is this same value, so the
     * two spellings are one object as they are in node. */
    if (builtin == KILN_C99_BUILTIN_PERFORMANCE) {
        return 1;
    }
    return builtin == KILN_C99_BUILTIN_MATH || builtin == KILN_C99_BUILTIN_JSON ||
           builtin == KILN_C99_BUILTIN_REFLECT ||
           builtin == KILN_C99_BUILTIN_OBJECT_PROTOTYPE ||
           builtin == KILN_C99_BUILTIN_ARRAY_PROTOTYPE ||
           builtin == KILN_C99_BUILTIN_ERROR_PROTOTYPE ||
           builtin == KILN_C99_BUILTIN_AGGREGATE_ERROR_PROTOTYPE ||
           builtin == KILN_C99_BUILTIN_MAP_PROTOTYPE ||
           builtin == KILN_C99_BUILTIN_SET_PROTOTYPE ||
           builtin == KILN_C99_BUILTIN_WEAK_MAP_PROTOTYPE ||
           builtin == KILN_C99_BUILTIN_WEAK_SET_PROTOTYPE ||
           builtin == KILN_C99_BUILTIN_STRING_PROTOTYPE ||
           builtin == KILN_C99_BUILTIN_ITERATOR_PROTOTYPE ||
           builtin == KILN_C99_BUILTIN_REGEXP_PROTOTYPE ||
           builtin == KILN_C99_BUILTIN_DATE_PROTOTYPE ||
           builtin == KILN_C99_BUILTIN_NUMBER_PROTOTYPE ||
           builtin == KILN_C99_BUILTIN_FUNCTION_PROTOTYPE ||
           builtin == KILN_C99_BUILTIN_PROMISE_PROTOTYPE ||
           builtin == KILN_C99_BUILTIN_TYPED_ARRAY_PROTOTYPE ||
           /* `DataView.prototype` is a table, and therefore a namespace here; `DataView` itself
            * is a constructor and is not, so `typeof DataView === "function"` stays true. */
           builtin == KILN_C99_BUILTIN_DATA_VIEW_PROTOTYPE ||
           (builtin >= KILN_C99_BUILTIN_INT8_ARRAY_PROTOTYPE &&
            builtin <= KILN_C99_BUILTIN_FLOAT64_ARRAY_PROTOTYPE);
}

/* The method a name resolves to on a given receiver, or 0.  One generated code list means a read
 * and a call cannot disagree about which names exist, and category membership is independent of
 * numeric adjacency. */
static const uint16_t kiln_c99_method_codes[] = {