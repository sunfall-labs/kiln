
/* Generated graph launchers provide the completion hook when module promises need to publish
 * their namespace. Keeping the symbol weak lets ordinary realms use this runtime file without a
 * graph launcher. */
__attribute__((weak)) int kiln_graph_module_completion(kiln_host *host, uint32_t module_index,
                                                       int rejected, const kiln_value *value,
                                                       kiln_value *result, int depth,
                                                       kiln_value *thrown) {
    (void)host;
    (void)module_index;
    (void)rejected;
    (void)value;
    (void)result;
    (void)depth;
    (void)thrown;
    return kiln_c99_internal_status("generated module completion callback");
}

static int kiln_c99_module_completion_body(kiln_call_context *ctx, kiln_val *result,
                                           kiln_val *upvalues, const kiln_val *receiver,
                                           const kiln_val *args, size_t argc) {
    kiln_host *host = ctx ? ctx->host : NULL;
    kiln_value *thrown = ctx ? ctx->thrown : NULL;
    int depth = ctx ? ctx->depth : 0;
    kiln_value value = argc > 0 ? kiln_val_to_boundary(args[0]) : kiln_c99_undefined();
    uint32_t module_index = (uint32_t)kiln_c99_as_number(kiln_val_to_boundary(upvalues[0]));
    int rejected = (int)kiln_c99_as_number(kiln_val_to_boundary(upvalues[1]));
    kiln_value boundary_result = kiln_c99_undefined();
    int status = kiln_graph_module_completion(host, module_index, rejected, &value,
                                              &boundary_result, depth, thrown);
    *result = kiln_val_from_boundary(boundary_result);
    return status;
}

int kiln_c99_module_completion(kiln_host *host, kiln_value input_promise,
                               uint32_t module_index, kiln_value *out_promise,
                               int depth, kiln_value *thrown) {
    kiln_val captured[2];
    kiln_value fulfilled;
    kiln_value rejected;
    kiln_value handlers[2];
    const kiln_string *closure_name;
    int status;
    if (out_promise == NULL || input_promise.tag != KILN_VALUE_PROMISE)
        KILN_C99_REFUSE_INTERNAL("module completion requires a promise output and input");
    captured[0] = kiln_val_from_boundary(kiln_c99_number((double)module_index));
    captured[1] = kiln_val_from_boundary(kiln_c99_number(0.0));
    closure_name = kiln_c99_intern_static("module completion", 17);
    if (closure_name == NULL) return KILN_C99_OUT_OF_MEMORY;
    status = kiln_c99_make_closure(&fulfilled, kiln_c99_module_completion_body,
                                   closure_name, "", 0, "module completion",
                                   0, 1, 0, 0, 0, 0, 0, 0, KILN_VAL_UNDEFINED,
                                   KILN_VAL_UNDEFINED, 0, 16, 0, 0, captured, 2);
    if (status != KILN_C99_OK) return status;
    kiln_c99_gc_keep_alive(&fulfilled);
    kiln_c99_gc_keep_alive(&input_promise);
    captured[1] = kiln_val_from_boundary(kiln_c99_number(1.0));
    status = kiln_c99_make_closure(&rejected, kiln_c99_module_completion_body,
                                   closure_name, "", 0, "module completion",
                                   0, 1, 0, 0, 0, 0, 0, 0, KILN_VAL_UNDEFINED,
                                   KILN_VAL_UNDEFINED, 0, 16, 0, 0, captured, 2);
    if (status != KILN_C99_OK) return status;
    kiln_c99_gc_keep_alive(&fulfilled);
    kiln_c99_gc_keep_alive(&rejected);
    kiln_c99_gc_keep_alive(&input_promise);
    handlers[0] = fulfilled;
    handlers[1] = rejected;
    status = kiln_c99_invoke_method(host, out_promise, KILN_C99_METHOD_THEN,
                                    input_promise, handlers, 2, depth, thrown);
    kiln_c99_gc_keep_alive(&input_promise);
    kiln_c99_gc_keep_alive(&handlers);
    kiln_c99_gc_keep_alive(&fulfilled);
    kiln_c99_gc_keep_alive(&rejected);
    return status;
}

typedef struct kiln_dyn_binding {
    char *name;
    size_t name_len;
    kiln_value value;
    /* Constants/imports always reject writes. A named function-expression binding ignores a
     * sloppy write but throws in strict code. */
    unsigned char restriction;
} kiln_dyn_binding;

enum {
    KILN_DYN_BINDING_MUTABLE = 0,
    KILN_DYN_BINDING_READ_ONLY = 1,
    KILN_DYN_BINDING_FUNCTION_NAME = 2,
    KILN_DYN_BINDING_PARAMETER = 3,
    KILN_DYN_BINDING_ANNEX_B_VAR = 4,
    KILN_DYN_BINDING_LEXICAL = 5
};

typedef struct kiln_dyn_scope {
    kiln_dyn_binding *bindings;
    size_t count;
    size_t capacity;
    struct kiln_dyn_scope *parent;
    kiln_value with_object;
    int has_with_object;
    int strict;
    /* Captured lexical bindings stay in their serialized environment as live cells. A function
     * call scope borrows that environment instead of rebuilding a malloc-backed binding table. */
    kiln_value captured_environment;
    int has_captured_environment;
    /* Ordered lexical/with records inherited by a closure. The flat environment above is kept
     * as the lexical-cell index; this list preserves ObjectEnvironmentRecord position/identity. */
    kiln_value captured_records;
    int has_captured_records;
    /* Bindings live in a malloc-grown index for fast name lookup. Mirror each live value in a
     * GC object reachable from this stack scope: the collector scans C stacks, not arbitrary
     * malloc allocations. */
    kiln_value roots;
    /* Switch cases share one lexical scope; var declarations still target its parent. */
    struct kiln_dyn_scope *var_target;
    const char **function_declaration_sites;
    size_t function_declaration_count;
    size_t function_declaration_capacity;
    const char **annex_b_declaration_sites;
    size_t annex_b_declaration_count;
    size_t annex_b_declaration_capacity;
} kiln_dyn_scope;

typedef struct {
    const char *name;
    size_t name_len;
    size_t scope_id;
    int is_const;
} kiln_dyn_lexical_declaration;

static int kiln_dyn_scope_predeclare_lexicals(kiln_dyn_scope *scope,
                                               const char *source, size_t source_len,
                                               size_t target_scope_id);
static int kiln_dyn_source_may_have_function_declaration(const char *source, size_t source_len);
static int kiln_dyn_private_environment_has(kiln_value environment, const char *name,
                                           size_t name_len, int *found);

static int kiln_dyn_has_line_terminator(const char *source, size_t start, size_t end) {
    for (size_t index = start; index < end; ++index) {
        unsigned char byte = (unsigned char)source[index];
        if (byte == '\n' || byte == '\r') return 1;
        if (byte == 0xE2 && index + 2 < end &&
            (unsigned char)source[index + 1] == 0x80 &&
            ((unsigned char)source[index + 2] == 0xA8 ||
             (unsigned char)source[index + 2] == 0xA9))
            return 1;
    }
    return 0;
}

static void kiln_dyn_scope_init(kiln_dyn_scope *scope, kiln_dyn_scope *parent) {
    scope->bindings = NULL;
    scope->count = 0;
    scope->capacity = 0;
    scope->parent = parent;
    scope->with_object = kiln_c99_undefined();
    scope->has_with_object = 0;
    scope->strict = parent != NULL ? parent->strict : 0;
    scope->captured_environment = kiln_c99_undefined();
    scope->has_captured_environment = 0;
    scope->captured_records = kiln_c99_undefined();
    scope->has_captured_records = 0;
    scope->roots = kiln_c99_undefined();
    scope->var_target = NULL;
    scope->function_declaration_sites = NULL;
    scope->function_declaration_count = 0;
    scope->function_declaration_capacity = 0;
    scope->annex_b_declaration_sites = NULL;
    scope->annex_b_declaration_count = 0;
    scope->annex_b_declaration_capacity = 0;
}

static int kiln_dyn_scope_set(kiln_dyn_scope *scope, const char *name, size_t name_len, kiln_value val) {
    size_t i;
    for (i = 0; i < scope->count; ++i) {
        if (scope->bindings[i].name_len == name_len &&
            memcmp(scope->bindings[i].name, name, name_len) == 0) {
            if (scope->bindings[i].value.tag == KILN_VALUE_CELL) {
                kiln_c99_val_store_cell(kiln_c99_cell_of(scope->bindings[i].value),
                                        kiln_val_from_boundary(val));
            } else {
                if (kiln_c99_put(NULL, scope->roots, name, name_len, val, 0, NULL) != KILN_C99_OK)
                    return 0;
                scope->bindings[i].value = val;
            }
            return 1;
        }
    }
    if (scope->count >= scope->capacity) {
        size_t new_cap = scope->capacity == 0 ? 8 : scope->capacity * 2;
        kiln_dyn_binding *new_b = (kiln_dyn_binding *)realloc(scope->bindings, new_cap * sizeof *new_b);
        if (new_b == NULL) return 0;
        scope->bindings = new_b;
        scope->capacity = new_cap;
    }
    if (scope->roots.tag == KILN_VALUE_UNDEFINED) {
        int status = kiln_c99_new_object(&scope->roots);
        if (status == KILN_C99_OK) status = kiln_c99_set_prototype(scope->roots, kiln_c99_null());
        if (status != KILN_C99_OK) return 0;
    }
    /* Publish the value before storing its only long-lived copy in malloc-backed bindings. */
    if (kiln_c99_put(NULL, scope->roots, name, name_len, val, 0, NULL) != KILN_C99_OK) return 0;
    char *dup = (char *)malloc(name_len + 1);
    if (dup == NULL) return 0;
    memcpy(dup, name, name_len);
    dup[name_len] = '\0';
    scope->bindings[scope->count].name = dup;
    scope->bindings[scope->count].name_len = name_len;
    scope->bindings[scope->count].value = val;
    scope->bindings[scope->count].restriction = KILN_DYN_BINDING_MUTABLE;
    scope->count++;
    return 1;
}

static int kiln_dyn_scope_mark_restriction(kiln_dyn_scope *scope, const char *name,
                                           size_t name_len, unsigned char restriction) {
    for (size_t i = 0; i < scope->count; ++i) {
        if (scope->bindings[i].name_len == name_len &&
            memcmp(scope->bindings[i].name, name, name_len) == 0) {
            scope->bindings[i].restriction = restriction;
            return 1;
        }
    }
    return 0;
}

static int kiln_dyn_scope_add_function_declaration(kiln_dyn_scope *scope,
                                                    const char *site) {
    if (scope->function_declaration_count == scope->function_declaration_capacity) {
        size_t capacity = scope->function_declaration_capacity == 0
                              ? 8 : scope->function_declaration_capacity * 2;
        const char **sites = (const char **)realloc(
            scope->function_declaration_sites, capacity * sizeof *sites);
        if (sites == NULL) return 0;
        scope->function_declaration_sites = sites;
        scope->function_declaration_capacity = capacity;
    }
    scope->function_declaration_sites[scope->function_declaration_count++] = site;
    return 1;
}

static int kiln_dyn_scope_has_function_declaration(kiln_dyn_scope *scope,
                                                    const char *site) {
    for (kiln_dyn_scope *current = scope; current != NULL; current = current->parent) {
        for (size_t index = 0; index < current->function_declaration_count; ++index) {
            if (current->function_declaration_sites[index] == site) return 1;
        }
    }
    return 0;
}

static int kiln_dyn_scope_add_annex_b_declaration(kiln_dyn_scope *scope,
                                                   const char *site) {
    if (scope->annex_b_declaration_count == scope->annex_b_declaration_capacity) {
        size_t capacity = scope->annex_b_declaration_capacity == 0
                              ? 4 : scope->annex_b_declaration_capacity * 2;
        const char **sites = (const char **)realloc(
            scope->annex_b_declaration_sites, capacity * sizeof *sites);
        if (sites == NULL) return 0;
        scope->annex_b_declaration_sites = sites;
        scope->annex_b_declaration_capacity = capacity;
    }
    scope->annex_b_declaration_sites[scope->annex_b_declaration_count++] = site;
    return 1;
}

static int kiln_dyn_scope_has_annex_b_declaration(kiln_dyn_scope *scope,
                                                   const char *site) {
    for (kiln_dyn_scope *current = scope; current != NULL; current = current->parent) {
        for (size_t index = 0; index < current->annex_b_declaration_count; ++index) {
            if (current->annex_b_declaration_sites[index] == site) return 1;
        }
    }
    return 0;
}

static int kiln_dyn_scope_restriction(kiln_dyn_scope *scope, const char *name,
                                     size_t name_len, unsigned char *out,
                                     kiln_value *thrown) {
    for (kiln_dyn_scope *curr = scope; curr != NULL; curr = curr->parent) {
        for (size_t i = 0; i < curr->count; ++i) {
            if (curr->bindings[i].name_len == name_len &&
                memcmp(curr->bindings[i].name, name, name_len) == 0) {
                *out = curr->bindings[i].restriction;
                return KILN_C99_OK;
            }
        }
        if (curr->has_captured_environment) {
            int binding_present = 0;
            int binding_status = kiln_dyn_private_environment_has(curr->captured_environment,
                                                                  name, name_len,
                                                                  &binding_present);
            kiln_c99_gc_keep_alive(kiln_c99_gc_value_pointer(curr->captured_environment));
            if (binding_status != KILN_C99_OK) return binding_status;
            if (!binding_present) continue;
            char *marker = (char *)malloc(name_len + 1);
            if (marker == NULL) return KILN_C99_OUT_OF_MEMORY;
            const unsigned char candidates[] = {
                1, 2, KILN_DYN_BINDING_PARAMETER, KILN_DYN_BINDING_ANNEX_B_VAR,
                KILN_DYN_BINDING_LEXICAL
            };
            unsigned char restriction = KILN_DYN_BINDING_MUTABLE;
            for (size_t candidate = 0; candidate < sizeof candidates; ++candidate) {
                marker[0] = (char)candidates[candidate];
                memcpy(marker + 1, name, name_len);
                int present = 0;
                int status = kiln_dyn_private_environment_has(curr->captured_environment,
                                                              marker, name_len + 1,
                                                              &present);
                kiln_c99_gc_keep_alive(kiln_c99_gc_value_pointer(curr->captured_environment));
                if (status != KILN_C99_OK) {
                    free(marker);
                    return status;
                }
                if (present) {
                    restriction = candidates[candidate] == 1
                                     ? KILN_DYN_BINDING_READ_ONLY
                                     : candidates[candidate] == 2
                                           ? KILN_DYN_BINDING_FUNCTION_NAME
                                           : candidates[candidate];
                    break;
                }
            }
            free(marker);
            *out = restriction;
            return KILN_C99_OK;
        }
    }
    *out = KILN_DYN_BINDING_MUTABLE;
    return KILN_C99_OK;
}

/* Returns one when an assignment to this binding is a sloppy named-function write that must
 * evaluate its RHS but leave the immutable name binding unchanged. */
static int kiln_dyn_scope_check_write(kiln_dyn_scope *scope, const char *name,
                                      size_t name_len, int *ignore_write,
                                      kiln_value *thrown) {
    unsigned char restriction = KILN_DYN_BINDING_MUTABLE;
    int restriction_status = kiln_dyn_scope_restriction(scope, name, name_len,
                                                        &restriction, thrown);
    if (restriction_status != KILN_C99_OK) return restriction_status;
    *ignore_write = 0;
    if (restriction == KILN_DYN_BINDING_FUNCTION_NAME) {
        if (!scope->strict) {
            *ignore_write = 1;
            return KILN_C99_OK;
        }
    } else if (restriction == KILN_DYN_BINDING_MUTABLE ||
               restriction == KILN_DYN_BINDING_PARAMETER ||
               restriction == KILN_DYN_BINDING_ANNEX_B_VAR ||
               restriction == KILN_DYN_BINDING_LEXICAL) {
        return KILN_C99_OK;
    }
    return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                "Assignment to constant variable.");
}

/* The dynamic evaluator's lexer and binding table are UTF-8 text APIs.  Stored object keys are
 * canonical UTF-16/Latin1 headers, so cross that boundary through the explicit host codec rather
 * than passing the header as a byte span.  This is evaluator-local text, not property identity;
 * property lookup remains unit based in the ordinary runtime. */
static int kiln_dyn_scope_set_key(kiln_dyn_scope *scope, const kiln_string *key, kiln_value val) {
    kiln_value key_value;
    uint8_t *encoded = NULL;
    size_t encoded_len = 0;
    int status;
    int result;
    if (key == NULL) return 0;
    key_value = kiln_c99_value(KILN_VALUE_STRING, 0, (const char *)key, key->unit_len);
    status = kiln_c99_string_to_utf8_lossy(key_value, &encoded, &encoded_len);
    if (status != KILN_C99_OK) return 0;
    result = kiln_dyn_scope_set(scope, (const char *)encoded, encoded_len, val);
    free(encoded);
    return result;
}

/* Captured environments are private null-prototype ordinary objects. Check their own-name
 * membership, then read the data slot directly so Object.prototype pollution cannot become a
 * captured binding and no accessor can run while resolving a lexical name. */
static int kiln_dyn_private_environment_get(kiln_value environment, const char *name,
                                            size_t name_len, kiln_value *out,
                                            int *found) {
    *found = 0;
    if (environment.tag != KILN_VALUE_OBJECT)
        return kiln_c99_internal_status("dynamic captured environment is not an object");
    if (kiln_c99_object_of(environment)->prototype.tag != KILN_VALUE_NULL)
        return kiln_c99_internal_status("dynamic captured environment gained a prototype");
    int present = 0;
    int status = kiln_c99_has_property(NULL, environment, name, name_len, &present, 0, NULL);
    kiln_c99_gc_keep_alive(kiln_c99_gc_value_pointer(environment));
    if (status != KILN_C99_OK || !present) return status;
    kiln_property *slot = NULL;
    status = kiln_c99_own_slot(environment, name, name_len, &slot);
    kiln_c99_gc_keep_alive(kiln_c99_gc_value_pointer(environment));
    if (status != KILN_C99_OK) return status;
    if (slot == NULL || slot->accessor || slot->refused)
        return kiln_c99_internal_status("dynamic captured environment contains a non-data slot");
    *out = slot->value;
    kiln_c99_gc_keep_alive(out);
    *found = 1;
    return KILN_C99_OK;
}

static int kiln_dyn_private_environment_has(kiln_value environment, const char *name,
                                            size_t name_len, int *found) {
    kiln_value ignored = kiln_c99_undefined();
    return kiln_dyn_private_environment_get(environment, name, name_len, &ignored, found);
}

static int kiln_dyn_captured_get(kiln_dyn_scope *scope, const char *name, size_t name_len,
                                 kiln_value *out, int *found, kiln_value *thrown) {
    (void)thrown;
    *found = 0;
    if (!scope->has_captured_environment) return KILN_C99_OK;
    kiln_value stored = kiln_c99_undefined();
    int status = kiln_dyn_private_environment_get(scope->captured_environment, name, name_len,
                                                  &stored, found);
    if (status != KILN_C99_OK) return status;
    if (!*found) return KILN_C99_OK;
    if (stored.tag == KILN_VALUE_CELL) {
        kiln_val *cell = kiln_c99_cell_of(stored);
        status = kiln_c99_val_check_tdz(*cell, thrown);
        if (status != KILN_C99_OK) return status;
        *out = kiln_val_to_boundary(*cell);
    } else {
        *out = stored;
    }
    *found = 1;
    return KILN_C99_OK;
}

static int kiln_dyn_captured_update(kiln_dyn_scope *scope, const char *name, size_t name_len,
                                    kiln_value value, int *found) {
    *found = 0;
    if (!scope->has_captured_environment) return KILN_C99_OK;
    kiln_value stored = kiln_c99_undefined();
    int status = kiln_dyn_private_environment_get(scope->captured_environment, name, name_len,
                                                  &stored, found);
    if (status != KILN_C99_OK) return status;
    if (!*found) return KILN_C99_OK;
    if (stored.tag == KILN_VALUE_CELL) {
        kiln_c99_val_store_cell(kiln_c99_cell_of(stored), kiln_val_from_boundary(value));
    } else {
        status = kiln_c99_put(NULL, scope->captured_environment, name, name_len, value, 0, NULL);
        if (status != KILN_C99_OK) return status;
    }
    *found = 1;
    return KILN_C99_OK;
}

static int kiln_dyn_with_has_binding(kiln_host *host, kiln_value object, const char *name,
                                    size_t name_len, int depth, int *found,
                                    kiln_value *thrown);

static int kiln_dyn_captured_records_get(kiln_host *host, kiln_value records,
                                         const char *name, size_t name_len, int depth,
                                         int read_value, kiln_value *out, int *found, int *is_with,
                                         kiln_value *with_target, kiln_value *thrown) {
    *found = 0;
    *is_with = 0;
    if (records.tag != KILN_VALUE_ARRAY)
        return kiln_c99_internal_status("dynamic captured environment records");
    kiln_array *array = kiln_c99_array_of(records);
    for (size_t i = 0; i < array->count; ++i) {
        kiln_value entry = kiln_val_to_boundary(array->elements[i]);
        if (entry.tag != KILN_VALUE_ARRAY || kiln_c99_array_of(entry)->count != 2)
            return kiln_c99_internal_status("dynamic captured environment record entry");
        kiln_array *pair = kiln_c99_array_of(entry);
        int with_record = kiln_c99_truthy(kiln_val_to_boundary(pair->elements[0]));
        kiln_value target = kiln_val_to_boundary(pair->elements[1]);
        int present = 0;
        int status;
        if (with_record) {
            status = kiln_dyn_with_has_binding(host, target, name, name_len, depth,
                                               &present, thrown);
            kiln_c99_gc_keep_alive(&records);
            kiln_c99_gc_keep_alive(&entry);
            kiln_c99_gc_keep_alive(&target);
            if (status == KILN_C99_OK && present && read_value)
                status = kiln_c99_get(host, target, name, name_len, out, depth, thrown);
            kiln_c99_gc_keep_alive(&records);
            kiln_c99_gc_keep_alive(&entry);
            kiln_c99_gc_keep_alive(&target);
        } else {
            status = read_value
                         ? kiln_dyn_private_environment_get(target, name, name_len, out,
                                                           &present)
                         : kiln_dyn_private_environment_has(target, name, name_len, &present);
        }
        if (status != KILN_C99_OK || !present) {
            if (status != KILN_C99_OK) return status;
            continue;
        }
        if (status != KILN_C99_OK) return status;
        if (read_value && !with_record && out->tag == KILN_VALUE_CELL) {
            kiln_val *cell = kiln_c99_cell_of(*out);
            status = kiln_c99_val_check_tdz(*cell, thrown);
            if (status != KILN_C99_OK) return status;
            *out = kiln_val_to_boundary(*cell);
        }
        *found = 1;
        *is_with = with_record;
        if (with_record && with_target != NULL) *with_target = target;
        return KILN_C99_OK;
    }
    return KILN_C99_OK;
}

static int kiln_dyn_captured_records_has(kiln_host *host, kiln_value records,
                                         const char *name, size_t name_len, int depth,
                                         int *found, int *is_with, kiln_value *target_out,
                                         kiln_value *thrown) {
    *found = 0;
    *is_with = 0;
    if (records.tag != KILN_VALUE_ARRAY)
        return kiln_c99_internal_status("dynamic captured environment records");
    kiln_array *array = kiln_c99_array_of(records);
    for (size_t i = 0; i < array->count; ++i) {
        kiln_value entry = kiln_val_to_boundary(array->elements[i]);
        if (entry.tag != KILN_VALUE_ARRAY || kiln_c99_array_of(entry)->count != 2)
            return kiln_c99_internal_status("dynamic captured environment record entry");
        kiln_array *pair = kiln_c99_array_of(entry);
        int with_record = kiln_c99_truthy(kiln_val_to_boundary(pair->elements[0]));
        kiln_value target = kiln_val_to_boundary(pair->elements[1]);
        int present = 0;
        int status = with_record
                         ? kiln_dyn_with_has_binding(host, target, name, name_len, depth,
                                                     &present, thrown)
                         : kiln_dyn_private_environment_has(target, name, name_len, &present);
        kiln_c99_gc_keep_alive(&records);
        kiln_c99_gc_keep_alive(&entry);
        kiln_c99_gc_keep_alive(&target);
        if (status != KILN_C99_OK) return status;
        if (!present) continue;
        *found = 1;
        *is_with = with_record;
        if (target_out != NULL) *target_out = target;
        return KILN_C99_OK;
    }
    return KILN_C99_OK;
}

static int kiln_dyn_with_has_binding(kiln_host *host, kiln_value object, const char *name,
                                    size_t name_len, int depth, int *found,
                                    kiln_value *thrown) {
    int present = 0;
    int status = kiln_c99_has_property(host, object, name, name_len, &present, depth, thrown);
    kiln_c99_gc_keep_alive(kiln_c99_gc_value_pointer(object));
    if (status != KILN_C99_OK || !present) {
        *found = 0;
        return status;
    }
    kiln_value symbol = kiln_c99_undefined();
    status = kiln_c99_symbol_of_key("@@unscopables", 13, &symbol);
    if (status != KILN_C99_OK) return status;
    kiln_value unscopables = kiln_c99_undefined();
    status = kiln_c99_get_computed(host, object, symbol, &unscopables, depth, thrown);
    kiln_c99_gc_keep_alive(kiln_c99_gc_value_pointer(object));
    kiln_c99_gc_keep_alive(&symbol);
    if (status != KILN_C99_OK) return status;
    if (kiln_c99_is_reference(unscopables)) {
        kiln_value excluded = kiln_c99_undefined();
        status = kiln_c99_get(host, unscopables, name, name_len, &excluded, depth, thrown);
        kiln_c99_gc_keep_alive(&unscopables);
        if (status != KILN_C99_OK) return status;
        if (kiln_c99_truthy(excluded)) {
            *found = 0;
            return KILN_C99_OK;
        }
    }
    *found = 1;
    return KILN_C99_OK;
}

static int kiln_dyn_scope_update(kiln_dyn_scope *scope, const char *name, size_t name_len, kiln_value val) {
    kiln_dyn_scope *curr = scope;
    while (curr != NULL) {
        size_t i;
        for (i = 0; i < curr->count; ++i) {
            if (curr->bindings[i].name_len == name_len &&
                memcmp(curr->bindings[i].name, name, name_len) == 0) {
                if (curr->bindings[i].value.tag == KILN_VALUE_CELL) {
                    kiln_c99_val_store_cell(kiln_c99_cell_of(curr->bindings[i].value),
                                            kiln_val_from_boundary(val));
                } else {
                    if (kiln_c99_put(NULL, curr->roots, name, name_len, val, 0, NULL) != KILN_C99_OK)
                        return 0;
                    curr->bindings[i].value = val;
                }
                return 1;
            }
        }
        int found = 0;
        if (kiln_dyn_captured_update(curr, name, name_len, val, &found) != KILN_C99_OK)
            return 0;
        if (found) return 1;
        curr = curr->parent;
    }
    return 0;
}

static int kiln_dyn_scope_get(kiln_host *host, kiln_dyn_scope *scope, const char *name,
                              size_t name_len, int depth, kiln_value *out, int *found,
                              kiln_value *thrown) {
    kiln_dyn_scope *curr = scope;
    *found = 0;
    while (curr != NULL) {
        size_t i;
        for (i = 0; i < curr->count; ++i) {
            if (curr->bindings[i].name_len == name_len &&
                memcmp(curr->bindings[i].name, name, name_len) == 0) {
                if (curr->bindings[i].value.tag == KILN_VALUE_CELL) {
                    int status = kiln_c99_val_check_tdz(
                        *kiln_c99_cell_of(curr->bindings[i].value), thrown);
                    if (status != KILN_C99_OK) return status;
                    *out = kiln_val_to_boundary(*kiln_c99_cell_of(curr->bindings[i].value));
                } else {
                    *out = curr->bindings[i].value;
                }
                *found = 1;
                return KILN_C99_OK;
            }
        }
        if (curr->has_with_object) {
            int present = 0;
            int status = kiln_dyn_with_has_binding(host, curr->with_object, name, name_len,
                                                   depth, &present, thrown);
            if (status != KILN_C99_OK) return status;
            if (present) {
                status = kiln_c99_get(host, curr->with_object, name, name_len, out, depth,
                                      thrown);
                if (status == KILN_C99_OK) *found = 1;
                return status;
            }
        }
        if (curr->has_captured_records) {
            int is_with = 0;
            int status = kiln_dyn_captured_records_get(host, curr->captured_records, name,
                                                       name_len, depth, 1, out, found, &is_with,
                                                       NULL, thrown);
            if (status != KILN_C99_OK || *found) return status;
        }
        int status = kiln_dyn_captured_get(curr, name, name_len, out, found, thrown);
        if (status != KILN_C99_OK || *found) return status;
        curr = curr->parent;
    }
    return KILN_C99_OK;
}

static void kiln_dyn_scope_destroy(kiln_dyn_scope *scope) {
    size_t i;
    kiln_c99_gc_keep_alive(kiln_c99_gc_value_pointer(scope->roots));
    kiln_c99_gc_keep_alive(kiln_c99_gc_value_pointer(scope->captured_environment));
    kiln_c99_gc_keep_alive(kiln_c99_gc_value_pointer(scope->captured_records));
    for (i = 0; i < scope->count; ++i) {
        free(scope->bindings[i].name);
    }
    free(scope->bindings);
    free(scope->function_declaration_sites);
    free(scope->annex_b_declaration_sites);
}

typedef enum {
    KILN_DYN_EOF,
    KILN_DYN_NUM,
    KILN_DYN_BIGINT,
    KILN_DYN_STR,
    KILN_DYN_REGEX,
    KILN_DYN_IDENT,
    KILN_DYN_KW_RETURN,
    KILN_DYN_KW_THROW,
    KILN_DYN_KW_IF,
    KILN_DYN_KW_ELSE,
    KILN_DYN_KW_VAR,
    KILN_DYN_KW_LET,
    KILN_DYN_KW_CONST,
    KILN_DYN_KW_TRUE,
    KILN_DYN_KW_FALSE,
    KILN_DYN_KW_NULL,
    KILN_DYN_KW_UNDEFINED,
    KILN_DYN_KW_THIS,
    KILN_DYN_KW_ARGUMENTS,
    KILN_DYN_KW_TYPEOF,
    KILN_DYN_KW_DELETE,
    KILN_DYN_KW_FUNCTION,
    KILN_DYN_KW_NEW,
    KILN_DYN_KW_FOR,
    KILN_DYN_KW_IN,
    KILN_DYN_KW_INSTANCEOF,
    KILN_DYN_KW_OF,
    KILN_DYN_KW_BREAK,
    KILN_DYN_KW_CONTINUE,
    KILN_DYN_KW_WITH,
    KILN_DYN_KW_ASYNC,
    KILN_DYN_KW_SWITCH,
    KILN_DYN_KW_CASE,
    KILN_DYN_KW_DEFAULT,
    KILN_DYN_ARROW,
    KILN_DYN_PLUS_PLUS,
    KILN_DYN_MINUS_MINUS,
    KILN_DYN_PLUS,
    KILN_DYN_PLUS_ASSIGN,
    KILN_DYN_MINUS,
    KILN_DYN_MINUS_ASSIGN,
    KILN_DYN_STAR,
    KILN_DYN_STAR_ASSIGN,
    KILN_DYN_SLASH,
    KILN_DYN_SLASH_ASSIGN,
    KILN_DYN_PERCENT,
    KILN_DYN_PERCENT_ASSIGN,
    KILN_DYN_LSHIFT,
    KILN_DYN_RSHIFT,
    KILN_DYN_URSHIFT,
    KILN_DYN_LSHIFT_ASSIGN,
    KILN_DYN_RSHIFT_ASSIGN,
    KILN_DYN_URSHIFT_ASSIGN,
    KILN_DYN_BIT_AND,
    KILN_DYN_BIT_OR,
    KILN_DYN_BIT_XOR,
    KILN_DYN_BIT_AND_ASSIGN,
    KILN_DYN_BIT_OR_ASSIGN,
    KILN_DYN_BIT_XOR_ASSIGN,
    KILN_DYN_BIT_NOT,
    KILN_DYN_ASSIGN,
    KILN_DYN_EQ,
    KILN_DYN_STRICT_EQ,
    KILN_DYN_NE,
    KILN_DYN_STRICT_NE,
    KILN_DYN_LT,
    KILN_DYN_LE,
    KILN_DYN_GT,
    KILN_DYN_GE,
    KILN_DYN_AND,
    KILN_DYN_OR,
    KILN_DYN_NULLISH,
    KILN_DYN_NOT,
    KILN_DYN_DOT,
    KILN_DYN_ELLIPSIS,
    KILN_DYN_COMMA,
    KILN_DYN_COLON,
    KILN_DYN_QUESTION,
    KILN_DYN_SEMI,
    KILN_DYN_LPAREN,
    KILN_DYN_RPAREN,
    KILN_DYN_LBRACE,
    KILN_DYN_RBRACE,
    KILN_DYN_LBRACKET,
    KILN_DYN_RBRACKET,
    KILN_DYN_ERROR
} kiln_dyn_token_kind;

typedef struct {
    kiln_dyn_token_kind kind;
    const char *start;
    size_t len;
    double num_val;
    char *str_val;
    size_t str_len;
    /* String tokens keep their actual UTF-16 code units. `str_val` is a UTF-8 view used only
     * where this small evaluator still needs a C property-name spelling. */
    uint16_t *str_units;
    size_t str_unit_len;
} kiln_dyn_token;

typedef struct kiln_dyn_active_label {
    const char *name;
    size_t name_len;
    int iteration_target;
    struct kiln_dyn_active_label *parent;
} kiln_dyn_active_label;

typedef struct {
    const char *src;
    size_t len;
    size_t pos;
    int dynamic_function_body;
    int strict;
    /* The lexer only needs to remember whether a closing parenthesis ended a control head,
     * not the full token stack. Keep the bounded nesting state inline so cheap lexer copies
     * remain independent and no parser-owned heap allocation can escape. */
    unsigned char control_parens[128];
    size_t paren_depth;
    size_t paren_overflow;
    int closed_control_paren;
    int regex_literal_error;
    int numeric_literal_error;
    int allocation_error;
    kiln_dyn_token curr;
    kiln_dyn_token_kind prev_kind;
    kiln_dyn_active_label *active_labels;
    const char *break_label;
    size_t break_label_len;
    /* Set only by function-declaration instantiation, where a declaration's name is a mutable
     * binding in its containing function scope rather than a named-expression self binding. */
    int function_declaration;
} kiln_dyn_lexer;

static int kiln_dyn_label_matches(const kiln_dyn_active_label *labels,
                                  const char *name, size_t name_len) {
    for (; labels != NULL; labels = labels->parent) {
        if (labels->iteration_target && labels->name_len == name_len &&
            memcmp(labels->name, name, name_len) == 0)
            return 1;
    }
    return 0;
}

static void kiln_dyn_token_free(kiln_dyn_token *tok);
static void kiln_dyn_lexer_next(kiln_dyn_lexer *lex);

static int kiln_dyn_current_token_is_control_head(const kiln_dyn_lexer *lex) {
    if (lex->prev_kind == KILN_DYN_DOT) return 0;
    if (lex->curr.kind == KILN_DYN_KW_IF || lex->curr.kind == KILN_DYN_KW_FOR ||
        lex->curr.kind == KILN_DYN_KW_WITH || lex->curr.kind == KILN_DYN_KW_SWITCH)
        return 1;
    return lex->curr.kind == KILN_DYN_IDENT &&
        ((lex->curr.len == 5 &&
          (memcmp(lex->curr.start, "while", 5) == 0 ||
           memcmp(lex->curr.start, "catch", 5) == 0)));
}

static int kiln_dyn_labeled_statement_targets_iteration(const kiln_dyn_lexer *lex) {
    kiln_dyn_lexer look;
    memset(&look, 0, sizeof look);
    look.src = lex->src;
    look.len = lex->len;
    look.pos = lex->curr.start == NULL ? lex->pos
        : (size_t)(lex->curr.start - lex->src) +
              (lex->curr.kind == KILN_DYN_COLON ? lex->curr.len : 0);
    kiln_dyn_lexer_next(&look);
    for (;;) {
        if (look.curr.kind == KILN_DYN_KW_FOR ||
            (look.curr.kind == KILN_DYN_IDENT && look.curr.len == 2 &&
             memcmp(look.curr.start, "do", 2) == 0) ||
            (look.curr.kind == KILN_DYN_IDENT && look.curr.len == 5 &&
             memcmp(look.curr.start, "while", 5) == 0)) {
            kiln_dyn_token_free(&look.curr);
            return 1;
        }
        if (look.curr.kind != KILN_DYN_IDENT) break;
        kiln_dyn_lexer_next(&look);
        if (look.curr.kind != KILN_DYN_COLON) break;
        kiln_dyn_lexer_next(&look);
    }
    kiln_dyn_token_free(&look.curr);
    return 0;
}

/* Keep every dynamic parser failure useful when a Function constructor supplies opaque source.
 * The snippet is deliberately bounded and printable so diagnostics cannot dump a whole bundle. */
static int kiln_dyn_throw_parse_error(kiln_dyn_lexer *lex, kiln_value *thrown,
                                      const char *reason) {
    if (lex->allocation_error) return KILN_C99_OUT_OF_MEMORY;
    static const char function_label[] = " in dynamic Function body at byte ";
    static const char eval_label[] = " in dynamic eval source at byte ";
    static const char near_text[] = " near `";
    char message[192];
    size_t used = 0;
    size_t reason_len = strlen(reason);
    if (reason_len > 76) reason_len = 76;
    memcpy(message + used, reason, reason_len); used += reason_len;
    const char *label = lex->dynamic_function_body ? function_label : eval_label;
    size_t label_len = lex->dynamic_function_body ? sizeof function_label - 1
                                                  : sizeof eval_label - 1;
    memcpy(message + used, label, label_len); used += label_len;
    size_t offset = lex->curr.start != NULL && lex->curr.start >= lex->src
                        ? (size_t)(lex->curr.start - lex->src) : lex->pos;
    char digits[3 * sizeof(size_t) + 1];
    size_t digit_count = 0;
    do { digits[digit_count++] = (char)('0' + offset % 10); offset /= 10; }
    while (offset != 0 && digit_count < sizeof digits);
    while (digit_count != 0) message[used++] = digits[--digit_count];
    memcpy(message + used, near_text, sizeof near_text - 1); used += sizeof near_text - 1;
    const char *start = lex->curr.start != NULL ? lex->curr.start
        : lex->src + (lex->pos < lex->len ? lex->pos : lex->len);
    size_t remaining = (size_t)(lex->src + lex->len - start);
    size_t count = remaining < 32 ? remaining : 32;
    for (size_t index = 0; index < count && used + 3 < sizeof message; ++index) {
        unsigned char ch = (unsigned char)start[index];
        message[used++] = ch >= 0x20 && ch <= 0x7e && ch != '`' ? (char)ch : '.';
    }
    message[used++] = '`'; message[used] = '\0';
    return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_SYNTAX_ERROR, message);
}

static int kiln_dyn_throw_unexpected_token(kiln_dyn_lexer *lex, kiln_value *thrown) {
    return kiln_dyn_throw_parse_error(lex, thrown, "Unexpected token");
}

/* A missing identifier is much easier to diagnose when the ReferenceError retains its name.
 * Dynamic source identifiers are length-delimited UTF-8, so do not pass them to a `%s` formatter
 * or assume they are NUL-terminated. This path runs only after environment lookup has failed. */
static int kiln_dyn_throw_unresolvable_reference(kiln_value *thrown,
                                                  const char *name, size_t name_len) {
    static const char suffix[] = " is not defined";
    if (name_len > SIZE_MAX - sizeof suffix) return KILN_C99_OUT_OF_MEMORY;
    char *message = (char *)malloc(name_len + sizeof suffix);
    if (message == NULL) return KILN_C99_OUT_OF_MEMORY;
    if (name_len != 0) memcpy(message, name, name_len);
    memcpy(message + name_len, suffix, sizeof suffix);
    int status = kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_REFERENCE_ERROR, message);
    free(message);
    return status;
}

static int kiln_dyn_source_to_string(const char *source, size_t source_len, kiln_value *out);

/* The dynamic executor parses lazily while running a body, but JavaScript validates escapes
 * when the Function constructor is called. Tokenize the source once up front so malformed
 * escapes and regexp literals are reported at construction time, including in an unreachable
 * branch. Regexp grammar is delegated to the shared provider rather than duplicated here. */
static int kiln_dyn_validate_source(const char *source, size_t length, int strict,
                                    kiln_value *thrown) {
    kiln_dyn_lexer scan;
    memset(&scan, 0, sizeof(scan));
    scan.src = source;
    scan.len = length;
    scan.dynamic_function_body = 1;
    scan.strict = strict;
    kiln_dyn_lexer_next(&scan);
    while (scan.curr.kind != KILN_DYN_EOF && scan.curr.kind != KILN_DYN_ERROR) {
        if (strict && scan.curr.kind == KILN_DYN_KW_DELETE &&
            scan.prev_kind != KILN_DYN_DOT) {
            kiln_dyn_lexer look;
            memset(&look, 0, sizeof(look));
            look.src = source;
            look.len = length;
            look.pos = (size_t)(scan.curr.start - source) + scan.curr.len;
            look.dynamic_function_body = 1;
            look.strict = strict;
            look.prev_kind = KILN_DYN_KW_DELETE;
            int parens = 0;
            kiln_dyn_lexer_next(&look);
            while (look.curr.kind == KILN_DYN_LPAREN) {
                parens++;
                kiln_dyn_lexer_next(&look);
            }
            int identifier = look.curr.kind == KILN_DYN_IDENT;
            if (identifier) kiln_dyn_lexer_next(&look);
            while (parens > 0 && look.curr.kind == KILN_DYN_RPAREN) {
                parens--;
                kiln_dyn_lexer_next(&look);
            }
            int unqualified = identifier && parens == 0 &&
                look.curr.kind != KILN_DYN_DOT && look.curr.kind != KILN_DYN_LBRACKET &&
                look.curr.kind != KILN_DYN_LPAREN && look.curr.kind != KILN_DYN_LBRACE;
            kiln_dyn_token_free(&look.curr);
            if (unqualified) {
                int error_status = kiln_dyn_throw_parse_error(
                    &scan, thrown, "Delete of an unqualified identifier");
                kiln_dyn_token_free(&scan.curr);
                return error_status;
            }
        }
        if (scan.curr.kind == KILN_DYN_REGEX) {
            const char *pattern_text = scan.curr.str_val;
            size_t pattern_len = scan.curr.str_len;
            const char *flags_text = pattern_text + pattern_len + 1;
            /* The token spans both slashes and the original pattern/flags bytes. */
            size_t flags_len = scan.curr.len - pattern_len - 2;
            kiln_value pattern = kiln_c99_undefined();
            kiln_value flags = kiln_c99_undefined();
            int status = kiln_dyn_source_to_string(pattern_text, pattern_len, &pattern);
            if (status != KILN_C99_OK) {
                kiln_dyn_token_free(&scan.curr);
                return status;
            }
            kiln_c99_gc_keep_alive(&pattern);
            flags = kiln_c99_copy_text(flags_text, flags_len);
            if (flags.tag != KILN_VALUE_STRING) {
                kiln_dyn_token_free(&scan.curr);
                return KILN_C99_OUT_OF_MEMORY;
            }
            kiln_c99_gc_keep_alive(&pattern);
            kiln_c99_gc_keep_alive(&flags);
            void *handle = NULL;
            if (kiln_regexp_compile((const kiln_string *)pattern.ptr,
                                    (const kiln_string *)flags.ptr, &handle) != 0) {
                int error_status = kiln_dyn_throw_parse_error(
                    &scan, thrown, "Invalid regular expression");
                kiln_dyn_token_free(&scan.curr);
                return error_status;
            }
            kiln_regexp_free(handle);
            kiln_c99_gc_keep_alive(&pattern);
            kiln_c99_gc_keep_alive(&flags);
        }
        kiln_dyn_lexer_next(&scan);
    }
    int status = KILN_C99_OK;
    if (scan.curr.kind == KILN_DYN_ERROR) {
        if (scan.allocation_error) {
            status = KILN_C99_OUT_OF_MEMORY;
        } else {
            status = kiln_dyn_throw_parse_error(
                &scan, thrown, scan.regex_literal_error
                    ? "Invalid regular expression literal"
                    : scan.numeric_literal_error ? "Invalid numeric literal"
                                                 : "Invalid string escape");
        }
    }
    kiln_dyn_token_free(&scan.curr);
    return status;
}

static void kiln_dyn_token_free(kiln_dyn_token *tok) {
    if ((tok->kind == KILN_DYN_STR || tok->kind == KILN_DYN_REGEX ||
         tok->kind == KILN_DYN_BIGINT) && tok->str_val != NULL) {
        free(tok->str_val);
        tok->str_val = NULL;
    }
    free(tok->str_units);
    tok->str_units = NULL;
    tok->str_unit_len = 0;
}

static int kiln_dyn_is_ident_name(kiln_dyn_token_kind kind) {
    return kind == KILN_DYN_IDENT || (kind >= KILN_DYN_KW_RETURN && kind <= KILN_DYN_KW_DEFAULT);
}

static int kiln_dyn_hex_digit(unsigned char ch) {
    if (ch >= '0' && ch <= '9') return (int)(ch - '0');
    if (ch >= 'a' && ch <= 'f') return (int)(ch - 'a') + 10;
    if (ch >= 'A' && ch <= 'F') return (int)(ch - 'A') + 10;
    return -1;
}

static int kiln_dyn_radix_digit(unsigned char ch, unsigned radix) {
    int digit = kiln_dyn_hex_digit(ch);
    return digit >= 0 && (unsigned)digit < radix ? digit : -1;
}

/* Scan one decimal digit run, allowing separators only between digits. */
static int kiln_dyn_scan_digits(const char *source, size_t length, size_t *position,
                               unsigned radix, size_t *digit_count, int *has_separator) {
    size_t count = 0;
    int previous_digit = 0;
    while (*position < length) {
        unsigned char ch = (unsigned char)source[*position];
        int digit = kiln_dyn_radix_digit(ch, radix);
        if (digit >= 0) {
            count++;
            previous_digit = 1;
            (*position)++;
        } else if (ch == '_') {
            if (!previous_digit || *position + 1 >= length ||
                kiln_dyn_radix_digit((unsigned char)source[*position + 1], radix) < 0) {
                return 0;
            }
            *has_separator = 1;
            previous_digit = 0;
            (*position)++;
        } else {
            break;
        }
    }
    *digit_count = count;
    return 1;
}

static int kiln_dyn_number_suffix(unsigned char ch) {
    return (ch >= 'a' && ch <= 'z') || (ch >= 'A' && ch <= 'Z') || ch == '_' || ch == '$' ||
           ch >= 0x80;
}

/* Dynamic Function source is a JavaScript string, so a lone UTF-16 surrogate in its body must
 * survive the trip through the byte-oriented lexer. Use WTF-8 only for this private source
 * buffer: ordinary runtime strings remain canonical UTF-16 strings, and lexer string tokens are
 * rebuilt from their exact UTF-16 units. */
static int kiln_dyn_string_to_source(kiln_value value, uint8_t **out, size_t *byte_len) {
    uint16_t *units = NULL;
    size_t count = 0;
    if (out == NULL || byte_len == NULL) return KILN_C99_INVALID_ARGUMENT;
    *out = NULL;
    *byte_len = 0;
    /* Most generated source is already ASCII. Copy it directly rather than allocating an
     * intermediate UTF-16 buffer and encoding every unit on each invocation. */
    size_t ascii_len = 0;
    const char *ascii = kiln_c99_string_ascii_bytes(value, &ascii_len);
    if (ascii != NULL && kiln_c99_ascii_scan(ascii, ascii_len)) {
        if (ascii_len == SIZE_MAX) return KILN_C99_OUT_OF_MEMORY;
        uint8_t *copy = (uint8_t *)malloc(ascii_len + 1);
        if (copy == NULL) return KILN_C99_OUT_OF_MEMORY;
        if (ascii_len != 0) memcpy(copy, ascii, ascii_len);
        copy[ascii_len] = 0;
        *out = copy;
        *byte_len = ascii_len;
        return KILN_C99_OK;
    }
    int status = kiln_c99_to_units(value, &units, &count);
    if (status != KILN_C99_OK) return status;
    if (count > (SIZE_MAX - 1) / 3) {
        free(units);
        return KILN_C99_OUT_OF_MEMORY;
    }
    uint8_t *bytes = (uint8_t *)malloc(count * 3 + 1);
    if (bytes == NULL) {
        free(units);
        return KILN_C99_OUT_OF_MEMORY;
    }
    size_t used = 0;
    for (size_t index = 0; index < count; ++index) {
        uint32_t cp = units[index];
        if (cp >= 0xD800 && cp <= 0xDBFF && index + 1 < count &&
            units[index + 1] >= 0xDC00 && units[index + 1] <= 0xDFFF) {
            cp = UINT32_C(0x10000) + ((cp - 0xD800) << 10) +
                 ((uint32_t)units[++index] - 0xDC00);
        }
        if (cp < 0x80) {
            bytes[used++] = (uint8_t)cp;
        } else if (cp < 0x800) {
            bytes[used++] = (uint8_t)(0xC0u | (cp >> 6));
            bytes[used++] = (uint8_t)(0x80u | (cp & 0x3Fu));
        } else if (cp < 0x10000) {
            /* Surrogate code units are intentionally encoded as WTF-8 here. */
            bytes[used++] = (uint8_t)(0xE0u | (cp >> 12));
            bytes[used++] = (uint8_t)(0x80u | ((cp >> 6) & 0x3Fu));
            bytes[used++] = (uint8_t)(0x80u | (cp & 0x3Fu));
        } else {
            bytes[used++] = (uint8_t)(0xF0u | (cp >> 18));
            bytes[used++] = (uint8_t)(0x80u | ((cp >> 12) & 0x3Fu));
            bytes[used++] = (uint8_t)(0x80u | ((cp >> 6) & 0x3Fu));
            bytes[used++] = (uint8_t)(0x80u | (cp & 0x3Fu));
        }
    }
    bytes[used] = 0;
    free(units);
    *out = bytes;
    *byte_len = used;
    return KILN_C99_OK;
}

/* Decode the private dynamic-source WTF-8 representation. Unlike the shared UTF-8 text reader,
 * this accepts encoded surrogate code points so raw lone surrogates in quoted source become the
 * corresponding JavaScript string code unit. */
static uint32_t kiln_dyn_read_source_codepoint(const char *source, size_t length,
                                               size_t *position) {
    size_t start = *position;
    if (start >= length) return 0xFFFD;
    const unsigned char *bytes = (const unsigned char *)source;
    unsigned char first = bytes[start];
    if (first < 0x80) {
        *position = start + 1;
        return first;
    }
    size_t width;
    uint32_t cp;
    if (first >= 0xC2 && first <= 0xDF) {
        width = 2; cp = first & 0x1Fu;
    } else if (first >= 0xE0 && first <= 0xEF) {
        width = 3; cp = first & 0x0Fu;
    } else if (first >= 0xF0 && first <= 0xF4) {
        width = 4; cp = first & 0x07u;
    } else {
        *position = start + 1;
        return 0xFFFD;
    }
    if (width > length - start) {
        *position = start + 1;
        return 0xFFFD;
    }
    for (size_t offset = 1; offset < width; ++offset) {
        unsigned char byte = bytes[start + offset];
        if ((byte & 0xC0u) != 0x80u) {
            *position = start + 1;
            return 0xFFFD;
        }
        cp = (cp << 6) | (byte & 0x3Fu);
    }
    if ((width == 2 && cp < 0x80) || (width == 3 && cp < 0x800) ||
        (width == 4 && cp < 0x10000) || cp > 0x10FFFF) {
        *position = start + 1;
        return 0xFFFD;
    }
    *position = start + width;
    return cp;
}

static int kiln_dyn_read_hex(const char *source, size_t length, size_t *position,
                             size_t digits, uint32_t *value) {
    uint32_t result = 0;
    if (digits > length - *position) return 0;
    for (size_t i = 0; i < digits; ++i) {
        int digit = kiln_dyn_hex_digit((unsigned char)source[*position + i]);
        if (digit < 0) return 0;
        result = (result << 4) | (uint32_t)digit;
    }
    *position += digits;
    *value = result;
    return 1;
}

static int kiln_dyn_append_codepoint(uint16_t **units, size_t *capacity, size_t *length,
                                     uint32_t codepoint) {
    if (codepoint > UINT32_C(0x10FFFF)) return 0;
    size_t needed = *length + (codepoint > UINT32_C(0xFFFF) ? 2 : 1);
    if (needed < *length) return 0;
    if (needed > *capacity) {
        size_t next = *capacity == 0 ? 16 : *capacity;
        while (next < needed) {
            if (next > SIZE_MAX / 2) { next = needed; break; }
            next *= 2;
        }
        if (next > SIZE_MAX / sizeof(uint16_t)) return 0;
        uint16_t *grown = (uint16_t *)realloc(*units, next * sizeof(uint16_t));
        if (grown == NULL) return 0;
        *units = grown;
        *capacity = next;
    }
    if (codepoint <= UINT32_C(0xFFFF)) {
        (*units)[(*length)++] = (uint16_t)codepoint;
        return 1;
    }
    codepoint -= UINT32_C(0x10000);
    (*units)[(*length)++] = (uint16_t)(UINT32_C(0xD800) | (codepoint >> 10));
    (*units)[(*length)++] = (uint16_t)(UINT32_C(0xDC00) | (codepoint & UINT32_C(0x3FF)));
    return 1;
}

/* Source slices are kept in the private WTF-8 lexer buffer until a nested function closure is
 * created. Materialize them back into canonical UTF-16 strings here; copy_text would decode the
 * private surrogate encoding as invalid UTF-8 and replace the original code unit. */
static int kiln_dyn_source_to_string(const char *source, size_t source_len, kiln_value *out) {
    if (out == NULL || (source_len != 0 && source == NULL)) return KILN_C99_INVALID_ARGUMENT;
    if (source_len == 0 || kiln_c99_ascii_scan(source, source_len)) {
        *out = kiln_c99_copy_text(source, source_len);
        return out->tag == KILN_VALUE_STRING ? KILN_C99_OK : KILN_C99_OUT_OF_MEMORY;
    }
    if (source_len > SIZE_MAX / sizeof(uint16_t)) return KILN_C99_OUT_OF_MEMORY;
    uint16_t *units = source_len == 0 ? NULL :
        (uint16_t *)malloc(source_len * sizeof(uint16_t));
    if (source_len != 0 && units == NULL) return KILN_C99_OUT_OF_MEMORY;
    size_t capacity = source_len;
    size_t count = 0;
    size_t position = 0;
    while (position < source_len) {
        uint32_t cp = kiln_dyn_read_source_codepoint(source, source_len, &position);
        if (!kiln_dyn_append_codepoint(&units, &capacity, &count, cp)) {
            free(units);
            return KILN_C99_OUT_OF_MEMORY;
        }
    }
    *out = kiln_c99_from_units(units, count);
    free(units);
    return out->tag == KILN_VALUE_STRING ? KILN_C99_OK : KILN_C99_OUT_OF_MEMORY;
}

static char *kiln_dyn_utf8_from_units(const uint16_t *units, size_t count, size_t *byte_len) {
    if (count > (SIZE_MAX - 1) / 3) return NULL;
    size_t capacity = count * 3 + 1;
    char *bytes = (char *)malloc(capacity);
    if (bytes == NULL) return NULL;
    size_t used = 0;
    for (size_t i = 0; i < count; ++i) {
        uint32_t cp = units[i];
        if (cp >= 0xD800 && cp <= 0xDBFF && i + 1 < count &&
            units[i + 1] >= 0xDC00 && units[i + 1] <= 0xDFFF) {
            cp = UINT32_C(0x10000) + ((cp - 0xD800) << 10) + (units[++i] - 0xDC00);
        } else if (cp >= 0xD800 && cp <= 0xDFFF) {
            cp = UINT32_C(0xFFFD);
        }
        if (cp < 0x80) {
            bytes[used++] = (char)cp;
        } else if (cp < 0x800) {
            bytes[used++] = (char)(0xC0u | (cp >> 6));
            bytes[used++] = (char)(0x80u | (cp & 0x3Fu));
        } else if (cp < 0x10000) {
            bytes[used++] = (char)(0xE0u | (cp >> 12));
            bytes[used++] = (char)(0x80u | ((cp >> 6) & 0x3Fu));
            bytes[used++] = (char)(0x80u | (cp & 0x3Fu));
        } else {
            bytes[used++] = (char)(0xF0u | (cp >> 18));
            bytes[used++] = (char)(0x80u | ((cp >> 12) & 0x3Fu));
            bytes[used++] = (char)(0x80u | ((cp >> 6) & 0x3Fu));
            bytes[used++] = (char)(0x80u | (cp & 0x3Fu));
        }
    }
    bytes[used] = '\0';
    *byte_len = used;
    return bytes;
}

static void kiln_dyn_lexer_next(kiln_dyn_lexer *lex) {
    int opening_control_paren = kiln_dyn_current_token_is_control_head(lex);
    int regex_after_control = lex->closed_control_paren;
    lex->closed_control_paren = 0;
    lex->prev_kind = lex->curr.kind;
    kiln_dyn_token_free(&lex->curr);
    memset(&lex->curr, 0, sizeof(lex->curr));

    while (lex->pos < lex->len) {
        char c = lex->src[lex->pos];
        if (c == ' ' || c == '\t' || c == '\r' || c == '\n') {
            lex->pos++;
            continue;
        }
        if (c == '/' && lex->pos + 1 < lex->len && lex->src[lex->pos + 1] == '/') {
            lex->pos += 2;
            while (lex->pos < lex->len && lex->src[lex->pos] != '\n') {
                lex->pos++;
            }
            continue;
        }
        if (c == '/' && lex->pos + 1 < lex->len && lex->src[lex->pos + 1] == '*') {
            lex->pos += 2;
            while (lex->pos + 1 < lex->len && !(lex->src[lex->pos] == '*' && lex->src[lex->pos + 1] == '/')) {
                lex->pos++;
            }
            if (lex->pos + 1 < lex->len) lex->pos += 2;
            continue;
        }
        break;
    }

    if (lex->pos >= lex->len) {
        lex->curr.kind = KILN_DYN_EOF;
        return;
    }

    char c = lex->src[lex->pos];
    const char *start = &lex->src[lex->pos];

    if ((c >= '0' && c <= '9') ||
        (c == '.' && lex->pos + 1 < lex->len && lex->src[lex->pos + 1] >= '0' &&
         lex->src[lex->pos + 1] <= '9')) {
        size_t num_start = lex->pos;
        size_t digit_count = 0;
        int has_separator = 0;
        int valid = 1;
        double value = 0;
        unsigned radix = 10;
        size_t legacy_octal_start = 0;
        int legacy_octal = 0;
        size_t integer_digits = 0;
        size_t integer_end = num_start;
        int integer_has_separator = 0;
        int has_fraction_or_exponent = 0;
        int bigint_literal = 0;
        if (c == '0' && lex->pos + 1 < lex->len &&
            (lex->src[lex->pos + 1] == 'x' || lex->src[lex->pos + 1] == 'X' ||
             lex->src[lex->pos + 1] == 'b' || lex->src[lex->pos + 1] == 'B' ||
             lex->src[lex->pos + 1] == 'o' || lex->src[lex->pos + 1] == 'O')) {
            char prefix = lex->src[lex->pos + 1];
            radix = prefix == 'x' || prefix == 'X' ? 16 :
                    prefix == 'b' || prefix == 'B' ? 2 : 8;
            lex->pos += 2;
            valid = kiln_dyn_scan_digits(lex->src, lex->len, &lex->pos, radix,
                                         &digit_count, &has_separator) && digit_count != 0;
        } else {
            int leading_dot = c == '.';
            size_t fraction_digits = 0;
            if (leading_dot) {
                lex->pos++;
                has_fraction_or_exponent = 1;
                valid = kiln_dyn_scan_digits(lex->src, lex->len, &lex->pos, 10,
                                             &fraction_digits, &has_separator) &&
                        fraction_digits != 0;
            } else {
                valid = kiln_dyn_scan_digits(lex->src, lex->len, &lex->pos, 10,
                                             &integer_digits, &has_separator);
                integer_end = lex->pos;
                integer_has_separator = has_separator;
                if (num_start < lex->len && lex->src[num_start] == '0' &&
                    integer_has_separator)
                    valid = 0;
                if (valid && lex->pos < lex->len && lex->src[lex->pos] == '.') {
                    has_fraction_or_exponent = 1;
                    lex->pos++;
                    valid = kiln_dyn_scan_digits(lex->src, lex->len, &lex->pos, 10,
                                                 &fraction_digits, &has_separator);
                }
            }
            if (valid && integer_digits + fraction_digits == 0) valid = 0;
            if (valid && lex->pos < lex->len &&
                (lex->src[lex->pos] == 'e' || lex->src[lex->pos] == 'E')) {
                has_fraction_or_exponent = 1;
                lex->pos++;
                if (lex->pos < lex->len &&
                    (lex->src[lex->pos] == '+' || lex->src[lex->pos] == '-')) lex->pos++;
                size_t exponent_digits = 0;
                valid = kiln_dyn_scan_digits(lex->src, lex->len, &lex->pos, 10,
                                             &exponent_digits, &has_separator) &&
                        exponent_digits != 0;
            }
            if (valid && num_start < lex->len && lex->src[num_start] == '0' &&
                integer_digits > 1) {
                if (lex->strict || integer_has_separator || has_fraction_or_exponent) {
                    if (lex->strict || integer_has_separator) {
                        valid = 0;
                    } else {
                        int octal = 1;
                        for (size_t i = num_start; i < integer_end; ++i) {
                            if (lex->src[i] < '0' || lex->src[i] > '7') octal = 0;
                        }
                        if (octal) valid = 0;
                    }
                } else {
                    int octal = 1;
                    for (size_t i = num_start; i < integer_end; ++i) {
                        if (lex->src[i] < '0' || lex->src[i] > '7') octal = 0;
                    }
                    legacy_octal = octal;
                    legacy_octal_start = num_start;
                }
            }
        }
        if (valid && lex->pos < lex->len && lex->src[lex->pos] == 'n') {
            bigint_literal = 1;
            lex->pos++;
            if (radix == 10 &&
                (has_fraction_or_exponent ||
                 (lex->src[num_start] == '0' && integer_digits > 1))) valid = 0;
        }
        if (valid && lex->pos < lex->len) {
            unsigned char next = (unsigned char)lex->src[lex->pos];
            if (kiln_dyn_number_suffix(next) ||
                ((radix != 10 || bigint_literal) && next >= '0' && next <= '9'))
                valid = 0;
        }
        if (valid && !bigint_literal) {
            size_t length = lex->pos - num_start;
            if (legacy_octal) {
                if (length > SIZE_MAX - 3) {
                    lex->allocation_error = 1;
                    valid = 0;
                } else {
                    char *normalized = (char *)malloc(length + 3);
                    if (normalized == NULL) {
                        lex->allocation_error = 1;
                        valid = 0;
                    } else {
                        normalized[0] = '0';
                        normalized[1] = 'o';
                        memcpy(normalized + 2, lex->src + legacy_octal_start, length);
                        normalized[length + 2] = '\0';
                        value = kiln_string_to_number(normalized, length + 2);
                        free(normalized);
                    }
                }
            } else if (!has_separator) {
                value = kiln_string_to_number(start, length);
            } else {
                char *normalized = (char *)malloc(length + 1);
                if (normalized == NULL) {
                    lex->allocation_error = 1;
                    valid = 0;
                } else {
                    size_t used = 0;
                    for (size_t i = 0; i < length; ++i) {
                        if (lex->src[num_start + i] != '_')
                            normalized[used++] = lex->src[num_start + i];
                    }
                    normalized[used] = '\0';
                    value = kiln_string_to_number(normalized, used);
                    free(normalized);
                }
            }
        }
        if (!valid) {
            lex->curr.kind = KILN_DYN_ERROR;
            lex->numeric_literal_error = 1;
            lex->curr.start = start;
            lex->curr.len = lex->pos - num_start;
            return;
        }
        size_t num_len = lex->pos - num_start;
        if (bigint_literal) {
            size_t literal_len = num_len - 1;
            char *normalized = (char *)malloc(literal_len + 1);
            if (normalized == NULL) {
                lex->allocation_error = 1;
                lex->curr.kind = KILN_DYN_ERROR;
                lex->numeric_literal_error = 1;
                lex->curr.start = start;
                lex->curr.len = num_len;
                return;
            }
            size_t used = 0;
            for (size_t i = 0; i < literal_len; ++i) {
                if (lex->src[num_start + i] != '_') normalized[used++] = lex->src[num_start + i];
            }
            normalized[used] = '\0';
            lex->curr.kind = KILN_DYN_BIGINT;
            lex->curr.start = start;
            lex->curr.len = num_len;
            lex->curr.str_val = normalized;
            lex->curr.str_len = used;
            return;
        }
        lex->curr.kind = KILN_DYN_NUM;
        lex->curr.start = start;
        lex->curr.len = num_len;
        lex->curr.num_val = value;
        return;
    }

    if (c == '\'' || c == '"' || c == '`') {
        char quote = c;
        size_t literal_start = lex->pos++;
        size_t unit_capacity = 16;
        size_t unit_count = 0;
        uint16_t *units = (uint16_t *)malloc(unit_capacity * sizeof(uint16_t));
        int closed = 0;
        int valid = units != NULL;
        while (valid && lex->pos < lex->len) {
            unsigned char sc = (unsigned char)lex->src[lex->pos];
            if (sc == (unsigned char)quote) {
                lex->pos++;
                closed = 1;
                break;
            }
            if (quote != '`' && (sc == '\n' || sc == '\r')) {
                valid = 0;
                break;
            }
            uint32_t cp;
            if (sc != '\\') {
                if (quote == '`' && sc == '\r') {
                    lex->pos++;
                    if (lex->pos < lex->len && lex->src[lex->pos] == '\n') lex->pos++;
                    cp = '\n';
                } else {
                    cp = kiln_dyn_read_source_codepoint(lex->src, lex->len, &lex->pos);
                }
            } else {
                lex->pos++;
                if (lex->pos >= lex->len) {
                    valid = 0;
                    break;
                }
                unsigned char escape = (unsigned char)lex->src[lex->pos++];
                switch (escape) {
                case '\n': continue;
                case '\r':
                    if (lex->pos < lex->len && lex->src[lex->pos] == '\n') lex->pos++;
                    continue;
                case 0xE2:
                    if (lex->pos + 1 < lex->len &&
                        (unsigned char)lex->src[lex->pos] == 0x80 &&
                        ((unsigned char)lex->src[lex->pos + 1] == 0xA8 ||
                         (unsigned char)lex->src[lex->pos + 1] == 0xA9)) {
                        lex->pos += 2;
                        continue;
                    }
                    {
                        size_t back = lex->pos - 1;
                        cp = kiln_dyn_read_source_codepoint(lex->src, lex->len, &back);
                        lex->pos = back;
                    }
                    break;
                case 'b': cp = '\b'; break;
                case 'f': cp = '\f'; break;
                case 'n': cp = '\n'; break;
                case 'r': cp = '\r'; break;
                case 't': cp = '\t'; break;
                case 'v': cp = '\v'; break;
                case '0':
                    if (lex->pos < lex->len && lex->src[lex->pos] >= '0' &&
                        lex->src[lex->pos] <= '9') {
                        if (lex->strict || quote == '`') { valid = 0; break; }
                        unsigned value = 0;
                        size_t digits = 0;
                        while (digits < 2 && lex->pos < lex->len &&
                               lex->src[lex->pos] >= '0' && lex->src[lex->pos] <= '7') {
                            value = value * 8 + (unsigned)(lex->src[lex->pos++] - '0');
                            digits++;
                        }
                        cp = value;
                    } else cp = 0;
                    break;
                case '1': case '2': case '3': case '4': case '5': case '6': case '7':
                    if (lex->strict || quote == '`') { valid = 0; break; }
                    cp = (uint32_t)(escape - '0');
                    for (size_t digits = 1; digits < (escape <= '3' ? 3u : 2u) &&
                         lex->pos < lex->len && lex->src[lex->pos] >= '0' &&
                         lex->src[lex->pos] <= '7'; ++digits) {
                        cp = cp * 8 + (uint32_t)(lex->src[lex->pos++] - '0');
                    }
                    if (cp > 0xFF && lex->pos > 0) {
                        /* A three-digit legacy escape is capped at 0377; the last digit remains
                         * ordinary source when consuming it would exceed that range. */
                        lex->pos--;
                        cp >>= 3;
                    }
                    break;
                case '8': case '9':
                    if (lex->strict || quote == '`') { valid = 0; break; }
                    cp = escape;
                    break;
                case 'x':
                    if (!kiln_dyn_read_hex(lex->src, lex->len, &lex->pos, 2, &cp)) valid = 0;
                    break;
                case 'u':
                    if (lex->pos < lex->len && lex->src[lex->pos] == '{') {
                        lex->pos++;
                        cp = 0;
                        size_t digits = 0;
                        while (lex->pos < lex->len && lex->src[lex->pos] != '}') {
                            int digit = kiln_dyn_hex_digit((unsigned char)lex->src[lex->pos]);
                            if (digit < 0 || cp > ((UINT32_C(0x10FFFF) -
                                                    (uint32_t)(digit < 0 ? 0 : digit)) / 16)) {
                                valid = 0;
                                break;
                            }
                            cp = cp * 16 + (uint32_t)digit;
                            lex->pos++;
                            digits++;
                        }
                        if (valid && (digits == 0 || lex->pos >= lex->len ||
                                      lex->src[lex->pos] != '}' || cp > UINT32_C(0x10FFFF)))
                            valid = 0;
                        if (valid) lex->pos++;
                    } else if (!kiln_dyn_read_hex(lex->src, lex->len, &lex->pos, 4, &cp)) {
                        valid = 0;
                    }
                    break;
                default:
                    if (escape < 0x80) cp = escape;
                    else {
                        size_t back = lex->pos - 1;
                        cp = kiln_dyn_read_source_codepoint(lex->src, lex->len, &back);
                        lex->pos = back;
                    }
                    break;
                }
            }
            if (valid && !kiln_dyn_append_codepoint(&units, &unit_capacity, &unit_count, cp))
                valid = 0;
        }
        if (!closed) valid = 0;
        if (!valid) {
            free(units);
            lex->curr.kind = KILN_DYN_ERROR;
            lex->curr.start = start;
            lex->curr.len = lex->pos - literal_start;
            return;
        }
        size_t byte_count = 0;
        char *bytes = kiln_dyn_utf8_from_units(units, unit_count, &byte_count);
        if (bytes == NULL) {
            free(units);
            lex->curr.kind = KILN_DYN_ERROR;
            lex->curr.start = start;
            return;
        }
        lex->curr.kind = KILN_DYN_STR;
        lex->curr.start = start;
        lex->curr.len = lex->pos - literal_start;
        lex->curr.str_val = bytes;
        lex->curr.str_len = byte_count;
        lex->curr.str_units = units;
        lex->curr.str_unit_len = unit_count;
        return;
    }

    if (c == '/') {
        int can_be_regex = (regex_after_control || lex->prev_kind == KILN_DYN_EOF ||
                            lex->prev_kind == KILN_DYN_SEMI ||
                            lex->prev_kind == KILN_DYN_LBRACE ||
                            lex->prev_kind == KILN_DYN_LPAREN ||
                            lex->prev_kind == KILN_DYN_LBRACKET ||
                            lex->prev_kind == KILN_DYN_COMMA ||
                            lex->prev_kind == KILN_DYN_COLON ||
                            lex->prev_kind == KILN_DYN_QUESTION ||
                            lex->prev_kind == KILN_DYN_ASSIGN ||
                            lex->prev_kind == KILN_DYN_EQ ||
                            lex->prev_kind == KILN_DYN_STRICT_EQ ||
                            lex->prev_kind == KILN_DYN_NE ||
                            lex->prev_kind == KILN_DYN_STRICT_NE ||
                            lex->prev_kind == KILN_DYN_LT ||
                            lex->prev_kind == KILN_DYN_LE ||
                            lex->prev_kind == KILN_DYN_GT ||
                            lex->prev_kind == KILN_DYN_GE ||
                            lex->prev_kind == KILN_DYN_LSHIFT ||
                            lex->prev_kind == KILN_DYN_RSHIFT ||
                            lex->prev_kind == KILN_DYN_URSHIFT ||
                            lex->prev_kind == KILN_DYN_BIT_AND ||
                            lex->prev_kind == KILN_DYN_BIT_OR ||
                            lex->prev_kind == KILN_DYN_BIT_XOR ||
                            lex->prev_kind == KILN_DYN_LSHIFT_ASSIGN ||
                            lex->prev_kind == KILN_DYN_RSHIFT_ASSIGN ||
                            lex->prev_kind == KILN_DYN_URSHIFT_ASSIGN ||
                            lex->prev_kind == KILN_DYN_BIT_AND_ASSIGN ||
                            lex->prev_kind == KILN_DYN_BIT_OR_ASSIGN ||
                            lex->prev_kind == KILN_DYN_BIT_XOR_ASSIGN ||
                            lex->prev_kind == KILN_DYN_PLUS_ASSIGN ||
                            lex->prev_kind == KILN_DYN_MINUS_ASSIGN ||
                            lex->prev_kind == KILN_DYN_STAR_ASSIGN ||
                            lex->prev_kind == KILN_DYN_SLASH_ASSIGN ||
                            lex->prev_kind == KILN_DYN_PERCENT_ASSIGN ||
                            lex->prev_kind == KILN_DYN_BIT_NOT ||
                            lex->prev_kind == KILN_DYN_AND ||
                            lex->prev_kind == KILN_DYN_OR ||
                            lex->prev_kind == KILN_DYN_NULLISH ||
                            lex->prev_kind == KILN_DYN_NOT ||
                            lex->prev_kind == KILN_DYN_PLUS ||
                            lex->prev_kind == KILN_DYN_MINUS ||
                            lex->prev_kind == KILN_DYN_STAR ||
                            lex->prev_kind == KILN_DYN_SLASH ||
                            lex->prev_kind == KILN_DYN_PERCENT ||
                            lex->prev_kind == KILN_DYN_KW_RETURN ||
                            lex->prev_kind == KILN_DYN_KW_CASE ||
                            lex->prev_kind == KILN_DYN_KW_IN ||
                            lex->prev_kind == KILN_DYN_KW_INSTANCEOF ||
                            lex->prev_kind == KILN_DYN_KW_DELETE ||
                            lex->prev_kind == KILN_DYN_KW_TYPEOF);
        if (can_be_regex) {
            size_t reg_start = lex->pos;
            lex->pos++;
            size_t pat_start = lex->pos;
            int in_class = 0;
            int closed = 0;
            while (lex->pos < lex->len) {
                char rc = lex->src[lex->pos];
                if (rc == '\n' || rc == '\r' ||
                    ((unsigned char)rc == 0xE2 && lex->pos + 2 < lex->len &&
                     (unsigned char)lex->src[lex->pos + 1] == 0x80 &&
                     ((unsigned char)lex->src[lex->pos + 2] == 0xA8 ||
                      (unsigned char)lex->src[lex->pos + 2] == 0xA9))) {
                    lex->pos += (rc == '\n' || rc == '\r') ? 1 : 3;
                    lex->regex_literal_error = 1;
                    lex->curr.kind = KILN_DYN_ERROR;
                    lex->curr.start = &lex->src[reg_start];
                    lex->curr.len = lex->pos - reg_start;
                    return;
                }
                if (rc == '\\' && lex->pos + 1 < lex->len) {
                    size_t escaped_pos = lex->pos + 1;
                    uint32_t escaped_cp = kiln_dyn_read_source_codepoint(
                        lex->src, lex->len, &escaped_pos);
                    if (escaped_cp == '\n' || escaped_cp == '\r' ||
                        escaped_cp == 0x2028 || escaped_cp == 0x2029) {
                        lex->pos = escaped_pos;
                        lex->regex_literal_error = 1;
                        lex->curr.kind = KILN_DYN_ERROR;
                        lex->curr.start = &lex->src[reg_start];
                        lex->curr.len = lex->pos - reg_start;
                        return;
                    }
                    lex->pos = escaped_pos;
                    continue;
                }
                if (rc == '[') in_class = 1;
                else if (rc == ']') in_class = 0;
                else if (rc == '/' && !in_class) {
                    closed = 1;
                    break;
                }
                lex->pos++;
            }
            if (!closed) {
                lex->regex_literal_error = 1;
                lex->curr.kind = KILN_DYN_ERROR;
                lex->curr.start = &lex->src[reg_start];
                lex->curr.len = lex->pos - reg_start;
                return;
            }
            size_t pat_len = lex->pos - pat_start;
            lex->pos++;
            size_t flags_start = lex->pos;
            while (lex->pos < lex->len && ((lex->src[lex->pos] >= 'a' && lex->src[lex->pos] <= 'z') ||
                                           (lex->src[lex->pos] >= 'A' && lex->src[lex->pos] <= 'Z'))) {
                lex->pos++;
            }
            size_t flags_len = lex->pos - flags_start;
            if (flags_len > SIZE_MAX - 2 || pat_len > SIZE_MAX - flags_len - 2) {
                lex->allocation_error = 1;
                lex->curr.kind = KILN_DYN_ERROR;
                lex->curr.start = &lex->src[reg_start];
                lex->curr.len = lex->pos - reg_start;
                return;
            }
            char *buf = (char *)malloc(pat_len + flags_len + 2);
            if (buf == NULL) {
                lex->allocation_error = 1;
                lex->curr.kind = KILN_DYN_ERROR;
                lex->curr.start = &lex->src[reg_start];
                lex->curr.len = lex->pos - reg_start;
                return;
            }
            memcpy(buf, &lex->src[pat_start], pat_len);
            buf[pat_len] = '\0';
            memcpy(buf + pat_len + 1, &lex->src[flags_start], flags_len);
            buf[pat_len + 1 + flags_len] = '\0';

            lex->curr.kind = KILN_DYN_REGEX;
            lex->curr.start = &lex->src[reg_start];
            lex->curr.len = lex->pos - reg_start;
            lex->curr.str_val = buf;
            lex->curr.str_len = pat_len;
            return;
        }
    }

    if ((c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z') || c == '_' || c == '$') {
        size_t id_start = lex->pos;
        while (lex->pos < lex->len) {
            char ic = lex->src[lex->pos];
            if ((ic >= 'a' && ic <= 'z') || (ic >= 'A' && ic <= 'Z') || (ic >= '0' && ic <= '9') || ic == '_' || ic == '$') {
                lex->pos++;
            } else {
                break;
            }
        }
        size_t id_len = lex->pos - id_start;
        lex->curr.start = &lex->src[id_start];
        lex->curr.len = id_len;

        #define KILN_CHECK_KW(s, k) if (id_len == strlen(s) && strncmp(&lex->src[id_start], s, id_len) == 0) { lex->curr.kind = k; return; }
        KILN_CHECK_KW("return", KILN_DYN_KW_RETURN);
        KILN_CHECK_KW("throw", KILN_DYN_KW_THROW);
        KILN_CHECK_KW("if", KILN_DYN_KW_IF);
        KILN_CHECK_KW("else", KILN_DYN_KW_ELSE);
        KILN_CHECK_KW("var", KILN_DYN_KW_VAR);
        KILN_CHECK_KW("let", KILN_DYN_KW_LET);
        KILN_CHECK_KW("const", KILN_DYN_KW_CONST);
        KILN_CHECK_KW("true", KILN_DYN_KW_TRUE);
        KILN_CHECK_KW("false", KILN_DYN_KW_FALSE);
        KILN_CHECK_KW("null", KILN_DYN_KW_NULL);
        KILN_CHECK_KW("undefined", KILN_DYN_KW_UNDEFINED);
        KILN_CHECK_KW("this", KILN_DYN_KW_THIS);
        KILN_CHECK_KW("arguments", KILN_DYN_KW_ARGUMENTS);
        KILN_CHECK_KW("typeof", KILN_DYN_KW_TYPEOF);
        KILN_CHECK_KW("delete", KILN_DYN_KW_DELETE);
        KILN_CHECK_KW("function", KILN_DYN_KW_FUNCTION);
        KILN_CHECK_KW("new", KILN_DYN_KW_NEW);
        KILN_CHECK_KW("for", KILN_DYN_KW_FOR);
        KILN_CHECK_KW("in", KILN_DYN_KW_IN);
        KILN_CHECK_KW("instanceof", KILN_DYN_KW_INSTANCEOF);
        KILN_CHECK_KW("of", KILN_DYN_KW_OF);
        KILN_CHECK_KW("break", KILN_DYN_KW_BREAK);
        KILN_CHECK_KW("continue", KILN_DYN_KW_CONTINUE);
        KILN_CHECK_KW("with", KILN_DYN_KW_WITH);
        KILN_CHECK_KW("async", KILN_DYN_KW_ASYNC);
        KILN_CHECK_KW("switch", KILN_DYN_KW_SWITCH);
        KILN_CHECK_KW("case", KILN_DYN_KW_CASE);
        KILN_CHECK_KW("default", KILN_DYN_KW_DEFAULT);
        #undef KILN_CHECK_KW

        lex->curr.kind = KILN_DYN_IDENT;
        return;
    }

    lex->pos++;
    lex->curr.start = start;
    lex->curr.len = 1;

    switch (c) {
    case '=':
        if (lex->pos < lex->len && lex->src[lex->pos] == '>') {
            lex->pos++;
            lex->curr.len = 2;
            lex->curr.kind = KILN_DYN_ARROW;
            return;
        }
        if (lex->pos < lex->len && lex->src[lex->pos] == '=') {
            lex->pos++;
            if (lex->pos < lex->len && lex->src[lex->pos] == '=') {
                lex->pos++;
                lex->curr.len = 3;
                lex->curr.kind = KILN_DYN_STRICT_EQ;
                return;
            }
            lex->curr.len = 2;
            lex->curr.kind = KILN_DYN_EQ;
            return;
        }
        lex->curr.kind = KILN_DYN_ASSIGN;
        return;
    case '!':
        if (lex->pos < lex->len && lex->src[lex->pos] == '=') {
            lex->pos++;
            if (lex->pos < lex->len && lex->src[lex->pos] == '=') {
                lex->pos++;
                lex->curr.len = 3;
                lex->curr.kind = KILN_DYN_STRICT_NE;
                return;
            }
            lex->curr.len = 2;
            lex->curr.kind = KILN_DYN_NE;
            return;
        }
        lex->curr.kind = KILN_DYN_NOT;
        return;
    case '<':
        if (lex->pos < lex->len && lex->src[lex->pos] == '<') {
            lex->pos++;
            if (lex->pos < lex->len && lex->src[lex->pos] == '=') {
                lex->pos++; lex->curr.len = 3; lex->curr.kind = KILN_DYN_LSHIFT_ASSIGN;
            } else { lex->curr.len = 2; lex->curr.kind = KILN_DYN_LSHIFT; }
            return;
        }
        if (lex->pos < lex->len && lex->src[lex->pos] == '=') {
            lex->pos++;
            lex->curr.len = 2;
            lex->curr.kind = KILN_DYN_LE;
            return;
        }
        lex->curr.kind = KILN_DYN_LT;
        return;
    case '>':
        if (lex->pos < lex->len && lex->src[lex->pos] == '>') {
            lex->pos++;
            int unsigned_shift = lex->pos < lex->len && lex->src[lex->pos] == '>';
            if (unsigned_shift) lex->pos++;
            if (lex->pos < lex->len && lex->src[lex->pos] == '=') {
                lex->pos++; lex->curr.len = unsigned_shift ? 4 : 3;
                lex->curr.kind = unsigned_shift ? KILN_DYN_URSHIFT_ASSIGN : KILN_DYN_RSHIFT_ASSIGN;
            } else { lex->curr.len = unsigned_shift ? 3 : 2;
                lex->curr.kind = unsigned_shift ? KILN_DYN_URSHIFT : KILN_DYN_RSHIFT; }
            return;
        }
        if (lex->pos < lex->len && lex->src[lex->pos] == '=') {
            lex->pos++;
            lex->curr.len = 2;
            lex->curr.kind = KILN_DYN_GE;
            return;
        }
        lex->curr.kind = KILN_DYN_GT;
        return;
    case '&':
        if (lex->pos < lex->len && lex->src[lex->pos] == '&') {
            lex->pos++;
            lex->curr.len = 2;
            lex->curr.kind = KILN_DYN_AND;
            return;
        }
        if (lex->pos < lex->len && lex->src[lex->pos] == '=') {
            lex->pos++; lex->curr.len = 2; lex->curr.kind = KILN_DYN_BIT_AND_ASSIGN;
        } else lex->curr.kind = KILN_DYN_BIT_AND;
        return;
    case '|':
        if (lex->pos < lex->len && lex->src[lex->pos] == '|') {
            lex->pos++;
            lex->curr.len = 2;
            lex->curr.kind = KILN_DYN_OR;
            return;
        }
        if (lex->pos < lex->len && lex->src[lex->pos] == '=') {
            lex->pos++; lex->curr.len = 2; lex->curr.kind = KILN_DYN_BIT_OR_ASSIGN;
        } else lex->curr.kind = KILN_DYN_BIT_OR;
        return;
    case '^':
        if (lex->pos < lex->len && lex->src[lex->pos] == '=') {
            lex->pos++; lex->curr.len = 2; lex->curr.kind = KILN_DYN_BIT_XOR_ASSIGN;
        } else lex->curr.kind = KILN_DYN_BIT_XOR;
        return;
    case '~': lex->curr.kind = KILN_DYN_BIT_NOT; return;
    case '?':
        if (lex->pos < lex->len && lex->src[lex->pos] == '?') {
            lex->pos++;
            lex->curr.len = 2;
            lex->curr.kind = KILN_DYN_NULLISH;
            return;
        }
        lex->curr.kind = KILN_DYN_QUESTION;
        return;
    case '+':
        if (lex->pos < lex->len && lex->src[lex->pos] == '+') {
            lex->pos++;
            lex->curr.len = 2;
            lex->curr.kind = KILN_DYN_PLUS_PLUS;
            return;
        }
        if (lex->pos < lex->len && lex->src[lex->pos] == '=') {
            lex->pos++;
            lex->curr.len = 2;
            lex->curr.kind = KILN_DYN_PLUS_ASSIGN;
            return;
        }
        lex->curr.kind = KILN_DYN_PLUS;
        return;
    case '-':
        if (lex->pos < lex->len && lex->src[lex->pos] == '-') {
            lex->pos++;
            lex->curr.len = 2;
            lex->curr.kind = KILN_DYN_MINUS_MINUS;
            return;
        }
        if (lex->pos < lex->len && lex->src[lex->pos] == '=') {
            lex->pos++;
            lex->curr.len = 2;
            lex->curr.kind = KILN_DYN_MINUS_ASSIGN;
            return;
        }
        lex->curr.kind = KILN_DYN_MINUS;
        return;
    case '*':
        if (lex->pos < lex->len && lex->src[lex->pos] == '=') {
            lex->pos++;
            lex->curr.len = 2;
            lex->curr.kind = KILN_DYN_STAR_ASSIGN;
            return;
        }
        lex->curr.kind = KILN_DYN_STAR;
        return;
    case '/':
        if (lex->pos < lex->len && lex->src[lex->pos] == '=') {
            lex->pos++;
            lex->curr.len = 2;
            lex->curr.kind = KILN_DYN_SLASH_ASSIGN;
            return;
        }
        lex->curr.kind = KILN_DYN_SLASH;
        return;
    case '%':
        if (lex->pos < lex->len && lex->src[lex->pos] == '=') {
            lex->pos++;
            lex->curr.len = 2;
            lex->curr.kind = KILN_DYN_PERCENT_ASSIGN;
            return;
        }
        lex->curr.kind = KILN_DYN_PERCENT;
        return;
    case '.':
        if (lex->pos + 1 < lex->len && lex->src[lex->pos] == '.' &&
            lex->src[lex->pos + 1] == '.') {
            lex->pos += 2;
            lex->curr.kind = KILN_DYN_ELLIPSIS;
            lex->curr.len = 3;
        } else {
            lex->curr.kind = KILN_DYN_DOT;
        }
        return;
    case ',': lex->curr.kind = KILN_DYN_COMMA; return;
    case ':': lex->curr.kind = KILN_DYN_COLON; return;
    case ';': lex->curr.kind = KILN_DYN_SEMI; return;
    case '(': {
        if (lex->paren_overflow != 0 || lex->paren_depth == sizeof(lex->control_parens)) {
            if (lex->paren_overflow == SIZE_MAX) {
                lex->curr.kind = KILN_DYN_ERROR;
                return;
            }
            lex->paren_overflow++;
        } else {
            lex->control_parens[lex->paren_depth++] = (unsigned char)opening_control_paren;
        }
        lex->curr.kind = KILN_DYN_LPAREN;
        return;
    }
    case ')':
        if (lex->paren_overflow != 0) {
            lex->paren_overflow--;
        } else if (lex->paren_depth != 0) {
            lex->closed_control_paren = lex->control_parens[--lex->paren_depth] != 0;
        }
        lex->curr.kind = KILN_DYN_RPAREN;
        return;
    case '{': lex->curr.kind = KILN_DYN_LBRACE; return;
    case '}': lex->curr.kind = KILN_DYN_RBRACE; return;
    case '[': lex->curr.kind = KILN_DYN_LBRACKET; return;
    case ']': lex->curr.kind = KILN_DYN_RBRACKET; return;
    default:
        lex->curr.kind = KILN_DYN_ERROR;
        return;
    }
}

static int kiln_dyn_eval_expr(kiln_host *host, kiln_dyn_lexer *lex, kiln_dyn_scope *scope,
                              kiln_value receiver, const kiln_value *args, size_t argc,
                              int depth, kiln_value *out, kiln_value *thrown);
static int kiln_dyn_eval_postfix(kiln_host *host, kiln_dyn_lexer *lex, kiln_dyn_scope *scope,
                                 kiln_value receiver, const kiln_value *args, size_t argc,
                                 int depth, kiln_value *out, kiln_value *thrown,
                                 int construct_first_call);

/* This object is a private serialized lexical environment, not a JavaScript object with the
 * realm's inherited names or legacy __proto__ setter. */
static int kiln_dyn_new_environment(kiln_value *out) {
    int status = kiln_c99_new_object(out);
    if (status == KILN_C99_OK)
        status = kiln_c99_set_prototype(*out, kiln_c99_null());
    return status;
}

static int kiln_dyn_captured_record_push(kiln_value records, int with_record,
                                         kiln_value value) {
    kiln_value pair = kiln_c99_undefined();
    int status = kiln_c99_new_array(&pair);
    if (status == KILN_C99_OK) {
        status = kiln_c99_array_push(pair, kiln_c99_boolean(with_record));
        kiln_c99_gc_keep_alive(&pair);
        kiln_c99_gc_keep_alive(&records);
    }
    if (status == KILN_C99_OK) {
        status = kiln_c99_array_push(pair, value);
        kiln_c99_gc_keep_alive(&pair);
        kiln_c99_gc_keep_alive(&value);
    }
    if (status == KILN_C99_OK) {
        status = kiln_c99_array_push(records, pair);
        kiln_c99_gc_keep_alive(&records);
        kiln_c99_gc_keep_alive(&pair);
    }
    return status;
}

static int kiln_dyn_capture_environment(kiln_host *host, kiln_dyn_scope *scope,
                                       kiln_value env_obj, int depth,
                                       kiln_value *thrown) {
    /* Visit the innermost environment first. A later outer binding with the same spelling must
     * not replace the value that lexical lookup would find. */
    for (kiln_dyn_scope *s = scope; s != NULL; s = s->parent) {
        for (size_t i = 0; i < s->count; ++i) {
            int already_captured = 0;
            int status = kiln_c99_has_property(host, env_obj, s->bindings[i].name,
                                               s->bindings[i].name_len, &already_captured,
                                               depth, thrown);
            if (status != KILN_C99_OK) return status;
            if (already_captured) continue;
            if (s->bindings[i].value.tag != KILN_VALUE_CELL) {
                status = kiln_c99_make_cell(&s->bindings[i].value);
                if (status != KILN_C99_OK) return status;
                kiln_value rooted_cell = s->bindings[i].value;
                kiln_c99_gc_keep_alive(kiln_c99_gc_value_pointer(rooted_cell));
                status = kiln_c99_put(NULL, s->roots, s->bindings[i].name,
                                      s->bindings[i].name_len, rooted_cell, 0, NULL);
                if (status != KILN_C99_OK) return status;
            }
            status = kiln_c99_put(host, env_obj, s->bindings[i].name,
                                  s->bindings[i].name_len, s->bindings[i].value, 0, thrown);
            if (status != KILN_C99_OK) return status;
            if (s->bindings[i].restriction != KILN_DYN_BINDING_MUTABLE) {
                char *marker = (char *)malloc(s->bindings[i].name_len + 1);
                if (marker == NULL) return KILN_C99_OUT_OF_MEMORY;
                marker[0] = s->bindings[i].restriction == KILN_DYN_BINDING_READ_ONLY
                                ? '\001'
                                : s->bindings[i].restriction == KILN_DYN_BINDING_FUNCTION_NAME
                                    ? '\002'
                                    : (char)s->bindings[i].restriction;
                memcpy(marker + 1, s->bindings[i].name, s->bindings[i].name_len);
                status = kiln_c99_put(host, env_obj, marker, s->bindings[i].name_len + 1,
                                      kiln_c99_boolean(1), 0, thrown);
                free(marker);
                if (status != KILN_C99_OK) return status;
            }
        }
        if (s->has_captured_environment && s->captured_environment.tag == KILN_VALUE_OBJECT) {
            kiln_object *captured = kiln_c99_object_of(s->captured_environment);
            if (captured->shape != NULL) kiln_c99_ensure_dictionary(captured);
            kiln_c99_gc_keep_alive(kiln_c99_gc_value_pointer(s->captured_environment));
            size_t captured_count = captured->count;
            for (size_t i = 0; i < captured_count; ++i) {
                captured = kiln_c99_object_of(s->captured_environment);
                const kiln_string *key_header = captured->properties[i].key;
                if (key_header == NULL) continue;
                kiln_value key = kiln_c99_value(KILN_VALUE_STRING, 0,
                                                (const char *)key_header,
                                                key_header->unit_len);
                uint8_t *bytes = NULL;
                size_t byte_len = 0;
                int status = kiln_c99_string_to_utf8_lossy(key, &bytes, &byte_len);
                kiln_c99_gc_keep_alive(kiln_c99_gc_value_pointer(s->captured_environment));
                if (status != KILN_C99_OK) return status;
                if (byte_len == 1 && (bytes[0] == 3 || bytes[0] == 4)) {
                    free(bytes);
                    continue;
                }
                if (byte_len == 0 || bytes[0] == 1 || bytes[0] == 2 ||
                    bytes[0] == KILN_DYN_BINDING_PARAMETER ||
                    bytes[0] == KILN_DYN_BINDING_ANNEX_B_VAR ||
                    bytes[0] == KILN_DYN_BINDING_LEXICAL) {
                    free(bytes);
                    continue;
                }
                int already_captured = 0;
                status = kiln_c99_has_property(host, env_obj, (const char *)bytes,
                                               byte_len, &already_captured, depth, thrown);
                kiln_c99_gc_keep_alive(&env_obj);
                kiln_c99_gc_keep_alive(kiln_c99_gc_value_pointer(s->captured_environment));
                if (status != KILN_C99_OK || already_captured) {
                    free(bytes);
                    if (status != KILN_C99_OK) return status;
                    continue;
                }
                captured = kiln_c99_object_of(s->captured_environment);
                status = kiln_c99_put(host, env_obj, (const char *)bytes, byte_len,
                                      captured->properties[i].value, 0, thrown);
                kiln_c99_gc_keep_alive(&env_obj);
                kiln_c99_gc_keep_alive(kiln_c99_gc_value_pointer(s->captured_environment));
                if (status != KILN_C99_OK) {
                    free(bytes);
                    return status;
                }
                const unsigned char marker_kinds[] = {
                    1, 2, KILN_DYN_BINDING_PARAMETER, KILN_DYN_BINDING_ANNEX_B_VAR,
                    KILN_DYN_BINDING_LEXICAL
                };
                char *marker = (char *)malloc(byte_len + 1);
                if (marker == NULL) {
                    free(bytes);
                    return KILN_C99_OUT_OF_MEMORY;
                }
                memcpy(marker + 1, bytes, byte_len);
                for (size_t m = 0; m < sizeof marker_kinds; ++m) {
                    marker[0] = (char)marker_kinds[m];
                    int marker_present = 0;
                    status = kiln_c99_has_property(host, s->captured_environment, marker,
                                                   byte_len + 1, &marker_present, depth, thrown);
                    kiln_c99_gc_keep_alive(kiln_c99_gc_value_pointer(s->captured_environment));
                    if (status != KILN_C99_OK) break;
                    if (marker_present) {
                        status = kiln_c99_put(host, env_obj, marker, byte_len + 1,
                                              kiln_c99_boolean(1), 0, thrown);
                        kiln_c99_gc_keep_alive(&env_obj);
                        break;
                    }
                }
                free(marker);
                free(bytes);
                if (status != KILN_C99_OK) return status;
            }
        }
    }
    kiln_value records = kiln_c99_undefined();
    int record_status = kiln_c99_new_array(&records);
    if (record_status != KILN_C99_OK) return record_status;
    for (kiln_dyn_scope *s = scope; s != NULL; s = s->parent) {
        if (s->count > 0) {
            kiln_value lexical = kiln_c99_undefined();
            record_status = kiln_dyn_new_environment(&lexical);
            if (record_status != KILN_C99_OK) return record_status;
            kiln_c99_gc_keep_alive(&records);
            kiln_c99_gc_keep_alive(&lexical);
            for (size_t i = 0; i < s->count; ++i) {
                if (s->bindings[i].value.tag != KILN_VALUE_CELL) {
                    record_status = kiln_c99_make_cell(&s->bindings[i].value);
                    if (record_status != KILN_C99_OK) return record_status;
                    kiln_value rooted_cell = s->bindings[i].value;
                    kiln_c99_gc_keep_alive(&rooted_cell);
                    record_status = kiln_c99_put(NULL, s->roots, s->bindings[i].name,
                                                 s->bindings[i].name_len, rooted_cell, 0, NULL);
                    if (record_status != KILN_C99_OK) return record_status;
                }
                record_status = kiln_c99_put(host, lexical, s->bindings[i].name,
                                             s->bindings[i].name_len,
                                             s->bindings[i].value, 0, thrown);
                kiln_c99_gc_keep_alive(&records);
                kiln_c99_gc_keep_alive(&lexical);
                kiln_c99_gc_keep_alive(&s->bindings[i].value);
                if (record_status != KILN_C99_OK) return record_status;
                if (s->bindings[i].restriction != KILN_DYN_BINDING_MUTABLE) {
                    char *marker = (char *)malloc(s->bindings[i].name_len + 1);
                    if (marker == NULL) return KILN_C99_OUT_OF_MEMORY;
                    marker[0] = s->bindings[i].restriction == KILN_DYN_BINDING_READ_ONLY
                                    ? '\001'
                                    : s->bindings[i].restriction == KILN_DYN_BINDING_FUNCTION_NAME
                                          ? '\002'
                                          : (char)s->bindings[i].restriction;
                    memcpy(marker + 1, s->bindings[i].name, s->bindings[i].name_len);
                    record_status = kiln_c99_put(host, lexical, marker,
                                                 s->bindings[i].name_len + 1,
                                                 kiln_c99_boolean(1), 0, thrown);
                    kiln_c99_gc_keep_alive(&records);
                    kiln_c99_gc_keep_alive(&lexical);
                    free(marker);
                    if (record_status != KILN_C99_OK) return record_status;
                }
            }
            record_status = kiln_dyn_captured_record_push(records, 0, lexical);
            if (record_status != KILN_C99_OK) return record_status;
        }
        if (s->has_with_object) {
            record_status = kiln_dyn_captured_record_push(records, 1, s->with_object);
            if (record_status != KILN_C99_OK) return record_status;
        }
        if (s->has_captured_records) {
            if (s->captured_records.tag != KILN_VALUE_ARRAY)
                return kiln_c99_internal_status("dynamic captured records are not an array");
            kiln_array *inherited = kiln_c99_array_of(s->captured_records);
            for (size_t i = 0; i < inherited->count; ++i) {
                kiln_value entry = kiln_val_to_boundary(inherited->elements[i]);
                record_status = kiln_c99_array_push(records, entry);
                kiln_c99_gc_keep_alive(&records);
                kiln_c99_gc_keep_alive(kiln_c99_gc_value_pointer(s->captured_records));
                kiln_c99_gc_keep_alive(&entry);
                if (record_status != KILN_C99_OK) return record_status;
            }
        } else if (s->has_captured_environment && s->captured_environment.tag == KILN_VALUE_OBJECT) {
            record_status = kiln_dyn_captured_record_push(records, 0,
                                                           s->captured_environment);
            if (record_status != KILN_C99_OK) return record_status;
        }
    }
    const char records_marker[] = {'\004'};
    record_status = kiln_c99_put(host, env_obj, records_marker, sizeof records_marker,
                                 records, 0, thrown);
    kiln_c99_gc_keep_alive(&env_obj);
    kiln_c99_gc_keep_alive(&records);
    if (record_status != KILN_C99_OK) return record_status;
    if (scope != NULL && scope->strict) {
        const char marker[] = {'\003'};
        int status = kiln_c99_put(host, env_obj, marker, sizeof marker,
                                  kiln_c99_boolean(1), 0, thrown);
        if (status != KILN_C99_OK) return status;
    }
    return KILN_C99_OK;
}

static int kiln_dyn_make_closure(kiln_host *host, kiln_dyn_scope *scope,
                                 const char *name, size_t name_len,
                                 const char *params, size_t params_len,
                                 const char *body, size_t body_len,
                                 const char *reflection_source, size_t reflection_source_len,
                                 kiln_value receiver,
                                 int asynchronous, int arrow, int method, kiln_value *out,
                                 kiln_value *thrown);
static int kiln_dyn_capture_environment(kiln_host *host, kiln_dyn_scope *scope,
                                        kiln_value env_obj, int depth,
                                        kiln_value *thrown);
static int kiln_dyn_capture_block(kiln_dyn_lexer *lex, const char **body, size_t *body_len);
static int kiln_dyn_capture_expression(kiln_dyn_lexer *lex, const char **body,
                                       size_t *body_len, const char **syntax_end);
static int kiln_dyn_make_arrow_from_tail(kiln_host *host, kiln_dyn_lexer *lex,
                                         kiln_dyn_scope *scope, const char *params,
                                         size_t params_len, const char *source_start,
                                         kiln_value receiver, int asynchronous,
                                         int depth, kiln_value *out, kiln_value *thrown);
static int kiln_dyn_parenthesized_arrow_params(kiln_dyn_lexer *lex, const char **params,
                                               size_t *params_len, size_t *after_close);

static int kiln_c99_dynamic_body_trampoline(kiln_call_context *ctx, kiln_val *result, kiln_val *upvalues,
                                            const kiln_val *receiver_ptr, const kiln_val *args, size_t argc);

static int kiln_dyn_append_source_units(kiln_value *source, kiln_value part) {
    kiln_value joined;
    if (source == NULL) return KILN_C99_INVALID_ARGUMENT;
    kiln_c99_gc_keep_alive(source);
    kiln_c99_gc_keep_alive(&part);
    joined = kiln_c99_concat(*source, part);
    kiln_c99_gc_keep_alive(source);
    kiln_c99_gc_keep_alive(&part);
    kiln_c99_gc_keep_alive(&joined);
    if (joined.tag != KILN_VALUE_STRING) return KILN_C99_OUT_OF_MEMORY;
    *source = joined;
    return KILN_C99_OK;
}

static int kiln_dyn_function_constructor_source(kiln_value parameters, kiln_value body,
                                                kiln_value *out) {
    int status;
    if (out == NULL || parameters.tag != KILN_VALUE_STRING || body.tag != KILN_VALUE_STRING)
        return KILN_C99_INVALID_ARGUMENT;
    *out = kiln_c99_static_text("function anonymous(", sizeof("function anonymous(") - 1);
    if (out->tag != KILN_VALUE_STRING) return KILN_C99_OUT_OF_MEMORY;
    status = kiln_dyn_append_source_units(out, parameters);
    if (status != KILN_C99_OK) return status;
    status = kiln_dyn_append_source_units(
        out, kiln_c99_static_text("\n) {\n", sizeof("\n) {\n") - 1));
    if (status != KILN_C99_OK) return status;
    status = kiln_dyn_append_source_units(out, body);
    if (status != KILN_C99_OK) return status;
    return kiln_dyn_append_source_units(out,
                                        kiln_c99_static_text("\n}", sizeof("\n}") - 1));
}

static int kiln_dyn_body_has_use_strict(const char *body, size_t length) {
    size_t i = 0;
    while (i < length && (body[i] == ' ' || body[i] == '\t' || body[i] == '\r' ||
                          body[i] == '\n' || body[i] == '\f')) i++;
    if (i >= length || (body[i] != '\'' && body[i] != '"')) return 0;
    char quote = body[i++];
    static const char directive[] = "use strict";
    if (length - i < sizeof directive - 1 ||
        memcmp(body + i, directive, sizeof directive - 1) != 0) return 0;
    i += sizeof directive - 1;
    if (i < length && body[i] == quote) {
        i++;
    } else {
        return 0;
    }
    while (i < length && (body[i] == ' ' || body[i] == '\t' || body[i] == '\f')) i++;
    return i == length || body[i] == ';' || body[i] == '\r' || body[i] == '\n';
}

static int kiln_dyn_eval_primary(kiln_host *host, kiln_dyn_lexer *lex, kiln_dyn_scope *scope,
                                kiln_value receiver, const kiln_value *args, size_t argc,
                                int depth, kiln_value *out, kiln_value *thrown) {
    if (lex->curr.kind == KILN_DYN_NUM) {
        *out = kiln_c99_number(lex->curr.num_val);
    kiln_dyn_lexer_next(lex);
        return KILN_C99_OK;
    }
    if (lex->curr.kind == KILN_DYN_STR) {
        *out = kiln_c99_from_units(lex->curr.str_units, lex->curr.str_unit_len);
        kiln_dyn_lexer_next(lex);
        return KILN_C99_OK;
    }
    if (lex->curr.kind == KILN_DYN_BIGINT) {
        kiln_value source = kiln_c99_copy_text(lex->curr.str_val, lex->curr.str_len);
        if (source.tag != KILN_VALUE_STRING) {
            kiln_dyn_lexer_next(lex);
            return KILN_C99_OUT_OF_MEMORY;
        }
        kiln_c99_gc_keep_alive(&source);
        kiln_value bigint = kiln_c99_builtin(KILN_C99_BUILTIN_BIGINT);
        int status = kiln_c99_call(host, out, bigint, kiln_c99_undefined(), &source, 1,
                                   depth, thrown);
        kiln_c99_gc_keep_alive(&source);
        if (status == KILN_C99_OK) kiln_c99_gc_keep_alive(out);
        kiln_dyn_lexer_next(lex);
        return status;
    }
    if (lex->curr.kind == KILN_DYN_REGEX) {
        const char *pat = lex->curr.str_val;
        size_t pat_len = lex->curr.str_len;
        const char *flags = pat + pat_len + 1;
        size_t flags_len = strlen(flags);
        /* The lexer owns and frees this token as soon as it advances. Copy both pieces into
         * GC-owned canonical strings before handing them to the regexp, which retains their
         * headers for the lifetime of the object. Keep the values live across the regexp
         * allocation so GC stress cannot collect either header before it is retained. */
        kiln_value source;
        int reg_status = kiln_dyn_source_to_string(pat, pat_len, &source);
        if (reg_status != KILN_C99_OK) return reg_status;
        kiln_value flag_value = kiln_c99_copy_text(flags, flags_len);
        if (source.tag != KILN_VALUE_STRING || flag_value.tag != KILN_VALUE_STRING) {
            kiln_dyn_lexer_next(lex);
            return KILN_C99_OUT_OF_MEMORY;
        }
        kiln_c99_gc_keep_alive(&source);
        kiln_c99_gc_keep_alive(&flag_value);
        reg_status = kiln_c99_new_regexp(out, (const kiln_string *)source.ptr,
                                         (const kiln_string *)flag_value.ptr, thrown);
        kiln_dyn_lexer_next(lex);
        return reg_status;
    }
    if (lex->curr.kind == KILN_DYN_KW_TRUE) {
        *out = kiln_c99_boolean(1);
        kiln_dyn_lexer_next(lex);
        return KILN_C99_OK;
    }
    if (lex->curr.kind == KILN_DYN_KW_FALSE) {
        *out = kiln_c99_boolean(0);
        kiln_dyn_lexer_next(lex);
        return KILN_C99_OK;
    }
    if (lex->curr.kind == KILN_DYN_KW_NULL) {
        *out = kiln_c99_null();
        kiln_dyn_lexer_next(lex);
        return KILN_C99_OK;
    }
    if (lex->curr.kind == KILN_DYN_KW_UNDEFINED) {
        *out = kiln_c99_undefined();
        kiln_dyn_lexer_next(lex);
        return KILN_C99_OK;
    }
    if (lex->curr.kind == KILN_DYN_KW_THIS) {
        *out = receiver;
        kiln_dyn_lexer_next(lex);
        return KILN_C99_OK;
    }
    if (lex->curr.kind == KILN_DYN_KW_ARGUMENTS) {
        int status = kiln_c99_new_array(out);
        if (status != KILN_C99_OK) return status;
        for (size_t i = 0; i < argc; ++i) {
            status = kiln_c99_array_push(*out, args[i]);
            if (status != KILN_C99_OK) return status;
        }
        kiln_dyn_lexer_next(lex);
        return KILN_C99_OK;
    }
    int asynchronous = 0;
    const char *async_start = NULL;
    if (lex->curr.kind == KILN_DYN_KW_ASYNC) {
        /* `async` is contextual in expression position. It starts an async function only
         * when followed by `function`; otherwise the keyword is a valid identifier (including
         * the common `async.method` and Function-constructor parameter cases). Peek with the
         * normal lexer so comments and whitespace follow the same rules, then restore the
         * original token when this is an identifier. */
        kiln_dyn_token async_token = lex->curr;
        size_t async_end = lex->pos;
        kiln_dyn_token_kind previous = lex->prev_kind;
        kiln_dyn_lexer_next(lex);
        size_t function_start = lex->curr.start == NULL
                                    ? async_end
                                    : (size_t)(lex->curr.start - lex->src);
        if (lex->curr.kind == KILN_DYN_KW_FUNCTION &&
            !kiln_dyn_has_line_terminator(lex->src, async_end, function_start)) {
            asynchronous = 1;
            async_start = async_token.start;
        } else {
            kiln_dyn_token_free(&lex->curr);
            lex->curr = async_token;
            lex->curr.kind = KILN_DYN_IDENT;
            lex->pos = async_end;
            lex->prev_kind = previous;
        }
    }
    if (lex->curr.kind == KILN_DYN_IDENT) {
        const char *name = lex->curr.start;
        size_t name_len = lex->curr.len;
        kiln_dyn_lexer_next(lex);
        if (lex->curr.kind == KILN_DYN_ARROW) {
            const char *body;
            size_t body_len;
            kiln_dyn_lexer_next(lex);
            if (lex->curr.kind == KILN_DYN_LBRACE) {
                if (!kiln_dyn_capture_block(lex, &body, &body_len)) {
                    return kiln_dyn_throw_parse_error(lex, thrown,
                                                "Unterminated arrow function body");
                }
                const char *source_end = body + body_len + 1;
                return kiln_dyn_make_closure(host, scope, NULL, 0, name, name_len,
                                             body, body_len, name,
                                             (size_t)(source_end - name), receiver, 0, 1, 0,
                                             out, thrown);
            }
            const char *syntax_end;
            if (!kiln_dyn_capture_expression(lex, &body, &body_len, &syntax_end)) {
                if (lex->allocation_error) return KILN_C99_OUT_OF_MEMORY;
                return kiln_dyn_throw_parse_error(lex, thrown,
                                            "Expected arrow function body");
            }
            const char *source_end = syntax_end;
            size_t return_len = body_len + sizeof "return ;";
            char *return_body = (char *)malloc(return_len);
            if (return_body == NULL) return KILN_C99_OUT_OF_MEMORY;
            memcpy(return_body, "return ", 7);
            memcpy(return_body + 7, body, body_len);
            return_body[7 + body_len] = ';';
            int closure_status = kiln_dyn_make_closure(host, scope, NULL, 0,
                                                       name, name_len, return_body, body_len + 8,
                                                       name, (size_t)(source_end - name),
                                                       receiver, 0, 1, 0, out, thrown);
            free(return_body);
            return closure_status;
        }
        int scope_found = 0;
        int scope_status = kiln_dyn_scope_get(host, scope, name, name_len, depth, out,
                                              &scope_found, thrown);
        if (scope_status != KILN_C99_OK) return scope_status;
        if (scope_found) {
            return KILN_C99_OK;
        }
        kiln_value globals = kiln_c99_undefined();
        int status = kiln_c99_global_object(&globals);
        if (status == KILN_C99_OK) {
            int present = 0;
            status = kiln_c99_has_property(host, globals, name, name_len, &present, depth, thrown);
            if (status != KILN_C99_OK) return status;
            if (present) return kiln_c99_get(host, globals, name, name_len, out, depth, thrown);
        }
        return kiln_dyn_throw_unresolvable_reference(thrown, name, name_len);
    }
    if (lex->curr.kind == KILN_DYN_LPAREN) {
        const char *arrow_source = lex->curr.start;
        const char *params;
        size_t params_len, after_close;
        if (kiln_dyn_parenthesized_arrow_params(lex, &params, &params_len, &after_close)) {
            lex->pos = after_close;
            kiln_dyn_lexer_next(lex);
            if (lex->curr.kind != KILN_DYN_ARROW)
                return kiln_dyn_throw_parse_error(lex, thrown,
                                            "Expected arrow token");
            kiln_dyn_lexer_next(lex);
            return kiln_dyn_make_arrow_from_tail(host, lex, scope, params, params_len,
                                                 arrow_source, receiver, 0, depth,
                                                 out, thrown);
        }
        kiln_dyn_lexer_next(lex);
        int status = kiln_dyn_eval_expr(host, lex, scope, receiver, args, argc, depth, out, thrown);
        if (status != KILN_C99_OK) return status;
        while (lex->curr.kind == KILN_DYN_COMMA) {
            kiln_dyn_lexer_next(lex);
            status = kiln_dyn_eval_expr(host, lex, scope, receiver, args, argc, depth, out, thrown);
            if (status != KILN_C99_OK) return status;
        }
        if (lex->curr.kind == KILN_DYN_RPAREN) {
            kiln_dyn_lexer_next(lex);
            return KILN_C99_OK;
        }
        return kiln_dyn_throw_parse_error(lex, thrown, "Expected ')'");
    }
    if (lex->curr.kind == KILN_DYN_LBRACKET) {
        kiln_dyn_lexer_next(lex);
        int status = kiln_c99_new_array(out);
        if (status != KILN_C99_OK) return status;
        while (lex->curr.kind != KILN_DYN_RBRACKET && lex->curr.kind != KILN_DYN_EOF) {
            kiln_value elem;
            status = kiln_dyn_eval_expr(host, lex, scope, receiver, args, argc, depth, &elem, thrown);
            if (status != KILN_C99_OK) return status;
            status = kiln_c99_array_push(*out, elem);
            if (status != KILN_C99_OK) return status;
            if (lex->curr.kind == KILN_DYN_COMMA) {
                kiln_dyn_lexer_next(lex);
            } else {
                break;
            }
        }
        if (lex->curr.kind == KILN_DYN_RBRACKET) {
            kiln_dyn_lexer_next(lex);
            return KILN_C99_OK;
        }
        return kiln_dyn_throw_parse_error(lex, thrown, "Expected ']'");
    }
    if (lex->curr.kind == KILN_DYN_LBRACE) {
        kiln_dyn_lexer_next(lex);
        int status = kiln_c99_new_object(out);
        if (status != KILN_C99_OK) return status;
        kiln_c99_gc_keep_alive(out);
        while (lex->curr.kind != KILN_DYN_RBRACE && lex->curr.kind != KILN_DYN_EOF) {
            if (lex->curr.kind == KILN_DYN_ELLIPSIS) {
                kiln_value source;
                kiln_dyn_lexer_next(lex);
                status = kiln_dyn_eval_expr(host, lex, scope, receiver, args, argc, depth,
                                            &source, thrown);
                if (status != KILN_C99_OK) return status;
                kiln_c99_gc_keep_alive(&source);
                status = kiln_c99_spread_object(host, *out, source, depth, thrown);
                if (status != KILN_C99_OK) return status;
                if (lex->curr.kind == KILN_DYN_COMMA) {
                    kiln_dyn_lexer_next(lex);
                } else {
                    break;
                }
                continue;
            }
            const char *key_name = NULL;
            const char *method_source = lex->curr.start;
            size_t key_len = 0;
            char *owned_key = NULL;
            int string_key = 0;
            kiln_value string_key_value = kiln_c99_undefined();
            if (kiln_dyn_is_ident_name(lex->curr.kind) || lex->curr.kind == KILN_DYN_STR) {
                if (lex->curr.kind == KILN_DYN_STR) {
                    /* The lexer frees decoded string storage on its next advance. Object
                     * values and method bodies are evaluated before the key is installed, so
                     * transfer ownership of the decoded key until this property is complete. */
                    owned_key = lex->curr.str_val;
                    lex->curr.str_val = NULL;
                    key_name = owned_key;
                    string_key = 1;
                    string_key_value = kiln_c99_from_units(lex->curr.str_units,
                                                            lex->curr.str_unit_len);
                    if (string_key_value.tag != KILN_VALUE_STRING) {
                        free(owned_key);
                        return KILN_C99_OUT_OF_MEMORY;
                    }
                    kiln_c99_gc_keep_alive(&string_key_value);
                } else {
                    key_name = lex->curr.start;
                }
                key_len = lex->curr.kind == KILN_DYN_STR ? lex->curr.str_len : lex->curr.len;
                kiln_dyn_lexer_next(lex);
            } else {
                return kiln_dyn_throw_parse_error(lex, thrown, "Expected property key");
            }
            kiln_value val;
            if (lex->curr.kind == KILN_DYN_LPAREN) {
                const char *params = lex->curr.start + 1;
                kiln_dyn_lexer_next(lex);
                int nesting = 1;
                size_t params_len = 0;
                while (lex->curr.kind != KILN_DYN_EOF && nesting != 0) {
                    if (lex->curr.kind == KILN_DYN_LPAREN) nesting++;
                    else if (lex->curr.kind == KILN_DYN_RPAREN) {
                        nesting--;
                        if (nesting == 0) {
                            params_len = (size_t)(lex->curr.start - params);
                            break;
                        }
                    }
                    kiln_dyn_lexer_next(lex);
                }
                if (nesting != 0) {
                    free(owned_key);
                    return kiln_dyn_throw_parse_error(lex, thrown,
                                                      "Expected ')' after method parameters");
                }
                kiln_dyn_lexer_next(lex);
                if (lex->curr.kind != KILN_DYN_LBRACE) {
                    free(owned_key);
                    return kiln_dyn_throw_parse_error(lex, thrown,
                                                      "Expected '{' for method body");
                }
                const char *body = NULL;
                size_t body_len = 0;
                if (!kiln_dyn_capture_block(lex, &body, &body_len)) {
                    free(owned_key);
                    return kiln_dyn_throw_parse_error(lex, thrown,
                                                      "Unterminated method body");
                }
                const char *source_end = body + body_len + 1;
                status = kiln_dyn_make_closure(host, scope, key_name, key_len, params,
                                               params_len, body, body_len, method_source,
                                               (size_t)(source_end - method_source),
                                               receiver, 0, 0, 1,
                                               &val, thrown);
                if (status != KILN_C99_OK) {
                    free(owned_key);
                    return status;
                }
                kiln_c99_gc_keep_alive(&val);
                if (string_key) {
                    status = kiln_c99_set_function_name(val, string_key_value, NULL);
                    if (status != KILN_C99_OK) {
                        free(owned_key);
                        return status;
                    }
                }
            } else if (lex->curr.kind == KILN_DYN_COLON) {
                kiln_dyn_lexer_next(lex);
                status = kiln_dyn_eval_expr(host, lex, scope, receiver, args, argc, depth, &val, thrown);
                if (status != KILN_C99_OK) {
                    free(owned_key);
                    return status;
                }
            } else {
                if (string_key) {
                    free(owned_key);
                    return kiln_dyn_throw_parse_error(lex, thrown,
                                                      "String property key needs a value");
                }
                int scope_found = 0;
                status = kiln_dyn_scope_get(host, scope, key_name, key_len, depth, &val,
                                            &scope_found, thrown);
                if (status != KILN_C99_OK) {
                    free(owned_key);
                    return status;
                }
                if (!scope_found) {
                    kiln_value globals = kiln_c99_undefined();
                    int st = kiln_c99_global_object(&globals);
                    if (st != KILN_C99_OK || kiln_c99_get(host, globals, key_name, key_len,
                                                        &val, depth, thrown) != KILN_C99_OK) {
                        val = kiln_c99_undefined();
                }
            }
            int non_ascii_key = 0;
            for (size_t byte = 0; byte < key_len; ++byte) {
                if ((unsigned char)key_name[byte] >= 0x80) {
                    non_ascii_key = 1;
                    break;
                }
            }
            if (string_key) {
                status = kiln_c99_define_computed(*out, string_key_value, val);
            } else if (non_ascii_key) {
                /* Fixed C names are Latin-1 byte views; a UTF-8 source key must become an
                 * actual JS string before property-key conversion or its bytes become two
                 * unrelated code points. */
                kiln_value key_value = kiln_c99_copy_text(key_name, key_len);
                kiln_c99_gc_keep_alive(&key_value);
                status = kiln_c99_define_computed(*out, key_value, val);
            } else {
                status = kiln_c99_put(host, *out, key_name, key_len, val, 0, thrown);
            }
            free(owned_key);
            if (status != KILN_C99_OK) return status;
            if (lex->curr.kind == KILN_DYN_COMMA) {
                kiln_dyn_lexer_next(lex);
            } else {
                break;
            }
        }
        if (lex->curr.kind == KILN_DYN_RBRACE) {
            kiln_dyn_lexer_next(lex);
            return KILN_C99_OK;
        }
        return kiln_dyn_throw_parse_error(lex, thrown, "Expected '}'");
    }

    if (lex->curr.kind == KILN_DYN_KW_FUNCTION) {
        const char *function_source = asynchronous && async_start != NULL
                                          ? async_start
                                          : lex->curr.start;
        kiln_dyn_lexer_next(lex);
        const char *fn_name = "anonymous";
        size_t fn_name_len = 9;
        int explicit_name = 0;
        if (lex->curr.kind == KILN_DYN_IDENT) {
            explicit_name = 1;
            fn_name = lex->curr.start;
            fn_name_len = lex->curr.len;
            kiln_dyn_lexer_next(lex);
        }
        if (lex->curr.kind != KILN_DYN_LPAREN) {
            return kiln_dyn_throw_parse_error(lex, thrown, "Expected '(' after function");
        }
        size_t pstart = lex->pos;
        int pdepth = 1;
        while (pdepth > 0 && lex->pos < lex->len) {
            char c = lex->src[lex->pos];
            if (c == '"' || c == '\'' || c == '`') {
                lex->pos++;
                while (lex->pos < lex->len) {
                    if (lex->src[lex->pos] == '\\') {
                        lex->pos += 2;
                    } else if (lex->src[lex->pos] == c) {
                        lex->pos++;
                        break;
                    } else {
                        lex->pos++;
                    }
                }
                continue;
            }
            if (c == '(') pdepth++;
            else if (c == ')') pdepth--;
            lex->pos++;
        }
        size_t plen = (lex->pos > 0 && lex->pos > pstart) ? (lex->pos - 1 - pstart) : 0;
        kiln_dyn_lexer_next(lex);

        if (lex->curr.kind != KILN_DYN_LBRACE) {
            return kiln_dyn_throw_parse_error(lex, thrown, "Expected '{' for function body");
        }
        const char *body_source = NULL;
        size_t blen = 0;
        if (!kiln_dyn_capture_block(lex, &body_source, &blen))
            return kiln_dyn_throw_parse_error(lex, thrown,
                                        "Unterminated function body");
        /* Keep the complete source-backed function expression for Function#toString. The
         * trampoline's body upvalue below is intentionally only the executable body; using it
         * as reflection text loses the parameter list and braces. Convert the exact parser slice
         * through the WTF-8-aware helper so lone UTF-16 surrogates survive. */
        const char *function_end = body_source + blen + 1; /* include the closing brace */
        size_t function_source_len = (size_t)(function_end - function_source);

        kiln_value upvals[3];
        kiln_value reflected_source;
        kiln_value name_value = kiln_c99_undefined();
        const kiln_string *closure_name = NULL;
        int status;
        if (explicit_name && !lex->function_declaration) {
            name_value = kiln_c99_copy_text(fn_name, fn_name_len);
            if (name_value.tag != KILN_VALUE_STRING) return KILN_C99_OUT_OF_MEMORY;
            kiln_c99_gc_keep_alive(&name_value);
            closure_name = (const kiln_string *)name_value.ptr;
        }
        status = kiln_dyn_source_to_string(&lex->src[pstart], plen, &upvals[0]);
        if (status != KILN_C99_OK) return status;
        kiln_c99_gc_keep_alive(&upvals[0]);
        status = kiln_dyn_source_to_string(body_source, blen, &upvals[1]);
        if (status != KILN_C99_OK) return status;
        kiln_c99_gc_keep_alive(&upvals[0]);
        kiln_c99_gc_keep_alive(&upvals[1]);
        status = kiln_dyn_source_to_string(function_source, function_source_len,
                                           &reflected_source);
        if (status != KILN_C99_OK) return status;
        kiln_c99_gc_keep_alive(&reflected_source);
        int function_strict = scope->strict ||
                              kiln_dyn_body_has_use_strict(body_source, blen);
        kiln_dyn_scope function_scope;
        kiln_dyn_scope_init(&function_scope, scope);
        function_scope.strict = function_strict;
        kiln_value self_cell = kiln_c99_undefined();
        if (explicit_name) {
            status = kiln_c99_make_cell(&self_cell);
            if (status != KILN_C99_OK) {
                kiln_c99_gc_keep_alive(&reflected_source);
                kiln_dyn_scope_destroy(&function_scope);
                return status;
            }
            kiln_c99_gc_keep_alive(&self_cell);
            kiln_c99_gc_keep_alive(&reflected_source);
            if (!kiln_dyn_scope_set(&function_scope, fn_name, fn_name_len, self_cell)) {
                kiln_c99_gc_keep_alive(&reflected_source);
                kiln_dyn_scope_destroy(&function_scope);
                return KILN_C99_OUT_OF_MEMORY;
            }
            (void)kiln_dyn_scope_mark_restriction(&function_scope, fn_name, fn_name_len,
                                                  KILN_DYN_BINDING_FUNCTION_NAME);
        }
        kiln_value env_obj;
        status = kiln_dyn_new_environment(&env_obj);
        if (status != KILN_C99_OK) {
            kiln_c99_gc_keep_alive(&reflected_source);
            kiln_dyn_scope_destroy(&function_scope);
            return status;
        }
        kiln_c99_gc_keep_alive(&env_obj);
        kiln_c99_gc_keep_alive(&reflected_source);
        status = kiln_dyn_capture_environment(host,
                                              explicit_name && !lex->function_declaration
                                                  ? &function_scope : scope,
                                              env_obj, 0, thrown);
        if (status != KILN_C99_OK) {
            kiln_c99_gc_keep_alive(&reflected_source);
            kiln_dyn_scope_destroy(&function_scope);
            return status;
        }
        kiln_c99_gc_keep_alive(&upvals[0]);
        kiln_c99_gc_keep_alive(&upvals[1]);
        kiln_c99_gc_keep_alive(&env_obj);
        kiln_c99_gc_keep_alive(&reflected_source);
        upvals[2] = env_obj;
        kiln_val val_upvals[3] = {
            kiln_val_from_boundary(upvals[0]),
            kiln_val_from_boundary(upvals[1]),
            kiln_val_from_boundary(upvals[2])
        };

        status = kiln_c99_make_closure(out, kiln_c99_dynamic_body_trampoline,
                                       closure_name,
                                       "", 0,
                                       /* Async dynamic functions use the generator-backed async
                                        * call path.  The dynamic body trampoline itself returns
                                        * normally; the async driver turns that completion into a
                                        * promise just as it does for compiled async functions. */
                                       "eval", 0, 0, asynchronous, asynchronous, 0, 0, 0,
                                       function_strict,
                                       KILN_VAL_UNDEFINED, KILN_VAL_UNDEFINED,
                                       0, 16, 0, 0, val_upvals, 3);
        if (status != KILN_C99_OK) {
            kiln_c99_gc_keep_alive(&reflected_source);
            kiln_dyn_scope_destroy(&function_scope);
            return status;
        }
        kiln_c99_gc_keep_alive(&upvals[0]);
        kiln_c99_gc_keep_alive(&upvals[1]);
        kiln_c99_gc_keep_alive(&env_obj);
        kiln_c99_gc_keep_alive(&reflected_source);
        if (explicit_name) kiln_c99_gc_keep_alive(&self_cell);
        kiln_c99_heap_store_value(kiln_c99_closure_of(*out),
                                  &kiln_c99_closure_of(*out)->source_value,
                                  reflected_source);
        if (explicit_name && !lex->function_declaration) {
            kiln_c99_val_store_cell(kiln_c99_cell_of(self_cell),
                                    kiln_val_from_boundary(*out));
        }
        kiln_dyn_scope_destroy(&function_scope);
        return KILN_C99_OK;
    }

    if (lex->curr.kind == KILN_DYN_KW_NEW) {
        kiln_dyn_lexer_next(lex);
        return kiln_dyn_eval_postfix(host, lex, scope, receiver, args, argc, depth, out,
                                     thrown, 1);
    }

    return kiln_dyn_throw_unexpected_token(lex, thrown);
}

static int kiln_dyn_eval_postfix(kiln_host *host, kiln_dyn_lexer *lex, kiln_dyn_scope *scope,
                                 kiln_value receiver, const kiln_value *args, size_t argc,
                                 int depth, kiln_value *out, kiln_value *thrown,
                                 int construct_first_call) {
    int status = kiln_dyn_eval_primary(host, lex, scope, receiver, args, argc, depth, out, thrown);
    if (status != KILN_C99_OK) return status;

    kiln_value current_receiver = kiln_c99_undefined();
    int constructing = construct_first_call;

    while (1) {
        if (lex->curr.kind == KILN_DYN_DOT) {
            kiln_dyn_lexer_next(lex);
            if (!kiln_dyn_is_ident_name(lex->curr.kind)) {
                return kiln_dyn_throw_parse_error(lex, thrown, "Expected identifier after '.'");
            }
            const char *prop = lex->curr.start;
            size_t prop_len = lex->curr.len;
            kiln_dyn_lexer_next(lex);
            kiln_value res;
            status = kiln_c99_get(host, *out, prop, prop_len, &res, depth, thrown);
            if (status != KILN_C99_OK) return status;
            current_receiver = *out;
            *out = res;
        } else if (lex->curr.kind == KILN_DYN_LBRACKET) {
            kiln_dyn_lexer_next(lex);
            kiln_value idx;
            status = kiln_dyn_eval_expr(host, lex, scope, receiver, args, argc, depth, &idx, thrown);
            if (status != KILN_C99_OK) return status;
            if (lex->curr.kind != KILN_DYN_RBRACKET) {
                return kiln_dyn_throw_parse_error(lex, thrown, "Expected ']'");
            }
            kiln_dyn_lexer_next(lex);
            kiln_value res;
            status = kiln_c99_get_computed(host, *out, idx, &res, depth, thrown);
            if (status != KILN_C99_OK) return status;
            current_receiver = *out;
            *out = res;
        } else if (lex->curr.kind == KILN_DYN_LPAREN) {
            kiln_dyn_lexer_next(lex);
            kiln_value call_args[16];
            size_t call_argc = 0;
            while (lex->curr.kind != KILN_DYN_RPAREN && lex->curr.kind != KILN_DYN_EOF) {
                if (call_argc < 16) {
                    status = kiln_dyn_eval_expr(host, lex, scope, receiver, args, argc, depth, &call_args[call_argc++], thrown);
                    if (status != KILN_C99_OK) return status;
                } else {
                    return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_RANGE_ERROR, "Too many arguments");
                }
                if (lex->curr.kind == KILN_DYN_COMMA) {
                    kiln_dyn_lexer_next(lex);
                } else {
                    break;
                }
            }
            if (lex->curr.kind != KILN_DYN_RPAREN) {
                return kiln_dyn_throw_parse_error(lex, thrown, "Expected ')'");
            }
            kiln_dyn_lexer_next(lex);
            kiln_value res;
            status = constructing
                         ? kiln_c99_construct(host, &res, *out, call_args, call_argc, depth,
                                              thrown)
                         : kiln_c99_call(host, &res, *out, current_receiver, call_args,
                                         call_argc, depth, thrown);
            if (status != KILN_C99_OK) return status;
            constructing = 0;
            current_receiver = kiln_c99_undefined();
            *out = res;
        } else {
            break;
        }
    }
    if (constructing) {
        status = kiln_c99_construct(host, out, *out, NULL, 0, depth, thrown);
        if (status != KILN_C99_OK) return status;
    }
    return KILN_C99_OK;
}

static int kiln_dyn_member_assignment_ahead(const kiln_dyn_lexer *input, int prefix);
static int kiln_dyn_eval_member_assignment(kiln_host *host, kiln_dyn_lexer *lex,
    kiln_dyn_scope *scope, kiln_value receiver, const kiln_value *args, size_t argc,
    int depth, kiln_value *out, kiln_value *thrown, kiln_dyn_token_kind prefix_update);
static int kiln_dyn_eval_delete_member(kiln_host *host, kiln_dyn_lexer *lex,
    kiln_dyn_scope *scope, kiln_value receiver, const kiln_value *args, size_t argc,
    int depth, kiln_value *out, kiln_value *thrown);
static int kiln_dyn_eval_delete_identifier(kiln_host *host, kiln_dyn_lexer *lex,
    kiln_dyn_scope *scope, int depth, kiln_value *out, kiln_value *thrown);
static int kiln_dyn_eval_delete_operand(kiln_host *host, kiln_dyn_lexer *lex,
    kiln_dyn_scope *scope, kiln_value receiver, const kiln_value *args, size_t argc,
    int depth, kiln_value *out, kiln_value *thrown);

typedef enum {
    KILN_DYN_UPDATE_BINDING,
    KILN_DYN_UPDATE_WITH,
    KILN_DYN_UPDATE_GLOBAL,
} kiln_dyn_update_reference;

static int kiln_dyn_resolve_update_reference(kiln_host *host, kiln_dyn_scope *scope,
                                            const char *name, size_t name_len, int depth,
                                            int allow_missing, int read_value, kiln_value *current,
                                            kiln_dyn_update_reference *reference,
                                            kiln_dyn_scope **binding_scope,
                                            kiln_value *with_target,
                                            kiln_value *global_target,
                                            int *unresolvable,
                                            kiln_value *thrown) {
    *unresolvable = 0;
    for (kiln_dyn_scope *environment = scope; environment != NULL;
         environment = environment->parent) {
        for (size_t index = 0; index < environment->count; ++index) {
            kiln_dyn_binding *binding = &environment->bindings[index];
            if (binding->name_len != name_len || memcmp(binding->name, name, name_len) != 0)
                continue;
            if (binding->value.tag == KILN_VALUE_CELL) {
                kiln_val *cell = kiln_c99_cell_of(binding->value);
                int status = kiln_c99_val_check_tdz(*cell, thrown);
                if (status != KILN_C99_OK) return status;
                if (read_value) *current = kiln_val_to_boundary(*cell);
            } else if (read_value) {
                *current = binding->value;
            }
            *binding_scope = environment;
            *reference = KILN_DYN_UPDATE_BINDING;
            return KILN_C99_OK;
        }
        int captured = 0;
        int is_captured_with = 0;
        kiln_value captured_with_target = kiln_c99_undefined();
        int captured_status = environment->has_captured_records
                                  ? kiln_dyn_captured_records_get(host,
                                        environment->captured_records, name, name_len, depth,
                                        read_value, current, &captured, &is_captured_with,
                                        &captured_with_target, thrown)
                                  : KILN_C99_OK;
        if (captured_status != KILN_C99_OK) return captured_status;
        if (captured) {
            if (is_captured_with) {
                *with_target = captured_with_target;
                kiln_c99_gc_keep_alive(with_target);
                *reference = KILN_DYN_UPDATE_WITH;
            } else {
                *binding_scope = environment;
                *reference = KILN_DYN_UPDATE_BINDING;
            }
            return KILN_C99_OK;
        }
        if (!environment->has_captured_records) {
            captured_status = kiln_dyn_captured_get(environment, name, name_len, current,
                                                    &captured, thrown);
            if (captured_status != KILN_C99_OK) return captured_status;
            if (captured) {
                *binding_scope = environment;
                *reference = KILN_DYN_UPDATE_BINDING;
                return KILN_C99_OK;
            }
        }
        if (environment->has_with_object) {
            int present = 0;
            int status = kiln_dyn_with_has_binding(host, environment->with_object, name,
                                                   name_len, depth, &present, thrown);
            if (status != KILN_C99_OK) return status;
            if (present) {
                *with_target = environment->with_object;
                kiln_c99_gc_keep_alive(with_target);
                if (read_value) {
                    status = kiln_c99_get(host, *with_target, name, name_len, current, depth,
                                          thrown);
                    if (status != KILN_C99_OK) return status;
                }
                *reference = KILN_DYN_UPDATE_WITH;
                return KILN_C99_OK;
            }
        }
    }
    int status = kiln_c99_global_object(global_target);
    if (status != KILN_C99_OK) return status;
    int present = 0;
    status = kiln_c99_has_property(host, *global_target, name, name_len, &present, depth,
                                   thrown);
    if (status != KILN_C99_OK) return status;
    if (!present) {
        if (!allow_missing)
            return kiln_dyn_throw_unresolvable_reference(thrown, name, name_len);
        *current = kiln_c99_undefined();
        *unresolvable = 1;
    } else if (read_value) {
        status = kiln_c99_get(host, *global_target, name, name_len, current, depth, thrown);
        if (status != KILN_C99_OK) return status;
    }
    *reference = KILN_DYN_UPDATE_GLOBAL;
    kiln_c99_gc_keep_alive(global_target);
    return KILN_C99_OK;
}

/* Identifier updates share one reference path for prefix and postfix forms. In particular,
 * resolve the reference before ToNumeric so missing names throw, then preserve the environment's
 * write rules when conversion has side effects. */
static int kiln_dyn_update_identifier(kiln_host *host, kiln_dyn_scope *scope,
                                      const char *name, size_t name_len, int increment,
                                      int prefix, int depth, kiln_value *out,
                                      kiln_value *thrown) {
    kiln_value current = kiln_c99_undefined();
    kiln_value global_target = kiln_c99_undefined();
    kiln_value with_target = kiln_c99_undefined();
    kiln_dyn_scope *binding_scope = NULL;
    kiln_dyn_update_reference reference = KILN_DYN_UPDATE_BINDING;
    int unresolvable = 0;
    int status = kiln_dyn_resolve_update_reference(host, scope, name, name_len, depth, 0, 1,
                                                   &current, &reference, &binding_scope,
                                                   &with_target, &global_target, &unresolvable,
                                                   thrown);
    if (status != KILN_C99_OK) return status;
    kiln_c99_gc_keep_alive(&current);
    kiln_c99_gc_keep_alive(&with_target);
    kiln_c99_gc_keep_alive(&global_target);

    kiln_value numeric = kiln_c99_undefined();
    status = kiln_c99_unary_checked(7, host, current, &numeric, depth, thrown);
    kiln_c99_gc_keep_alive(&current);
    kiln_c99_gc_keep_alive(&with_target);
    if (status != KILN_C99_OK) return status;
    kiln_c99_gc_keep_alive(&numeric);
    kiln_value next = kiln_c99_undefined();
    status = kiln_c99_unary_checked(increment ? 8 : 9, host, numeric, &next, depth, thrown);
    kiln_c99_gc_keep_alive(&numeric);
    kiln_c99_gc_keep_alive(&with_target);
    if (status != KILN_C99_OK) return status;
    kiln_c99_gc_keep_alive(&next);

    int ignore_write = 0;
    if (reference == KILN_DYN_UPDATE_BINDING) {
        status = kiln_dyn_scope_check_write(binding_scope, name, name_len, &ignore_write, thrown);
        if (status != KILN_C99_OK) return status;
    } else if (reference == KILN_DYN_UPDATE_WITH) {
        /* Keep the object environment selected by initial resolution. PutValue invokes
         * SetMutableBinding on that same environment after ToNumeric, even if the getter removed
         * the property; it must not walk outward to a different `with` or global binding. */
        kiln_c99_gc_keep_alive(&with_target);
    }
    if (!ignore_write) {
        if (reference == KILN_DYN_UPDATE_BINDING) {
            if (!kiln_dyn_scope_update(binding_scope, name, name_len, next))
                return KILN_C99_OUT_OF_MEMORY;
        } else if (reference == KILN_DYN_UPDATE_WITH) {
            status = kiln_c99_put(host, with_target, name, name_len, next, 0, thrown);
            if (status != KILN_C99_OK) return status;
        } else {
            if (unresolvable && scope->strict) {
                return kiln_dyn_throw_unresolvable_reference(thrown, name, name_len);
            }
            status = kiln_c99_put(host, global_target, name, name_len, next, 0, thrown);
            if (status != KILN_C99_OK) return status;
        }
    }
    *out = prefix ? next : numeric;
    kiln_c99_gc_keep_alive(out);
    return KILN_C99_OK;
}

static int kiln_dyn_eval_unary(kiln_host *host, kiln_dyn_lexer *lex, kiln_dyn_scope *scope,
                               kiln_value receiver, const kiln_value *args, size_t argc,
                               int depth, kiln_value *out, kiln_value *thrown) {
    if (lex->curr.kind == KILN_DYN_PLUS_PLUS || lex->curr.kind == KILN_DYN_MINUS_MINUS) {
        kiln_dyn_token_kind update = lex->curr.kind;
        kiln_dyn_lexer_next(lex);
        if (kiln_dyn_member_assignment_ahead(lex, 1))
            return kiln_dyn_eval_member_assignment(host, lex, scope, receiver, args, argc,
                                                   depth, out, thrown, update);
        if (lex->curr.kind == KILN_DYN_IDENT) {
            const char *name = lex->curr.start;
            size_t name_len = lex->curr.len;
            kiln_dyn_lexer_next(lex);
            return kiln_dyn_update_identifier(host, scope, name, name_len,
                                              update == KILN_DYN_PLUS_PLUS, 1, depth,
                                              out, thrown);
        }
        return kiln_dyn_throw_parse_error(lex, thrown,
                                    "Expected member reference after update operator");
    }
    if (kiln_dyn_member_assignment_ahead(lex, 2))
        return kiln_dyn_eval_member_assignment(host, lex, scope, receiver, args, argc,
                                               depth, out, thrown, KILN_DYN_EOF);
    if (lex->curr.kind == KILN_DYN_IDENT) {
        const char *name = lex->curr.start;
        size_t name_len = lex->curr.len;
        kiln_dyn_lexer look;
        memset(&look, 0, sizeof look);
        look.src = lex->src;
        look.len = lex->len;
        look.pos = (size_t)(lex->curr.start - lex->src) + lex->curr.len;
        look.dynamic_function_body = lex->dynamic_function_body;
        look.strict = lex->strict;
        look.prev_kind = lex->curr.kind;
        kiln_dyn_lexer_next(&look);
        int decrement = look.curr.kind == KILN_DYN_MINUS_MINUS;
        int update = decrement || look.curr.kind == KILN_DYN_PLUS_PLUS;
        kiln_dyn_token_free(&look.curr);
        if (update) {
            kiln_dyn_lexer_next(lex);
            kiln_dyn_lexer_next(lex);
            return kiln_dyn_update_identifier(host, scope, name, name_len, !decrement, 0,
                                              depth, out, thrown);
        }
    }
    if (lex->curr.kind == KILN_DYN_NOT) {
        kiln_dyn_lexer_next(lex);
        int status = kiln_dyn_eval_unary(host, lex, scope, receiver, args, argc, depth, out, thrown);
        if (status != KILN_C99_OK) return status;
        *out = kiln_c99_boolean(!kiln_c99_truthy(*out));
        return KILN_C99_OK;
    }
    if (lex->curr.kind == KILN_DYN_MINUS) {
        kiln_dyn_lexer_next(lex);
        int status = kiln_dyn_eval_unary(host, lex, scope, receiver, args, argc, depth, out, thrown);
        if (status != KILN_C99_OK) return status;
        *out = kiln_c99_unary(2, *out);
        return KILN_C99_OK;
    }
    if (lex->curr.kind == KILN_DYN_PLUS) {
        kiln_dyn_lexer_next(lex);
        int status = kiln_dyn_eval_unary(host, lex, scope, receiver, args, argc, depth, out, thrown);
        if (status != KILN_C99_OK) return status;
        *out = kiln_c99_unary(1, *out);
        return KILN_C99_OK;
    }
    if (lex->curr.kind == KILN_DYN_BIT_NOT) {
        kiln_dyn_lexer_next(lex);
        int status = kiln_dyn_eval_unary(host, lex, scope, receiver, args, argc, depth, out, thrown);
        if (status != KILN_C99_OK) return status;
        *out = kiln_c99_unary(5, *out);
        return KILN_C99_OK;
    }
    if (lex->curr.kind == KILN_DYN_KW_DELETE) {
        kiln_dyn_lexer_next(lex);
        return kiln_dyn_eval_delete_operand(host, lex, scope, receiver, args, argc,
                                            depth, out, thrown);
    }
    if (lex->curr.kind == KILN_DYN_KW_TYPEOF) {
        kiln_dyn_lexer_next(lex);
        if (lex->curr.kind == KILN_DYN_IDENT) {
            const char *name = lex->curr.start;
            size_t name_len = lex->curr.len;
            size_t saved_pos = lex->pos;
            kiln_dyn_token saved_tok = lex->curr;
            kiln_dyn_lexer_next(lex);
            if (lex->curr.kind != KILN_DYN_DOT && lex->curr.kind != KILN_DYN_LBRACKET && lex->curr.kind != KILN_DYN_LPAREN) {
                int scope_found = 0;
                int scope_status = kiln_dyn_scope_get(host, scope, name, name_len, depth,
                                                      out, &scope_found, thrown);
                if (scope_status != KILN_C99_OK) return scope_status;
                if (scope_found) {
                    *out = kiln_c99_typeof(*out);
                    return KILN_C99_OK;
                }
                kiln_value globals = kiln_c99_undefined();
                int gstatus = kiln_c99_global_object(&globals);
                if (gstatus == KILN_C99_OK) {
                    if (kiln_c99_get(host, globals, name, name_len, out, depth, NULL) == KILN_C99_OK) {
                        *out = kiln_c99_typeof(*out);
                        return KILN_C99_OK;
                    }
                }
                *out = kiln_c99_copy_text("undefined", 9);
                return KILN_C99_OK;
            }
            lex->pos = saved_pos;
            lex->curr = saved_tok;
        }
        int status = kiln_dyn_eval_unary(host, lex, scope, receiver, args, argc, depth, out, thrown);
        if (status != KILN_C99_OK) return status;
        *out = kiln_c99_typeof(*out);
        return KILN_C99_OK;
    }
    return kiln_dyn_eval_postfix(host, lex, scope, receiver, args, argc, depth, out, thrown, 0);
}

static int kiln_dyn_eval_multiplicative(kiln_host *host, kiln_dyn_lexer *lex, kiln_dyn_scope *scope,
                                        kiln_value receiver, const kiln_value *args, size_t argc,
                                        int depth, kiln_value *out, kiln_value *thrown) {
    int status = kiln_dyn_eval_unary(host, lex, scope, receiver, args, argc, depth, out, thrown);
    if (status != KILN_C99_OK) return status;

    while (lex->curr.kind == KILN_DYN_STAR || lex->curr.kind == KILN_DYN_SLASH || lex->curr.kind == KILN_DYN_PERCENT) {
        kiln_dyn_token_kind op = lex->curr.kind;
        kiln_dyn_lexer_next(lex);
        kiln_value right;
        status = kiln_dyn_eval_unary(host, lex, scope, receiver, args, argc, depth, &right, thrown);
        if (status != KILN_C99_OK) return status;
        if (op == KILN_DYN_STAR) *out = kiln_c99_binary(3, *out, right);
        else if (op == KILN_DYN_SLASH) *out = kiln_c99_binary(4, *out, right);
        else if (op == KILN_DYN_PERCENT) *out = kiln_c99_binary(5, *out, right);
    }
    return KILN_C99_OK;
}

static int kiln_dyn_eval_additive(kiln_host *host, kiln_dyn_lexer *lex, kiln_dyn_scope *scope,
                                  kiln_value receiver, const kiln_value *args, size_t argc,
                                  int depth, kiln_value *out, kiln_value *thrown) {
    int status = kiln_dyn_eval_multiplicative(host, lex, scope, receiver, args, argc, depth, out, thrown);
    if (status != KILN_C99_OK) return status;

    while (lex->curr.kind == KILN_DYN_PLUS || lex->curr.kind == KILN_DYN_MINUS) {
        kiln_dyn_token_kind op = lex->curr.kind;
        kiln_dyn_lexer_next(lex);
        kiln_value right;
        status = kiln_dyn_eval_multiplicative(host, lex, scope, receiver, args, argc, depth, &right, thrown);
        if (status != KILN_C99_OK) return status;
        if (op == KILN_DYN_PLUS) *out = kiln_c99_binary(1, *out, right);
        else *out = kiln_c99_binary(2, *out, right);
    }
    return KILN_C99_OK;
}

static int kiln_dyn_eval_shift(kiln_host *host, kiln_dyn_lexer *lex, kiln_dyn_scope *scope,
                               kiln_value receiver, const kiln_value *args, size_t argc,
                               int depth, kiln_value *out, kiln_value *thrown) {
    int status = kiln_dyn_eval_additive(host, lex, scope, receiver, args, argc, depth, out, thrown);
    if (status != KILN_C99_OK) return status;
    while (lex->curr.kind == KILN_DYN_LSHIFT || lex->curr.kind == KILN_DYN_RSHIFT ||
           lex->curr.kind == KILN_DYN_URSHIFT) {
        kiln_dyn_token_kind op = lex->curr.kind;
        kiln_dyn_lexer_next(lex);
        kiln_value right;
        status = kiln_dyn_eval_additive(host, lex, scope, receiver, args, argc, depth, &right, thrown);
        if (status != KILN_C99_OK) return status;
        *out = kiln_c99_binary(op == KILN_DYN_LSHIFT ? 20 : op == KILN_DYN_RSHIFT ? 21 : 22,
                               *out, right);
    }
    return KILN_C99_OK;
}

static int kiln_dyn_eval_relational(kiln_host *host, kiln_dyn_lexer *lex, kiln_dyn_scope *scope,
                                    kiln_value receiver, const kiln_value *args, size_t argc,
                                    int depth, kiln_value *out, kiln_value *thrown) {
    int status = kiln_dyn_eval_shift(host, lex, scope, receiver, args, argc, depth, out, thrown);
    if (status != KILN_C99_OK) return status;

    while (lex->curr.kind == KILN_DYN_LT || lex->curr.kind == KILN_DYN_LE ||
           lex->curr.kind == KILN_DYN_GT || lex->curr.kind == KILN_DYN_GE ||
           lex->curr.kind == KILN_DYN_KW_IN ||
           lex->curr.kind == KILN_DYN_KW_INSTANCEOF) {
        kiln_dyn_token_kind op = lex->curr.kind;
        kiln_dyn_lexer_next(lex);
        kiln_value right;
        status = kiln_dyn_eval_shift(host, lex, scope, receiver, args, argc, depth, &right, thrown);
        if (status != KILN_C99_OK) return status;
        if (op == KILN_DYN_KW_IN) {
            int answer = 0;
            status = kiln_c99_has_computed(host, right, *out, &answer, depth, thrown);
            if (status != KILN_C99_OK) return status;
            *out = kiln_c99_boolean(answer);
        } else if (op == KILN_DYN_KW_INSTANCEOF) {
            int answer = 0;
            status = kiln_c99_instance_of(host, *out, right, &answer, depth, thrown);
            if (status != KILN_C99_OK) return status;
            *out = kiln_c99_boolean(answer);
        } else if (op == KILN_DYN_LT) *out = kiln_c99_binary(9, *out, right);
        else if (op == KILN_DYN_LE) *out = kiln_c99_binary(10, *out, right);
        else if (op == KILN_DYN_GT) *out = kiln_c99_binary(11, *out, right);
        else if (op == KILN_DYN_GE) *out = kiln_c99_binary(12, *out, right);
    }
    return KILN_C99_OK;
}

static int kiln_dyn_eval_equality(kiln_host *host, kiln_dyn_lexer *lex, kiln_dyn_scope *scope,
                                  kiln_value receiver, const kiln_value *args, size_t argc,
                                  int depth, kiln_value *out, kiln_value *thrown) {
    int status = kiln_dyn_eval_relational(host, lex, scope, receiver, args, argc, depth, out, thrown);
    if (status != KILN_C99_OK) return status;

    while (lex->curr.kind == KILN_DYN_STRICT_EQ || lex->curr.kind == KILN_DYN_STRICT_NE ||
           lex->curr.kind == KILN_DYN_EQ || lex->curr.kind == KILN_DYN_NE) {
        kiln_dyn_token_kind op = lex->curr.kind;
        kiln_dyn_lexer_next(lex);
        kiln_value right;
        status = kiln_dyn_eval_relational(host, lex, scope, receiver, args, argc, depth, &right, thrown);
        if (status != KILN_C99_OK) return status;
        if (op == KILN_DYN_STRICT_EQ) *out = kiln_c99_binary(7, *out, right);
        else if (op == KILN_DYN_STRICT_NE) *out = kiln_c99_binary(8, *out, right);
        else if (op == KILN_DYN_EQ) *out = kiln_c99_binary(15, *out, right);
        else if (op == KILN_DYN_NE) *out = kiln_c99_binary(16, *out, right);
    }
    return KILN_C99_OK;
}

static void kiln_dyn_skip_expr(kiln_dyn_lexer *lex, int stop_at_colon) {
    int pdepth = 0;
    int bdepth = 0;
    int kdepth = 0;
    while (lex->curr.kind != KILN_DYN_EOF && lex->curr.kind != KILN_DYN_ERROR) {
        if (pdepth == 0 && bdepth == 0 && kdepth == 0) {
            if (stop_at_colon && lex->curr.kind == KILN_DYN_COLON) break;
            if (lex->curr.kind == KILN_DYN_SEMI || lex->curr.kind == KILN_DYN_COMMA ||
                lex->curr.kind == KILN_DYN_RPAREN || lex->curr.kind == KILN_DYN_RBRACE ||
                lex->curr.kind == KILN_DYN_RBRACKET) {
                break;
            }
        }
        if (lex->curr.kind == KILN_DYN_LPAREN) pdepth++;
        else if (lex->curr.kind == KILN_DYN_RPAREN && pdepth > 0) pdepth--;
        else if (lex->curr.kind == KILN_DYN_LBRACE) bdepth++;
        else if (lex->curr.kind == KILN_DYN_RBRACE && bdepth > 0) bdepth--;
        else if (lex->curr.kind == KILN_DYN_LBRACKET) kdepth++;
        else if (lex->curr.kind == KILN_DYN_RBRACKET && kdepth > 0) kdepth--;
        kiln_dyn_lexer_next(lex);
    }
}

/* Skip source without evaluating it, stopping at the grammar boundary needed by the caller.
 * Short-circuit operands stop before lower-precedence logical/conditional operators, while a
 * conditional consequent tracks nested ?: pairs and stops at its matching colon. */
static void kiln_dyn_skip_unexecuted(kiln_dyn_lexer *lex, kiln_dyn_token_kind logical_op,
                                     int match_conditional_colon) {
    int pdepth = 0;
    int bdepth = 0;
    int kdepth = 0;
    int conditional_depth = 0;
    while (lex->curr.kind != KILN_DYN_EOF && lex->curr.kind != KILN_DYN_ERROR) {
        if (pdepth == 0 && bdepth == 0 && kdepth == 0) {
            if (match_conditional_colon) {
                if (lex->curr.kind == KILN_DYN_QUESTION) {
                    conditional_depth++;
                } else if (lex->curr.kind == KILN_DYN_COLON) {
                    if (conditional_depth == 0) break;
                    conditional_depth--;
                } else if (conditional_depth == 0 &&
                           (lex->curr.kind == KILN_DYN_SEMI || lex->curr.kind == KILN_DYN_COMMA ||
                            lex->curr.kind == KILN_DYN_RPAREN || lex->curr.kind == KILN_DYN_RBRACE ||
                            lex->curr.kind == KILN_DYN_RBRACKET)) {
                    break;
                }
            } else if (lex->curr.kind == KILN_DYN_SEMI || lex->curr.kind == KILN_DYN_COMMA ||
                       lex->curr.kind == KILN_DYN_RPAREN || lex->curr.kind == KILN_DYN_RBRACE ||
                       lex->curr.kind == KILN_DYN_RBRACKET || lex->curr.kind == KILN_DYN_QUESTION ||
                       lex->curr.kind == KILN_DYN_COLON || lex->curr.kind == KILN_DYN_OR ||
                       lex->curr.kind == KILN_DYN_NULLISH ||
                       (logical_op == KILN_DYN_AND && lex->curr.kind == KILN_DYN_AND)) {
                break;
            }
        }
        if (lex->curr.kind == KILN_DYN_LPAREN) pdepth++;
        else if (lex->curr.kind == KILN_DYN_RPAREN && pdepth > 0) pdepth--;
        else if (lex->curr.kind == KILN_DYN_LBRACE) bdepth++;
        else if (lex->curr.kind == KILN_DYN_RBRACE && bdepth > 0) bdepth--;
        else if (lex->curr.kind == KILN_DYN_LBRACKET) kdepth++;
        else if (lex->curr.kind == KILN_DYN_RBRACKET && kdepth > 0) kdepth--;
        kiln_dyn_lexer_next(lex);
    }
}

static int kiln_dyn_eval_bitwise_and(kiln_host *host, kiln_dyn_lexer *lex, kiln_dyn_scope *scope,
                                     kiln_value receiver, const kiln_value *args, size_t argc,
                                     int depth, kiln_value *out, kiln_value *thrown) {
    int status = kiln_dyn_eval_equality(host, lex, scope, receiver, args, argc, depth, out, thrown);
    if (status != KILN_C99_OK) return status;
    while (lex->curr.kind == KILN_DYN_BIT_AND) {
        kiln_dyn_lexer_next(lex); kiln_value right;
        status = kiln_dyn_eval_equality(host, lex, scope, receiver, args, argc, depth, &right, thrown);
        if (status != KILN_C99_OK) return status;
        *out = kiln_c99_binary(17, *out, right);
    }
    return KILN_C99_OK;
}

static int kiln_dyn_eval_bitwise_xor(kiln_host *host, kiln_dyn_lexer *lex, kiln_dyn_scope *scope,
                                     kiln_value receiver, const kiln_value *args, size_t argc,
                                     int depth, kiln_value *out, kiln_value *thrown) {
    int status = kiln_dyn_eval_bitwise_and(host, lex, scope, receiver, args, argc, depth, out, thrown);
    if (status != KILN_C99_OK) return status;
    while (lex->curr.kind == KILN_DYN_BIT_XOR) {
        kiln_dyn_lexer_next(lex); kiln_value right;
        status = kiln_dyn_eval_bitwise_and(host, lex, scope, receiver, args, argc, depth, &right, thrown);
        if (status != KILN_C99_OK) return status;
        *out = kiln_c99_binary(19, *out, right);
    }
    return KILN_C99_OK;
}

static int kiln_dyn_eval_bitwise_or(kiln_host *host, kiln_dyn_lexer *lex, kiln_dyn_scope *scope,
                                    kiln_value receiver, const kiln_value *args, size_t argc,
                                    int depth, kiln_value *out, kiln_value *thrown) {
    int status = kiln_dyn_eval_bitwise_xor(host, lex, scope, receiver, args, argc, depth, out, thrown);
    if (status != KILN_C99_OK) return status;
    while (lex->curr.kind == KILN_DYN_BIT_OR) {
        kiln_dyn_lexer_next(lex); kiln_value right;
        status = kiln_dyn_eval_bitwise_xor(host, lex, scope, receiver, args, argc, depth, &right, thrown);
        if (status != KILN_C99_OK) return status;
        *out = kiln_c99_binary(18, *out, right);
    }
    return KILN_C99_OK;
}

static int kiln_dyn_eval_logical_and(kiln_host *host, kiln_dyn_lexer *lex, kiln_dyn_scope *scope,
                                     kiln_value receiver, const kiln_value *args, size_t argc,
                                     int depth, kiln_value *out, kiln_value *thrown) {
    int status = kiln_dyn_eval_bitwise_or(host, lex, scope, receiver, args, argc, depth, out, thrown);
    if (status != KILN_C99_OK) return status;

    while (lex->curr.kind == KILN_DYN_AND) {
        kiln_dyn_lexer_next(lex);
        if (!kiln_c99_truthy(*out)) {
            kiln_dyn_skip_unexecuted(lex, KILN_DYN_AND, 0);
        } else {
            status = kiln_dyn_eval_bitwise_or(host, lex, scope, receiver, args, argc, depth, out, thrown);
            if (status != KILN_C99_OK) return status;
        }
    }
    return KILN_C99_OK;
}

static int kiln_dyn_eval_logical_or(kiln_host *host, kiln_dyn_lexer *lex, kiln_dyn_scope *scope,
                                    kiln_value receiver, const kiln_value *args, size_t argc,
                                    int depth, kiln_value *out, kiln_value *thrown) {
    int status = kiln_dyn_eval_logical_and(host, lex, scope, receiver, args, argc, depth, out, thrown);
    if (status != KILN_C99_OK) return status;

    while (lex->curr.kind == KILN_DYN_OR || lex->curr.kind == KILN_DYN_NULLISH) {
        kiln_dyn_token_kind op = lex->curr.kind;
        kiln_dyn_lexer_next(lex);
        int take_lhs = (op == KILN_DYN_OR) ? kiln_c99_truthy(*out)
                                           : (out->tag != KILN_VALUE_UNDEFINED && out->tag != KILN_VALUE_NULL);
        if (take_lhs) {
            kiln_dyn_skip_unexecuted(lex, op, 0);
        } else {
            status = kiln_dyn_eval_logical_and(host, lex, scope, receiver, args, argc, depth, out, thrown);
            if (status != KILN_C99_OK) return status;
        }
    }
    return KILN_C99_OK;
}

static int kiln_dyn_eval_ternary(kiln_host *host, kiln_dyn_lexer *lex, kiln_dyn_scope *scope,
                                 kiln_value receiver, const kiln_value *args, size_t argc,
                                 int depth, kiln_value *out, kiln_value *thrown) {
    int status = kiln_dyn_eval_logical_or(host, lex, scope, receiver, args, argc, depth, out, thrown);
    if (status != KILN_C99_OK) return status;

    if (lex->curr.kind == KILN_DYN_QUESTION) {
        kiln_dyn_lexer_next(lex);
        int cond_true = kiln_c99_truthy(*out);
        if (cond_true) {
            status = kiln_dyn_eval_expr(host, lex, scope, receiver, args, argc, depth, out, thrown);
            if (status != KILN_C99_OK) return status;
            if (lex->curr.kind == KILN_DYN_COLON) {
                kiln_dyn_lexer_next(lex);
                kiln_dyn_skip_expr(lex, 0);
            }
        } else {
            kiln_dyn_skip_unexecuted(lex, KILN_DYN_EOF, 1);
            if (lex->curr.kind == KILN_DYN_COLON) {
                kiln_dyn_lexer_next(lex);
                status = kiln_dyn_eval_expr(host, lex, scope, receiver, args, argc, depth, out, thrown);
                if (status != KILN_C99_OK) return status;
            }
        }
    }
    return KILN_C99_OK;
}

/* Recognize a member reference without evaluating it. A speculative read would run getters or
 * computed-key effects twice when the expression is not an assignment. The real reference walk
 * below runs only after this token-only lookahead proves that an assignment/update follows. */
static int kiln_dyn_member_assignment_ahead(const kiln_dyn_lexer *input, int mode) {
    kiln_dyn_lexer scan = *input;
    int member = 0;
    int has_member = 0;
    if (scan.curr.kind == KILN_DYN_LPAREN) {
        int nesting = 1;
        kiln_dyn_lexer_next(&scan);
        while (nesting != 0 && scan.curr.kind != KILN_DYN_EOF &&
               scan.curr.kind != KILN_DYN_ERROR) {
            if (scan.curr.kind == KILN_DYN_LPAREN) nesting++;
            else if (scan.curr.kind == KILN_DYN_RPAREN) nesting--;
            if (nesting != 0) kiln_dyn_lexer_next(&scan);
        }
        if (nesting != 0) {
            kiln_dyn_token_free(&scan.curr);
            return 0;
        }
        kiln_dyn_lexer_next(&scan);
    } else {
        if (scan.curr.kind != KILN_DYN_IDENT && scan.curr.kind != KILN_DYN_KW_THIS &&
            scan.curr.kind != KILN_DYN_KW_ARGUMENTS) return 0;
        kiln_dyn_lexer_next(&scan);
    }
    while (scan.curr.kind == KILN_DYN_DOT || scan.curr.kind == KILN_DYN_LBRACKET ||
           (mode >= 3 && scan.curr.kind == KILN_DYN_LPAREN)) {
        if (scan.curr.kind == KILN_DYN_LPAREN) {
            member = 0;
            int nesting = 1;
            kiln_dyn_lexer_next(&scan);
            while (nesting != 0 && scan.curr.kind != KILN_DYN_EOF &&
                   scan.curr.kind != KILN_DYN_ERROR) {
                if (scan.curr.kind == KILN_DYN_LPAREN) nesting++;
                else if (scan.curr.kind == KILN_DYN_RPAREN) nesting--;
                if (nesting != 0) kiln_dyn_lexer_next(&scan);
            }
            if (nesting != 0) {
                kiln_dyn_token_free(&scan.curr);
                return 0;
            }
            kiln_dyn_lexer_next(&scan);
            continue;
        }
        member = 1;
        has_member = 1;
        if (scan.curr.kind == KILN_DYN_DOT) {
            kiln_dyn_lexer_next(&scan);
            if (!kiln_dyn_is_ident_name(scan.curr.kind)) {
                kiln_dyn_token_free(&scan.curr);
                return 0;
            }
            kiln_dyn_lexer_next(&scan);
        } else {
            int nesting = 1;
            kiln_dyn_lexer_next(&scan);
            while (nesting != 0 && scan.curr.kind != KILN_DYN_EOF &&
                   scan.curr.kind != KILN_DYN_ERROR) {
                if (scan.curr.kind == KILN_DYN_LBRACKET) nesting++;
                else if (scan.curr.kind == KILN_DYN_RBRACKET) nesting--;
                kiln_dyn_lexer_next(&scan);
            }
            if (nesting != 0) {
                kiln_dyn_token_free(&scan.curr);
                return 0;
            }
        }
    }
    kiln_dyn_token_kind kind = scan.curr.kind;
    kiln_dyn_token_free(&scan.curr);
    if (mode == 2) return member &&
        (kind == KILN_DYN_PLUS_PLUS || kind == KILN_DYN_MINUS_MINUS);
    if (mode == 3) return member && kind != KILN_DYN_LPAREN;
    if (mode == 4) return has_member;
    if (mode == 5) return member && kind == KILN_DYN_RPAREN;
    return member && ((mode == 1 && kind != KILN_DYN_LPAREN) || kind == KILN_DYN_ASSIGN || kind == KILN_DYN_PLUS_ASSIGN ||
        kind == KILN_DYN_MINUS_ASSIGN || kind == KILN_DYN_STAR_ASSIGN ||
        kind == KILN_DYN_SLASH_ASSIGN || kind == KILN_DYN_PERCENT_ASSIGN ||
        kind == KILN_DYN_LSHIFT_ASSIGN || kind == KILN_DYN_RSHIFT_ASSIGN ||
        kind == KILN_DYN_URSHIFT_ASSIGN || kind == KILN_DYN_BIT_AND_ASSIGN ||
        kind == KILN_DYN_BIT_OR_ASSIGN || kind == KILN_DYN_BIT_XOR_ASSIGN);
}

static int kiln_dyn_eval_member_assignment(kiln_host *host, kiln_dyn_lexer *lex,
    kiln_dyn_scope *scope, kiln_value receiver, const kiln_value *args, size_t argc,
    int depth, kiln_value *out, kiln_value *thrown, kiln_dyn_token_kind prefix_update) {
    kiln_value target, key, current, rhs;
    int status = kiln_dyn_eval_primary(host, lex, scope, receiver, args, argc, depth,
                                       &target, thrown);
    if (status != KILN_C99_OK) return status;
    while (lex->curr.kind == KILN_DYN_DOT || lex->curr.kind == KILN_DYN_LBRACKET) {
        int computed = lex->curr.kind == KILN_DYN_LBRACKET;
        kiln_dyn_lexer_next(lex);
        if (computed) {
            status = kiln_dyn_eval_expr(host, lex, scope, receiver, args, argc, depth,
                                        &key, thrown);
            kiln_c99_gc_keep_alive(&target);
            if (status != KILN_C99_OK) return status;
            if (lex->curr.kind != KILN_DYN_RBRACKET)
                return kiln_dyn_throw_parse_error(lex, thrown,
                                            "Expected ']' in assignment reference");
            kiln_dyn_lexer_next(lex);
            kiln_value property_key;
            status = kiln_c99_reduce_key(host, target, key, &property_key, depth, thrown);
            kiln_c99_gc_keep_alive(&target);
            if (status != KILN_C99_OK) return status;
            key = property_key;
        } else {
            key = kiln_c99_copy_text(lex->curr.start, lex->curr.len);
            kiln_c99_gc_keep_alive(&target);
            if (key.tag != KILN_VALUE_STRING) return KILN_C99_OUT_OF_MEMORY;
            kiln_dyn_lexer_next(lex);
        }
        if (lex->curr.kind == KILN_DYN_DOT || lex->curr.kind == KILN_DYN_LBRACKET) {
            kiln_value next;
            status = kiln_c99_get_computed(host, target, key, &next, depth, thrown);
            kiln_c99_gc_keep_alive(&target);
            kiln_c99_gc_keep_alive(&key);
            if (status != KILN_C99_OK) return status;
            target = next;
        } else break;
    }
    kiln_dyn_token_kind assignment = prefix_update != KILN_DYN_EOF ? prefix_update : lex->curr.kind;
    int update = assignment == KILN_DYN_PLUS_PLUS || assignment == KILN_DYN_MINUS_MINUS;
    if (assignment != KILN_DYN_ASSIGN) {
        status = kiln_c99_get_computed(host, target, key, &current, depth, thrown);
        kiln_c99_gc_keep_alive(&target);
        kiln_c99_gc_keep_alive(&key);
        if (status != KILN_C99_OK) return status;
    }
    if (prefix_update == KILN_DYN_EOF) kiln_dyn_lexer_next(lex);
    if (update) {
        status = kiln_c99_unary_checked(7, host, current, out, depth, thrown);
        kiln_c99_gc_keep_alive(&target);
        kiln_c99_gc_keep_alive(&key);
        kiln_c99_gc_keep_alive(&current);
        if (status != KILN_C99_OK) return status;
        status = kiln_c99_unary_checked(assignment == KILN_DYN_PLUS_PLUS ? 8 : 9,
                                        host, *out, &rhs, depth, thrown);
        kiln_c99_gc_keep_alive(out);
        if (status != KILN_C99_OK) return status;
        kiln_c99_gc_keep_alive(&rhs);
        if (prefix_update != KILN_DYN_EOF) *out = rhs;
    } else {
        status = kiln_dyn_eval_expr(host, lex, scope, receiver, args, argc, depth, &rhs, thrown);
        kiln_c99_gc_keep_alive(&target);
        kiln_c99_gc_keep_alive(&key);
        if (assignment != KILN_DYN_ASSIGN) kiln_c99_gc_keep_alive(&current);
        if (status != KILN_C99_OK) return status;
        switch (assignment) {
        case KILN_DYN_PLUS_ASSIGN: rhs = kiln_c99_binary(1, current, rhs); break;
        case KILN_DYN_MINUS_ASSIGN: rhs = kiln_c99_binary(2, current, rhs); break;
        case KILN_DYN_STAR_ASSIGN: rhs = kiln_c99_binary(3, current, rhs); break;
        case KILN_DYN_SLASH_ASSIGN: rhs = kiln_c99_binary(4, current, rhs); break;
        case KILN_DYN_PERCENT_ASSIGN: rhs = kiln_c99_binary(5, current, rhs); break;
        case KILN_DYN_LSHIFT_ASSIGN: rhs = kiln_c99_binary(20, current, rhs); break;
        case KILN_DYN_RSHIFT_ASSIGN: rhs = kiln_c99_binary(21, current, rhs); break;
        case KILN_DYN_URSHIFT_ASSIGN: rhs = kiln_c99_binary(22, current, rhs); break;
        case KILN_DYN_BIT_AND_ASSIGN: rhs = kiln_c99_binary(17, current, rhs); break;
        case KILN_DYN_BIT_OR_ASSIGN: rhs = kiln_c99_binary(18, current, rhs); break;
        case KILN_DYN_BIT_XOR_ASSIGN: rhs = kiln_c99_binary(19, current, rhs); break;
        default: break;
        }
        *out = rhs;
    }
    status = kiln_c99_set_computed(host, target, key, rhs, depth, thrown);
    kiln_c99_gc_keep_alive(&target);
    kiln_c99_gc_keep_alive(&key);
    kiln_c99_gc_keep_alive(out);
    return status;
}

static int kiln_dyn_eval_delete_member(kiln_host *host, kiln_dyn_lexer *lex,
                                       kiln_dyn_scope *scope, kiln_value receiver,
                                       const kiln_value *args, size_t argc, int depth,
                                       kiln_value *out, kiln_value *thrown) {
    kiln_value target = kiln_c99_undefined(), key = kiln_c99_undefined();
    int status = kiln_dyn_eval_primary(host, lex, scope, receiver, args, argc, depth,
                                       &target, thrown);
    if (status != KILN_C99_OK) return status;
    kiln_value current_receiver = kiln_c99_undefined();
    while (lex->curr.kind == KILN_DYN_DOT || lex->curr.kind == KILN_DYN_LBRACKET ||
           lex->curr.kind == KILN_DYN_LPAREN) {
        if (lex->curr.kind == KILN_DYN_LPAREN) {
            kiln_dyn_lexer_next(lex);
            kiln_value call_args[16];
            size_t call_argc = 0;
            while (lex->curr.kind != KILN_DYN_RPAREN && lex->curr.kind != KILN_DYN_EOF) {
                if (call_argc >= sizeof call_args / sizeof call_args[0])
                    return kiln_dyn_throw_parse_error(lex, thrown,
                                                      "Too many arguments in delete operand");
                status = kiln_dyn_eval_expr(host, lex, scope, receiver, args, argc, depth,
                                            &call_args[call_argc], thrown);
                kiln_c99_gc_keep_alive(&target);
                kiln_c99_gc_keep_alive(&current_receiver);
                if (status != KILN_C99_OK) return status;
                kiln_c99_gc_keep_alive(&call_args[call_argc]);
                call_argc++;
                if (lex->curr.kind != KILN_DYN_COMMA) break;
                kiln_dyn_lexer_next(lex);
            }
            if (lex->curr.kind != KILN_DYN_RPAREN)
                return kiln_dyn_throw_parse_error(lex, thrown,
                                                  "Expected ')' after delete-call arguments");
            kiln_dyn_lexer_next(lex);
            for (size_t i = 0; i < call_argc; ++i)
                kiln_c99_gc_keep_alive(&call_args[i]);
            kiln_value called;
            status = kiln_c99_call(host, &called, target, current_receiver, call_args,
                                   call_argc, depth, thrown);
            kiln_c99_gc_keep_alive(&target);
            kiln_c99_gc_keep_alive(&current_receiver);
            if (status != KILN_C99_OK) return status;
            kiln_c99_gc_keep_alive(&called);
            target = called;
            current_receiver = kiln_c99_undefined();
            continue;
        }
        int computed = lex->curr.kind == KILN_DYN_LBRACKET;
        kiln_dyn_lexer_next(lex);
        if (computed) {
            status = kiln_dyn_eval_expr(host, lex, scope, receiver, args, argc, depth,
                                        &key, thrown);
            kiln_c99_gc_keep_alive(&target);
            if (status != KILN_C99_OK) return status;
            if (lex->curr.kind != KILN_DYN_RBRACKET)
                return kiln_dyn_throw_parse_error(lex, thrown,
                                                  "Expected ']' in delete reference");
            kiln_dyn_lexer_next(lex);
            kiln_value property_key;
            status = kiln_c99_reduce_key(host, target, key, &property_key, depth, thrown);
            kiln_c99_gc_keep_alive(&target);
            if (status != KILN_C99_OK) return status;
            key = property_key;
        } else {
            key = kiln_c99_copy_text(lex->curr.start, lex->curr.len);
            kiln_c99_gc_keep_alive(&target);
            if (key.tag != KILN_VALUE_STRING) return KILN_C99_OUT_OF_MEMORY;
            kiln_dyn_lexer_next(lex);
        }
        if (lex->curr.kind == KILN_DYN_DOT || lex->curr.kind == KILN_DYN_LBRACKET ||
            lex->curr.kind == KILN_DYN_LPAREN) {
            kiln_value next;
            status = kiln_c99_get_computed(host, target, key, &next, depth, thrown);
            kiln_c99_gc_keep_alive(&target);
            kiln_c99_gc_keep_alive(&key);
            if (status != KILN_C99_OK) return status;
            current_receiver = target;
            target = next;
            continue;
        }
        int deleted = 0;
        status = kiln_c99_delete_computed_or_throw(host, target, key, scope->strict,
                                                   &deleted, depth, thrown);
        kiln_c99_gc_keep_alive(&target);
        kiln_c99_gc_keep_alive(&key);
        if (status == KILN_C99_OK) *out = kiln_c99_boolean(deleted);
        return status;
    }
    return kiln_dyn_throw_parse_error(lex, thrown, "Expected property reference after delete");
}

static int kiln_dyn_eval_delete_identifier(kiln_host *host, kiln_dyn_lexer *lex,
    kiln_dyn_scope *scope, int depth, kiln_value *out, kiln_value *thrown) {
    if (scope->strict)
        return kiln_dyn_throw_parse_error(lex, thrown,
                                          "Delete of an unqualified identifier");
    const char *name = lex->curr.start;
    size_t name_len = lex->curr.len;
    for (kiln_dyn_scope *environment = scope; environment != NULL;
         environment = environment->parent) {
        for (size_t i = 0; i < environment->count; ++i) {
            if (environment->bindings[i].name_len == name_len &&
                memcmp(environment->bindings[i].name, name, name_len) == 0) {
                *out = kiln_c99_boolean(0);
                kiln_dyn_lexer_next(lex);
                return KILN_C99_OK;
            }
        }
        int captured = 0;
        int is_captured_with = 0;
        kiln_value captured_target = kiln_c99_undefined();
        int captured_status = environment->has_captured_records
                                  ? kiln_dyn_captured_records_has(host,
                                        environment->captured_records, name, name_len, depth,
                                        &captured, &is_captured_with, &captured_target,
                                        thrown)
                                  : KILN_C99_OK;
        if (captured_status != KILN_C99_OK) return captured_status;
        if (!environment->has_captured_records && environment->has_captured_environment) {
            captured_status = kiln_dyn_private_environment_has(
                environment->captured_environment, name, name_len, &captured);
            if (captured_status != KILN_C99_OK) return captured_status;
        }
        if (captured) {
            int deleted = 0;
            if (is_captured_with) {
                kiln_value key = kiln_c99_copy_text(name, name_len);
                if (key.tag != KILN_VALUE_STRING) return KILN_C99_OUT_OF_MEMORY;
                captured_status = kiln_c99_delete_computed_or_throw(host, captured_target, key,
                                                                    0, &deleted, depth, thrown);
                kiln_c99_gc_keep_alive(&captured_target);
                kiln_c99_gc_keep_alive(&key);
                if (captured_status != KILN_C99_OK) return captured_status;
            }
            *out = kiln_c99_boolean(is_captured_with ? deleted : 0);
            kiln_dyn_lexer_next(lex);
            return KILN_C99_OK;
        }
        if (environment->has_with_object) {
            int present = 0;
            int status = kiln_c99_has_property(host, environment->with_object,
                                               name, name_len, &present, depth, thrown);
            if (status != KILN_C99_OK) return status;
            if (present) {
                kiln_c99_gc_keep_alive(
                    kiln_c99_gc_value_pointer(environment->with_object));
                kiln_value key = kiln_c99_copy_text(name, name_len);
                if (key.tag != KILN_VALUE_STRING) return KILN_C99_OUT_OF_MEMORY;
                int deleted = 0;
                status = kiln_c99_delete_computed_or_throw(host,
                    environment->with_object, key, 0, &deleted, depth, thrown);
                kiln_c99_gc_keep_alive(
                    kiln_c99_gc_value_pointer(environment->with_object));
                kiln_c99_gc_keep_alive(&key);
                if (status != KILN_C99_OK) return status;
                *out = kiln_c99_boolean(deleted);
                kiln_dyn_lexer_next(lex);
                return KILN_C99_OK;
            }
        }
    }
    kiln_value global = kiln_c99_undefined();
    int status = kiln_c99_global_object(&global);
    if (status == KILN_C99_OK) {
        int deleted = 0;
        status = kiln_c99_delete(host, global, name, name_len, &deleted, depth, thrown);
        kiln_c99_gc_keep_alive(kiln_c99_gc_value_pointer(global));
        if (status != KILN_C99_OK) return status;
        *out = kiln_c99_boolean(deleted);
    } else {
        *out = kiln_c99_boolean(1);
    }
    kiln_dyn_lexer_next(lex);
    return KILN_C99_OK;
}

static int kiln_dyn_eval_delete_operand(kiln_host *host, kiln_dyn_lexer *lex,
                                        kiln_dyn_scope *scope, kiln_value receiver,
                                        const kiln_value *args, size_t argc, int depth,
                                        kiln_value *out, kiln_value *thrown) {
    if (kiln_dyn_member_assignment_ahead(lex, 4)) {
        if (kiln_dyn_member_assignment_ahead(lex, 3))
            return kiln_dyn_eval_delete_member(host, lex, scope, receiver, args, argc,
                                               depth, out, thrown);
        int status = kiln_dyn_eval_unary(host, lex, scope, receiver, args, argc,
                                         depth, out, thrown);
        if (status == KILN_C99_OK) *out = kiln_c99_boolean(1);
        return status;
    }
    if (lex->curr.kind == KILN_DYN_LPAREN) {
        kiln_dyn_lexer_next(lex);
        int reference = kiln_dyn_member_assignment_ahead(lex, 5);
        int status;
        if (reference) {
            status = kiln_dyn_eval_delete_member(host, lex, scope, receiver, args, argc,
                                                 depth, out, thrown);
        } else if (lex->curr.kind == KILN_DYN_LPAREN) {
            status = kiln_dyn_eval_delete_operand(host, lex, scope, receiver, args, argc,
                                                  depth, out, thrown);
            reference = 1;
        } else if (lex->curr.kind == KILN_DYN_IDENT) {
            kiln_dyn_lexer look;
            memset(&look, 0, sizeof look);
            look.src = lex->src;
            look.len = lex->len;
            look.pos = (size_t)(lex->curr.start - lex->src) + lex->curr.len;
            look.dynamic_function_body = lex->dynamic_function_body;
            look.strict = lex->strict;
            look.prev_kind = lex->curr.kind;
            kiln_dyn_lexer_next(&look);
            int identifier_reference = look.curr.kind == KILN_DYN_RPAREN;
            kiln_dyn_token_free(&look.curr);
            if (identifier_reference) {
                status = kiln_dyn_eval_delete_identifier(host, lex, scope, depth, out, thrown);
                reference = 1;
            } else {
                status = kiln_dyn_eval_expr(host, lex, scope, receiver, args, argc, depth,
                                            out, thrown);
            }
        } else {
            status = kiln_dyn_eval_expr(host, lex, scope, receiver, args, argc, depth,
                                        out, thrown);
        }
        if (status != KILN_C99_OK) return status;
        if (lex->curr.kind != KILN_DYN_RPAREN)
            return kiln_dyn_throw_parse_error(lex, thrown,
                                              "Expected ')' after delete operand");
        kiln_dyn_lexer_next(lex);
        if (!reference) *out = kiln_c99_boolean(1);
        return KILN_C99_OK;
    }
    if (lex->curr.kind == KILN_DYN_IDENT) {
        kiln_dyn_lexer look;
        memset(&look, 0, sizeof look);
        look.src = lex->src;
        look.len = lex->len;
        look.pos = (size_t)(lex->curr.start - lex->src) + lex->curr.len;
        look.dynamic_function_body = lex->dynamic_function_body;
        look.strict = lex->strict;
        look.prev_kind = lex->curr.kind;
        kiln_dyn_lexer_next(&look);
        int call_expression = look.curr.kind == KILN_DYN_LPAREN;
        kiln_dyn_token_free(&look.curr);
        if (call_expression) {
            int status = kiln_dyn_eval_unary(host, lex, scope, receiver, args, argc,
                                             depth, out, thrown);
            if (status == KILN_C99_OK) *out = kiln_c99_boolean(1);
            return status;
        }
        return kiln_dyn_eval_delete_identifier(host, lex, scope, depth, out, thrown);
    }
    int status = kiln_dyn_eval_unary(host, lex, scope, receiver, args, argc, depth,
                                     out, thrown);
    if (status == KILN_C99_OK) *out = kiln_c99_boolean(1);
    return status;
}

static int kiln_dyn_eval_assignment(kiln_host *host, kiln_dyn_lexer *lex, kiln_dyn_scope *scope,
                                    kiln_value receiver, const kiln_value *args, size_t argc,
                                    int depth, kiln_value *out, kiln_value *thrown) {
    if (kiln_dyn_member_assignment_ahead(lex, 0))
        return kiln_dyn_eval_member_assignment(host, lex, scope, receiver, args, argc, depth,
                                               out, thrown, KILN_DYN_EOF);
    if (lex->curr.kind == KILN_DYN_IDENT) {
        const char *name = lex->curr.start;
        size_t name_len = lex->curr.len;
        size_t saved_pos = lex->pos;
        kiln_dyn_token saved_tok = lex->curr;

        kiln_dyn_lexer_next(lex);
        if (lex->curr.kind == KILN_DYN_ASSIGN ||
                   lex->curr.kind == KILN_DYN_PLUS_ASSIGN ||
                   lex->curr.kind == KILN_DYN_MINUS_ASSIGN ||
                   lex->curr.kind == KILN_DYN_STAR_ASSIGN ||
                   lex->curr.kind == KILN_DYN_SLASH_ASSIGN ||
                   lex->curr.kind == KILN_DYN_PERCENT_ASSIGN ||
                   lex->curr.kind == KILN_DYN_LSHIFT_ASSIGN ||
                   lex->curr.kind == KILN_DYN_RSHIFT_ASSIGN ||
                   lex->curr.kind == KILN_DYN_URSHIFT_ASSIGN ||
                   lex->curr.kind == KILN_DYN_BIT_AND_ASSIGN ||
                   lex->curr.kind == KILN_DYN_BIT_OR_ASSIGN ||
                   lex->curr.kind == KILN_DYN_BIT_XOR_ASSIGN) {
            kiln_dyn_token_kind assignment = lex->curr.kind;
            kiln_dyn_lexer_next(lex);
            kiln_value current = kiln_c99_undefined();
            kiln_value with_target = kiln_c99_undefined();
            kiln_value global_target = kiln_c99_undefined();
            kiln_dyn_scope *binding_scope = NULL;
            kiln_dyn_update_reference reference = KILN_DYN_UPDATE_GLOBAL;
            int unresolvable = 0;
            int status = kiln_dyn_resolve_update_reference(
                host, scope, name, name_len, depth, 1, assignment != KILN_DYN_ASSIGN,
                &current, &reference, &binding_scope, &with_target, &global_target,
                &unresolvable, thrown);
            if (status != KILN_C99_OK) return status;
            kiln_c99_gc_keep_alive(&current);
            kiln_c99_gc_keep_alive(&with_target);
            kiln_c99_gc_keep_alive(&global_target);
            kiln_value rhs;
            status = kiln_dyn_eval_expr(host, lex, scope, receiver, args, argc, depth, &rhs,
                                        thrown);
            if (status != KILN_C99_OK) return status;
            if (assignment != KILN_DYN_ASSIGN) {
                switch (assignment) {
                case KILN_DYN_PLUS_ASSIGN: rhs = kiln_c99_binary(1, current, rhs); break;
                case KILN_DYN_MINUS_ASSIGN: rhs = kiln_c99_binary(2, current, rhs); break;
                case KILN_DYN_STAR_ASSIGN: rhs = kiln_c99_binary(3, current, rhs); break;
                case KILN_DYN_SLASH_ASSIGN: rhs = kiln_c99_binary(4, current, rhs); break;
                case KILN_DYN_PERCENT_ASSIGN: rhs = kiln_c99_binary(5, current, rhs); break;
                case KILN_DYN_LSHIFT_ASSIGN: rhs = kiln_c99_binary(20, current, rhs); break;
                case KILN_DYN_RSHIFT_ASSIGN: rhs = kiln_c99_binary(21, current, rhs); break;
                case KILN_DYN_URSHIFT_ASSIGN: rhs = kiln_c99_binary(22, current, rhs); break;
                case KILN_DYN_BIT_AND_ASSIGN: rhs = kiln_c99_binary(17, current, rhs); break;
                case KILN_DYN_BIT_OR_ASSIGN: rhs = kiln_c99_binary(18, current, rhs); break;
                case KILN_DYN_BIT_XOR_ASSIGN: rhs = kiln_c99_binary(19, current, rhs); break;
                default: break;
                }
            }
            int ignore_write = 0;
            if (reference == KILN_DYN_UPDATE_BINDING) {
                status = kiln_dyn_scope_check_write(binding_scope, name, name_len,
                                                    &ignore_write, thrown);
                if (status != KILN_C99_OK) return status;
            }
            if (!ignore_write) {
                if (reference == KILN_DYN_UPDATE_BINDING) {
                    if (!kiln_dyn_scope_update(binding_scope, name, name_len, rhs))
                        return KILN_C99_OUT_OF_MEMORY;
                } else if (reference == KILN_DYN_UPDATE_WITH) {
                    status = kiln_c99_put(host, with_target, name, name_len, rhs, 0, thrown);
                    if (status != KILN_C99_OK) return status;
                } else {
                    if (unresolvable && scope->strict)
                        return kiln_dyn_throw_unresolvable_reference(thrown, name, name_len);
                    status = kiln_c99_put(host, global_target, name, name_len, rhs, 0, thrown);
                    if (status != KILN_C99_OK) return status;
                }
            }
            *out = rhs;
            return KILN_C99_OK;
        } else if (lex->curr.kind == KILN_DYN_DOT) {
            kiln_dyn_lexer_next(lex);
            if (kiln_dyn_is_ident_name(lex->curr.kind)) {
                const char *prop = lex->curr.start;
                size_t prop_len = lex->curr.len;
                kiln_dyn_lexer_next(lex);
                if (lex->curr.kind == KILN_DYN_ASSIGN) {
                    kiln_dyn_lexer_next(lex);
                    kiln_value rhs;
                    int status = kiln_dyn_eval_expr(host, lex, scope, receiver, args, argc, depth, &rhs, thrown);
                    if (status != KILN_C99_OK) return status;
                    kiln_value target;
                    int scope_found = 0;
                    status = kiln_dyn_scope_get(host, scope, name, name_len, depth, &target,
                                                &scope_found, thrown);
                    if (status != KILN_C99_OK) return status;
                    if (!scope_found) {
                        kiln_value globals = kiln_c99_undefined();
                        status = kiln_c99_global_object(&globals);
                        if (status != KILN_C99_OK) return status;
                        status = kiln_c99_get(host, globals, name, name_len, &target, depth,
                                              thrown);
                        if (status != KILN_C99_OK) return status;
                    }
                    status = kiln_c99_put(host, target, prop, prop_len, rhs, 0, thrown);
                    if (status != KILN_C99_OK) return status;
                    *out = rhs;
                    return KILN_C99_OK;
                }
            }
        } else if (lex->curr.kind == KILN_DYN_LBRACKET) {
            kiln_dyn_lexer_next(lex);
            kiln_value idx;
            int status = kiln_dyn_eval_expr(host, lex, scope, receiver, args, argc, depth, &idx, thrown);
            if (status != KILN_C99_OK) return status;
            if (lex->curr.kind == KILN_DYN_RBRACKET) {
                kiln_dyn_lexer_next(lex);
                if (lex->curr.kind == KILN_DYN_ASSIGN) {
                    kiln_dyn_lexer_next(lex);
                    kiln_value rhs;
                    status = kiln_dyn_eval_expr(host, lex, scope, receiver, args, argc, depth, &rhs, thrown);
                    if (status != KILN_C99_OK) return status;
                    kiln_value target;
                    int scope_found = 0;
                        status = kiln_dyn_scope_get(host, scope, name, name_len, depth,
                                                    &target, &scope_found, thrown);
                    if (status != KILN_C99_OK) return status;
                    if (!scope_found) {
                        kiln_value globals = kiln_c99_undefined();
                        status = kiln_c99_global_object(&globals);
                        if (status != KILN_C99_OK) return status;
                        status = kiln_c99_get(host, globals, name, name_len, &target, depth,
                                              thrown);
                        if (status != KILN_C99_OK) return status;
                    }
                    status = kiln_c99_set_computed(host, target, idx, rhs, depth, thrown);
                    if (status != KILN_C99_OK) return status;
                    *out = rhs;
                    return KILN_C99_OK;
                }
            }
        }
        lex->pos = saved_pos;
        lex->curr = saved_tok;
    }
    return kiln_dyn_eval_ternary(host, lex, scope, receiver, args, argc, depth, out, thrown);
}

static int kiln_dyn_eval_expr(kiln_host *host, kiln_dyn_lexer *lex, kiln_dyn_scope *scope,
                              kiln_value receiver, const kiln_value *args, size_t argc,
                              int depth, kiln_value *out, kiln_value *thrown) {
    int status = kiln_dyn_eval_assignment(host, lex, scope, receiver, args, argc, depth,
                                          out, thrown);
    if (status != KILN_C99_OK) return status;
    if (lex->allocation_error) return KILN_C99_OUT_OF_MEMORY;
    if (lex->curr.kind == KILN_DYN_ERROR)
        return kiln_dyn_throw_unexpected_token(lex, thrown);
    return KILN_C99_OK;
}

/* Direct eval cannot compile a new native body at run time. It can evaluate one expression with
 * the same cell-backed lexical environment the dynamic Function trampoline understands. This
 * supports expression forms the dynamic evaluator implements (including invoking a function
 * expression); statement eval still needs declaration-instantiation and is not approximated here.
 * Names arrive in EvalSite order, innermost first; duplicate spellings are ignored after their
 * first binding. */
int kiln_c99_dynamic_eval_expression(kiln_host *host, kiln_value source,
                                              const char *const *names,
                                              const size_t *name_lengths,
                                              const kiln_value *values,
                                              const unsigned char *read_only_flags,
                                              size_t binding_count,
                                              int strict_context, kiln_value *out, int depth,
                                              kiln_value *thrown) {
    kiln_dyn_scope scope;
    uint8_t *source_owned = NULL;
    size_t source_len = 0;
    kiln_dyn_lexer lex;
    kiln_value completion = kiln_c99_undefined();
    int status;
    if (out == NULL || source.tag != KILN_VALUE_STRING ||
        (binding_count != 0 && (names == NULL || name_lengths == NULL || values == NULL ||
                                read_only_flags == NULL)))
        return KILN_C99_INVALID_ARGUMENT;
    status = kiln_dyn_string_to_source(source, &source_owned, &source_len);
    if (status != KILN_C99_OK) return status;
    kiln_dyn_scope_init(&scope, NULL);
    for (size_t index = 0; index < binding_count; ++index) {
        if (names[index] == NULL || values[index].tag != KILN_VALUE_CELL) {
            status = KILN_C99_INVALID_ARGUMENT;
            goto direct_eval_done;
        }
        int shadowed = 0;
        for (size_t prior = 0; prior < scope.count; ++prior) {
            if (scope.bindings[prior].name_len == name_lengths[index] &&
                memcmp(scope.bindings[prior].name, names[index], name_lengths[index]) == 0) {
                shadowed = 1;
                break;
            }
        }
        if (shadowed) continue;
        if (!kiln_dyn_scope_set(&scope, names[index], name_lengths[index], values[index])) {
            status = KILN_C99_OUT_OF_MEMORY;
            goto direct_eval_done;
        }
        if (read_only_flags[index] == 1)
            (void)kiln_dyn_scope_mark_restriction(&scope, names[index], name_lengths[index],
                                                  KILN_DYN_BINDING_READ_ONLY);
        else if (read_only_flags[index] == 2)
            (void)kiln_dyn_scope_mark_restriction(&scope, names[index], name_lengths[index],
                                                  KILN_DYN_BINDING_FUNCTION_NAME);
    }
    scope.strict = strict_context != 0;
    memset(&lex, 0, sizeof(lex));
    lex.src = (const char *)source_owned;
    lex.len = source_len;
    lex.strict = scope.strict;
    kiln_dyn_lexer_next(&lex);
    if (lex.curr.kind == KILN_DYN_EOF) {
        *out = kiln_c99_undefined();
        goto direct_eval_done;
    }
    status = kiln_dyn_eval_expr(host, &lex, &scope, kiln_c99_undefined(), NULL, 0,
                                depth, &completion, thrown);
    if (status == KILN_C99_OK && lex.curr.kind != KILN_DYN_EOF)
        goto direct_eval_unsupported;
    if (status == KILN_C99_OK) {
        *out = completion;
    }
    kiln_dyn_token_free(&lex.curr);
    goto direct_eval_done;
direct_eval_unsupported:
    kiln_dyn_token_free(&lex.curr);
    kiln_dyn_scope_destroy(&scope);
    free(source_owned);
    KILN_C99_REFUSE("unsupported direct eval source (expected supported single expression)");
direct_eval_done:
    kiln_dyn_scope_destroy(&scope);
    free(source_owned);
    return status;
}

static int kiln_dyn_capture_block(kiln_dyn_lexer *lex, const char **body, size_t *body_len) {
    if (lex->curr.kind != KILN_DYN_LBRACE) return 0;
    size_t start = lex->pos;
    kiln_dyn_lexer scan;
    memset(&scan, 0, sizeof scan);
    scan.src = lex->src;
    scan.len = lex->len;
    scan.pos = start;
    scan.prev_kind = KILN_DYN_LBRACE;
    kiln_dyn_lexer_next(&scan);
    int nesting = 1;
    size_t close_start = start;
    while (scan.curr.kind != KILN_DYN_EOF) {
        if (scan.curr.kind == KILN_DYN_LBRACE) {
            nesting++;
        } else if (scan.curr.kind == KILN_DYN_RBRACE) {
            nesting--;
            if (nesting == 0) {
                close_start = (size_t)(scan.curr.start - scan.src);
                break;
            }
        }
        kiln_dyn_lexer_next(&scan);
    }
    if (nesting != 0) {
        kiln_dyn_token_free(&scan.curr);
        return 0;
    }
    size_t end = scan.pos;
    kiln_dyn_token_free(&scan.curr);
    *body = &lex->src[start];
    *body_len = close_start - start;
    lex->pos = end;
    /* The closing brace is the previous lexical token for the token after this body. */
    lex->curr.kind = KILN_DYN_RBRACE;
    kiln_dyn_lexer_next(lex);
    return 1;
}

static const char *kiln_dyn_expression_syntax_end(const char *source, size_t start,
                                                 size_t end, int *allocation_error) {
    kiln_dyn_lexer scan;
    const char *syntax_end = NULL;
    if (allocation_error != NULL) *allocation_error = 0;
    memset(&scan, 0, sizeof(scan));
    scan.src = source;
    scan.len = end;
    scan.pos = start;
    scan.curr.kind = KILN_DYN_EOF;
    while (scan.pos < scan.len) {
        size_t previous_pos = scan.pos;
        kiln_dyn_lexer_next(&scan);
        if (scan.curr.kind == KILN_DYN_EOF) break;
        if (scan.allocation_error) {
            if (allocation_error != NULL) *allocation_error = 1;
            syntax_end = NULL;
            break;
        }
        if (scan.curr.kind == KILN_DYN_ERROR || scan.pos <= previous_pos) {
            syntax_end = NULL;
            break;
        }
        syntax_end = scan.curr.start + scan.curr.len;
    }
    kiln_dyn_token_free(&scan.curr);
    return syntax_end;
}

static int kiln_dyn_capture_expression(kiln_dyn_lexer *lex, const char **body,
                                       size_t *body_len, const char **syntax_end) {
    size_t start = lex->curr.start == NULL
                       ? lex->pos
                       : (size_t)(lex->curr.start - lex->src);
    size_t pos = start;
    int parens = 0, brackets = 0, braces = 0;
    while (pos < lex->len) {
        char c = lex->src[pos];
        if (c == '\'' || c == '"' || c == '`') {
            char quote = c;
            pos++;
            while (pos < lex->len) {
                c = lex->src[pos++];
                if (c == '\\' && pos < lex->len) pos++;
                else if (c == quote) break;
            }
            continue;
        }
        if (parens == 0 && brackets == 0 && braces == 0 &&
            (c == ',' || c == ';' || c == ')' || c == ']' || c == '}')) break;
        if (c == '(') parens++;
        else if (c == ')' && parens > 0) parens--;
        else if (c == '[') brackets++;
        else if (c == ']' && brackets > 0) brackets--;
        else if (c == '{') braces++;
        else if (c == '}' && braces > 0) braces--;
        pos++;
    }
    size_t end = pos;
    while (end > start && (lex->src[end - 1] == ' ' || lex->src[end - 1] == '\t' ||
                           lex->src[end - 1] == '\r' || lex->src[end - 1] == '\n')) end--;
    while (start < end && (lex->src[start] == ' ' || lex->src[start] == '\t' ||
                           lex->src[start] == '\r' || lex->src[start] == '\n')) start++;
    if (start == end) return 0;
    *body = &lex->src[start];
    *body_len = end - start;
    if (syntax_end != NULL) {
        int allocation_error = 0;
        *syntax_end = kiln_dyn_expression_syntax_end(lex->src, start, end,
                                                      &allocation_error);
        if (allocation_error) lex->allocation_error = 1;
        if (*syntax_end == NULL) return 0;
    }
    lex->pos = pos;
    kiln_dyn_lexer_next(lex);
    return 1;
}

static int kiln_dyn_make_closure(kiln_host *host, kiln_dyn_scope *scope,
                                 const char *name, size_t name_len,
                                 const char *params, size_t params_len,
                                 const char *body, size_t body_len,
                                 const char *reflection_source, size_t reflection_source_len,
                                 kiln_value receiver,
                                 int asynchronous, int arrow, int method, kiln_value *out,
                                 kiln_value *thrown) {
    kiln_value upvals[3];
    kiln_value env_obj;
    int status = kiln_dyn_new_environment(&env_obj);
    if (status != KILN_C99_OK) return status;
    kiln_c99_gc_keep_alive(&env_obj);
    status = kiln_dyn_capture_environment(host, scope, env_obj, 0, thrown);
    if (status != KILN_C99_OK) return status;
    status = kiln_dyn_source_to_string(params, params_len, &upvals[0]);
    if (status != KILN_C99_OK) return status;
    kiln_c99_gc_keep_alive(&upvals[0]);
    status = kiln_dyn_source_to_string(body, body_len, &upvals[1]);
    if (status != KILN_C99_OK) return status;
    kiln_value reflected_source;
    status = kiln_dyn_source_to_string(reflection_source, reflection_source_len,
                                       &reflected_source);
    if (status != KILN_C99_OK) return status;
    upvals[2] = env_obj;
    if (upvals[0].tag != KILN_VALUE_STRING || upvals[1].tag != KILN_VALUE_STRING)
        return KILN_C99_OUT_OF_MEMORY;
    kiln_c99_gc_keep_alive(&upvals[0]);
    kiln_c99_gc_keep_alive(&upvals[1]);
    kiln_c99_gc_keep_alive(&upvals[2]);
    kiln_c99_gc_keep_alive(&reflected_source);
    kiln_val val_upvals[3] = {
        kiln_val_from_boundary(upvals[0]),
        kiln_val_from_boundary(upvals[1]),
        kiln_val_from_boundary(upvals[2])
    };
    kiln_value name_value = name == NULL ? kiln_c99_undefined()
                                         : kiln_c99_copy_text(name, name_len);
    if (name != NULL && name_value.tag != KILN_VALUE_STRING) return KILN_C99_OUT_OF_MEMORY;
    if (name != NULL) kiln_c99_gc_keep_alive(&name_value);
    const kiln_string *closure_name = name == NULL ? NULL : (const kiln_string *)name_value.ptr;
    status = kiln_c99_make_closure(out, kiln_c99_dynamic_body_trampoline, closure_name,
                                   "", 0, "eval", 0, 0, 0, asynchronous, 0,
                                   method ? KILN_C99_FUNCTION_METHOD : 0, arrow, 0,
                                   arrow ? kiln_val_from_boundary(receiver) : KILN_VAL_UNDEFINED,
                                   KILN_VAL_UNDEFINED, 0, 16, 0, 0, val_upvals, 3);
    kiln_c99_gc_keep_alive(&reflected_source);
    kiln_c99_gc_keep_alive(&name_value);
    if (status == KILN_C99_OK) {
        kiln_c99_heap_store_value(kiln_c99_closure_of(*out),
                                  &kiln_c99_closure_of(*out)->source_value,
                                  reflected_source);
    }
    return status;
}

static int kiln_dyn_make_arrow_from_tail(kiln_host *host, kiln_dyn_lexer *lex,
                                         kiln_dyn_scope *scope, const char *params,
                                         size_t params_len, const char *source_start,
                                         kiln_value receiver, int asynchronous,
                                         int depth, kiln_value *out, kiln_value *thrown) {
    const char *body;
    size_t body_len;
    if (lex->curr.kind == KILN_DYN_LBRACE) {
        if (!kiln_dyn_capture_block(lex, &body, &body_len))
            return kiln_dyn_throw_parse_error(lex, thrown,
                                        "Unterminated arrow function body");
        const char *source_end = body + body_len + 1;
        return kiln_dyn_make_closure(host, scope, NULL, 0, params, params_len, body,
                                     body_len, source_start,
                                     (size_t)(source_end - source_start), receiver,
                                     asynchronous, 1, 0, out, thrown);
    }
    const char *syntax_end;
    if (!kiln_dyn_capture_expression(lex, &body, &body_len, &syntax_end)) {
        if (lex->allocation_error) return KILN_C99_OUT_OF_MEMORY;
        return kiln_dyn_throw_parse_error(lex, thrown,
                                    "Expected arrow function body");
    }
    const char *source_end = syntax_end;
    char *return_body = (char *)malloc(body_len + sizeof "return ;");
    if (return_body == NULL) return KILN_C99_OUT_OF_MEMORY;
    memcpy(return_body, "return ", 7);
    memcpy(return_body + 7, body, body_len);
    return_body[7 + body_len] = ';';
    int status = kiln_dyn_make_closure(host, scope, NULL, 0, params, params_len,
                                       return_body, body_len + 8, source_start,
                                       (size_t)(source_end - source_start), receiver,
                                       asynchronous, 1, 0, out, thrown);
    free(return_body);
    (void)depth;
    return status;
}

static int kiln_dyn_parenthesized_arrow_params(kiln_dyn_lexer *lex, const char **params,
                                               size_t *params_len, size_t *after_close) {
    size_t start = lex->pos;
    size_t pos = start;
    int nesting = 1;
    while (pos < lex->len && nesting > 0) {
        char c = lex->src[pos++];
        if (c == '\'' || c == '"' || c == '`') {
            char quote = c;
            while (pos < lex->len) {
                c = lex->src[pos++];
                if (c == '\\' && pos < lex->len) pos++;
                else if (c == quote) break;
            }
        } else if (c == '(') {
            nesting++;
        } else if (c == ')') {
            nesting--;
        }
    }
    if (nesting != 0) return 0;
    size_t close = pos - 1;
    size_t next = pos;
    while (next < lex->len && (lex->src[next] == ' ' || lex->src[next] == '\t' ||
                               lex->src[next] == '\r' || lex->src[next] == '\n')) next++;
    if (next + 1 >= lex->len || lex->src[next] != '=' || lex->src[next + 1] != '>') return 0;
    *params = &lex->src[start];
    *params_len = close - start;
    *after_close = pos;
    return 1;
}

/* Skip one complete statement, including a loop or nested if used as an unbraced branch.
 * Stopping at the first semicolon can leave the closing loop body in the outer parser. */
static void kiln_dyn_skip_statement(kiln_dyn_lexer *lex) {
    /* Labels are statement prefixes, not expressions.  A loop-body capture must skip the
     * whole labeled statement or it can stop at a semicolon in the wrong statement and
     * leave the loop's closing delimiter in the caller's lexer.  Probe with an independent
     * lexer so the live token (and any decoded string storage it owns) stays untouched. */
    size_t label_count = 0;
    kiln_dyn_lexer look;
    memset(&look, 0, sizeof look);
    look.src = lex->src;
    look.len = lex->len;
    look.pos = lex->curr.start == NULL ? lex->pos
                                       : (size_t)(lex->curr.start - lex->src);
    kiln_dyn_lexer_next(&look);
    while (look.curr.kind == KILN_DYN_IDENT) {
        kiln_dyn_lexer_next(&look);
        if (look.curr.kind != KILN_DYN_COLON) break;
        label_count++;
        kiln_dyn_lexer_next(&look);
    }
    kiln_dyn_token_free(&look.curr);
    if (label_count != 0) {
        for (size_t i = 0; i < label_count * 2; i++) kiln_dyn_lexer_next(lex);
        kiln_dyn_skip_statement(lex);
        return;
    }
    if (lex->curr.kind == KILN_DYN_SEMI) { kiln_dyn_lexer_next(lex); return; }
    if (lex->curr.kind == KILN_DYN_LBRACE) {
        int braces = 1;
        kiln_dyn_lexer_next(lex);
        while (braces != 0 && lex->curr.kind != KILN_DYN_EOF) {
            if (lex->curr.kind == KILN_DYN_LBRACE) braces++;
            else if (lex->curr.kind == KILN_DYN_RBRACE) braces--;
            kiln_dyn_lexer_next(lex);
        }
        return;
    }
    if (lex->curr.kind == KILN_DYN_IDENT && lex->curr.len == 2 &&
        memcmp(lex->curr.start, "do", 2) == 0) {
        kiln_dyn_lexer_next(lex);
        kiln_dyn_skip_statement(lex);
        if (lex->curr.kind == KILN_DYN_IDENT && lex->curr.len == 5 &&
            memcmp(lex->curr.start, "while", 5) == 0) {
            kiln_dyn_lexer_next(lex);
            if (lex->curr.kind == KILN_DYN_LPAREN) {
                int parens = 1;
                kiln_dyn_lexer_next(lex);
                while (parens != 0 && lex->curr.kind != KILN_DYN_EOF) {
                    if (lex->curr.kind == KILN_DYN_LPAREN) parens++;
                    else if (lex->curr.kind == KILN_DYN_RPAREN) parens--;
                    if (parens != 0) kiln_dyn_lexer_next(lex);
                }
                if (lex->curr.kind == KILN_DYN_RPAREN) kiln_dyn_lexer_next(lex);
                if (lex->curr.kind == KILN_DYN_SEMI) kiln_dyn_lexer_next(lex);
            }
        }
        return;
    }
    int conditional = lex->curr.kind == KILN_DYN_KW_IF;
    int switch_stmt = lex->curr.kind == KILN_DYN_KW_SWITCH;
    int loop = lex->curr.kind == KILN_DYN_KW_FOR || lex->curr.kind == KILN_DYN_KW_WITH ||
        (lex->curr.kind == KILN_DYN_IDENT && lex->curr.len == 5 &&
         memcmp(lex->curr.start, "while", 5) == 0);
    if (conditional || loop || switch_stmt) {
        kiln_dyn_lexer_next(lex);
        if (lex->curr.kind == KILN_DYN_LPAREN) {
            int parens = 1;
            kiln_dyn_lexer_next(lex);
            while (parens != 0 && lex->curr.kind != KILN_DYN_EOF) {
                if (lex->curr.kind == KILN_DYN_LPAREN) parens++;
                else if (lex->curr.kind == KILN_DYN_RPAREN) parens--;
                kiln_dyn_lexer_next(lex);
            }
        }
        if (switch_stmt) {
            kiln_dyn_skip_statement(lex);
            return;
        }
        kiln_dyn_skip_statement(lex);
        if (conditional && lex->curr.kind == KILN_DYN_KW_ELSE) {
            kiln_dyn_lexer_next(lex);
            kiln_dyn_skip_statement(lex);
        }
        return;
    }
    int parens = 0, brackets = 0, braces = 0;
    while (lex->curr.kind != KILN_DYN_EOF) {
        if (parens == 0 && brackets == 0 && braces == 0) {
            if (lex->curr.kind == KILN_DYN_RBRACE || lex->curr.kind == KILN_DYN_KW_ELSE) return;
            if (lex->curr.kind == KILN_DYN_SEMI) { kiln_dyn_lexer_next(lex); return; }
        }
        if (lex->curr.kind == KILN_DYN_LPAREN) parens++;
        else if (lex->curr.kind == KILN_DYN_RPAREN && parens > 0) parens--;
        else if (lex->curr.kind == KILN_DYN_LBRACKET) brackets++;
        else if (lex->curr.kind == KILN_DYN_RBRACKET && brackets > 0) brackets--;
        else if (lex->curr.kind == KILN_DYN_LBRACE) braces++;
        else if (lex->curr.kind == KILN_DYN_RBRACE && braces > 0) braces--;
        kiln_dyn_lexer_next(lex);
    }
}

typedef struct {
    const char *test;
    size_t test_len;
    const char *body;
    size_t body_len;
    int is_default;
} kiln_dyn_switch_clause;

/* Inventory switch clauses without evaluating their tests. This preserves source order and
 * lets execution evaluate case expressions only until the first strict match. */
static int kiln_dyn_switch_clauses(const char *source, size_t length,
                                   kiln_dyn_switch_clause **out, size_t *out_count) {
    *out = NULL;
    *out_count = 0;
    size_t capacity = 0;
    kiln_dyn_lexer scan;
    memset(&scan, 0, sizeof scan);
    scan.src = source;
    scan.len = length;
    kiln_dyn_lexer_next(&scan);
    while (scan.curr.kind != KILN_DYN_EOF) {
        int is_default = scan.curr.kind == KILN_DYN_KW_DEFAULT;
        if (!is_default && scan.curr.kind != KILN_DYN_KW_CASE) goto malformed;
        kiln_dyn_token_free(&scan.curr);
        kiln_dyn_lexer_next(&scan);
        const char *test = NULL;
        size_t test_len = 0;
        if (!is_default) {
            if (scan.curr.kind == KILN_DYN_COLON || scan.curr.kind == KILN_DYN_EOF)
                goto malformed;
            test = scan.curr.start;
            size_t test_start = (size_t)(test - source);
            int parens = 0, brackets = 0, braces = 0, conditional = 0;
            for (;;) {
                if (scan.curr.kind == KILN_DYN_EOF) goto malformed;
                if (parens == 0 && brackets == 0 && braces == 0) {
                    if (scan.curr.kind == KILN_DYN_QUESTION) conditional++;
                    else if (scan.curr.kind == KILN_DYN_COLON) {
                        if (conditional != 0) conditional--;
                        else break;
                    }
                }
                if (scan.curr.kind == KILN_DYN_LPAREN) parens++;
                else if (scan.curr.kind == KILN_DYN_RPAREN && parens > 0) parens--;
                else if (scan.curr.kind == KILN_DYN_LBRACKET) brackets++;
                else if (scan.curr.kind == KILN_DYN_RBRACKET && brackets > 0) brackets--;
                else if (scan.curr.kind == KILN_DYN_LBRACE) braces++;
                else if (scan.curr.kind == KILN_DYN_RBRACE && braces > 0) braces--;
                kiln_dyn_lexer_next(&scan);
            }
            test_len = (size_t)(scan.curr.start - source) - test_start;
        } else if (scan.curr.kind != KILN_DYN_COLON) {
            goto malformed;
        }
        if (scan.curr.kind != KILN_DYN_COLON) goto malformed;
        kiln_dyn_lexer_next(&scan);
        const char *body = scan.curr.start != NULL ? scan.curr.start : source + length;
        int parens = 0, brackets = 0, braces = 0;
        while (scan.curr.kind != KILN_DYN_EOF) {
            if (parens == 0 && brackets == 0 && braces == 0 &&
                (scan.curr.kind == KILN_DYN_KW_CASE || scan.curr.kind == KILN_DYN_KW_DEFAULT) &&
                scan.prev_kind != KILN_DYN_DOT) break;
            if (scan.curr.kind == KILN_DYN_LPAREN) parens++;
            else if (scan.curr.kind == KILN_DYN_RPAREN && parens > 0) parens--;
            else if (scan.curr.kind == KILN_DYN_LBRACKET) brackets++;
            else if (scan.curr.kind == KILN_DYN_RBRACKET && brackets > 0) brackets--;
            else if (scan.curr.kind == KILN_DYN_LBRACE) braces++;
            else if (scan.curr.kind == KILN_DYN_RBRACE && braces > 0) braces--;
            kiln_dyn_lexer_next(&scan);
        }
        const char *body_end = scan.curr.start != NULL ? scan.curr.start : source + length;
        if (*out_count == capacity) {
            size_t next = capacity == 0 ? 4 : capacity * 2;
            kiln_dyn_switch_clause *grown = realloc(*out, next * sizeof **out);
            if (grown == NULL) goto oom;
            *out = grown;
            capacity = next;
        }
        (*out)[(*out_count)++] = (kiln_dyn_switch_clause){
            .test = test, .test_len = test_len, .body = body,
            .body_len = (size_t)(body_end - body), .is_default = is_default,
        };
    }
    kiln_dyn_token_free(&scan.curr);
    return 1;
malformed:
    kiln_dyn_token_free(&scan.curr);
    free(*out);
    *out = NULL;
    *out_count = 0;
    return 0;
oom:
    kiln_dyn_token_free(&scan.curr);
    free(*out);
    *out = NULL;
    *out_count = 0;
    return -1;
}

typedef struct {
    const char *name;
    size_t name_len;
    int is_const;
    int is_function;
    int is_annex_b;
    const char *function_site;
    size_t clause_index;
    int annex_b_alias;
} kiln_dyn_switch_binding;

/* Switch lexical declarations belong to one scope spanning every case body. Inventory only
 * their names here: initializers execute only if their declaration statement is reached. */
typedef struct {
    const char *name;
    size_t name_len;
} kiln_dyn_switch_var;

/* Shared callable-boundary state for declaration inventories. Both function hoisting and
 * switch CaseBlock validation must ignore declarations in nested callable bodies. */
static int kiln_dyn_callable_scan_advance(kiln_dyn_lexer *scan,
                                           kiln_dyn_token_kind previous,
                                           int *brace_depth, int *callable_depth,
                                           int *paren_depth, int *function_header,
                                           int *function_paren_depth,
                                           int *pending_callable_body,
                                           int *previous_previous_is_method_key,
                                           int *previous_is_control_head,
                                           int **callable_braces, size_t *callable_capacity,
                                           unsigned char **callable_parens,
                                           size_t *paren_capacity) {
    kiln_dyn_token_kind kind = scan->curr.kind;
    if (kind == KILN_DYN_KW_FUNCTION) {
        *function_header = 1;
        *function_paren_depth = -1;
    } else if (kind == KILN_DYN_LPAREN) {
        int method_candidate = *previous_previous_is_method_key && !*previous_is_control_head &&
            previous != KILN_DYN_KW_IF && previous != KILN_DYN_KW_FOR &&
            previous != KILN_DYN_KW_WITH;
        if ((size_t)*paren_depth >= *paren_capacity) {
            size_t capacity = *paren_capacity == 0 ? 16 : *paren_capacity * 2;
            if (capacity <= (size_t)*paren_depth) return -1;
            unsigned char *next = realloc(*callable_parens, capacity);
            if (next == NULL) return -1;
            memset(next + *paren_capacity, 0, capacity - *paren_capacity);
            *callable_parens = next;
            *paren_capacity = capacity;
        }
        (*callable_parens)[(*paren_depth)++] = (unsigned char)method_candidate;
        if (*function_header && *function_paren_depth < 0)
            *function_paren_depth = *paren_depth;
    } else if (kind == KILN_DYN_RPAREN && *paren_depth > 0) {
        int opener = --*paren_depth;
        int method_candidate = (*callable_parens)[opener];
        if (*function_paren_depth == *paren_depth + 1) {
            *pending_callable_body = 1;
            *function_header = 0;
            *function_paren_depth = -1;
        } else if (method_candidate && *paren_depth == 0) {
            *pending_callable_body = 1;
        }
    } else if (kind == KILN_DYN_ARROW) {
        *pending_callable_body = 1;
        *function_header = 0;
    } else if (kind != KILN_DYN_LBRACE) {
        if (*pending_callable_body) *pending_callable_body = 0;
        if (kind != KILN_DYN_KW_ASYNC && kind != KILN_DYN_KW_FUNCTION &&
            kind != KILN_DYN_IDENT && kind != KILN_DYN_STAR)
            *function_header = 0;
    }
    if (kind == KILN_DYN_LBRACE) {
        (*brace_depth)++;
        if (*pending_callable_body) {
            if ((size_t)*callable_depth >= *callable_capacity) {
                size_t capacity = *callable_capacity == 0 ? 16 : *callable_capacity * 2;
                if (capacity <= (size_t)*callable_depth ||
                    capacity > SIZE_MAX / sizeof **callable_braces) return -1;
                int *next = realloc(*callable_braces, capacity * sizeof *next);
                if (next == NULL) return -1;
                *callable_braces = next;
                *callable_capacity = capacity;
            }
            (*callable_braces)[(*callable_depth)++] = *brace_depth;
        }
        *pending_callable_body = 0;
    } else if (kind == KILN_DYN_RBRACE) {
        if (*callable_depth > 0 && (*callable_braces)[*callable_depth - 1] == *brace_depth)
            (*callable_depth)--;
        if (*brace_depth > 0) (*brace_depth)--;
    }
    *previous_previous_is_method_key = kind == KILN_DYN_IDENT || kind == KILN_DYN_STR ||
                                       kind == KILN_DYN_RBRACKET;
    *previous_is_control_head = kind == KILN_DYN_IDENT &&
        ((scan->curr.len == 5 && memcmp(scan->curr.start, "while", 5) == 0) ||
         (scan->curr.len == 6 && memcmp(scan->curr.start, "switch", 6) == 0) ||
         (scan->curr.len == 5 && memcmp(scan->curr.start, "catch", 5) == 0));
    return 1;
}

/* Collect CaseBlock VarDeclaredNames once, sharing one inventory across lexical-name checks.
 * Callable-body tracking follows the function-hoist scanner below so nested functions and
 * methods do not leak their var declarations into the surrounding CaseBlock. */
static int kiln_dyn_switch_vars(const kiln_dyn_switch_clause *clauses, size_t clause_count,
                                kiln_dyn_switch_var **out, size_t *out_count) {
    size_t capacity_vars = 0;
    *out = NULL;
    *out_count = 0;
    for (size_t clause = 0; clause < clause_count; ++clause) {
        const char *source = clauses[clause].body;
        size_t source_len = clauses[clause].body_len;
        kiln_dyn_lexer scan;
        memset(&scan, 0, sizeof scan);
        scan.src = source;
        scan.len = source_len;
        kiln_dyn_lexer_next(&scan);
        int brace_depth = 0, callable_depth = 0, paren_depth = 0;
        int function_header = 0, function_paren_depth = -1, pending_callable_body = 0;
        int previous_previous_is_method_key = 0, previous_is_control_head = 0;
        kiln_dyn_token_kind previous = KILN_DYN_EOF;
        int *callable_braces = NULL;
        size_t callable_capacity = 0;
        unsigned char *callable_parens = NULL;
        size_t paren_capacity = 0;
        while (scan.curr.kind != KILN_DYN_EOF) {
            kiln_dyn_token_kind kind = scan.curr.kind;
            if (kind == KILN_DYN_KW_VAR && callable_depth == 0 &&
                scan.prev_kind != KILN_DYN_DOT) {
                kiln_dyn_lexer look;
                memset(&look, 0, sizeof look);
                look.src = source;
                look.len = source_len;
                look.pos = scan.curr.start == NULL ? scan.pos
                                                    : (size_t)(scan.curr.start - source);
                kiln_dyn_lexer_next(&look);
                kiln_dyn_lexer_next(&look);
                int expect_name = 1, parens = 0, brackets = 0, braces = 0;
                while (look.curr.kind != KILN_DYN_EOF) {
                    if (expect_name && look.curr.kind == KILN_DYN_IDENT) {
                        if (*out_count == capacity_vars) {
                            size_t capacity = capacity_vars == 0 ? 8 : capacity_vars * 2;
                            if (capacity <= capacity_vars || capacity > SIZE_MAX / sizeof **out) {
                                kiln_dyn_token_free(&look.curr);
                                free(callable_braces); free(callable_parens);
                                kiln_dyn_token_free(&scan.curr); free(*out); *out = NULL;
                                *out_count = 0; return -1;
                            }
                            kiln_dyn_switch_var *next = realloc(*out, capacity * sizeof *next);
                            if (next == NULL) {
                                kiln_dyn_token_free(&look.curr);
                                free(callable_braces); free(callable_parens);
                                kiln_dyn_token_free(&scan.curr); free(*out); *out = NULL;
                                *out_count = 0; return -1;
                            }
                            *out = next;
                            capacity_vars = capacity;
                        }
                        (*out)[(*out_count)++] = (kiln_dyn_switch_var){
                            .name = look.curr.start, .name_len = look.curr.len,
                        };
                        expect_name = 0;
                        kiln_dyn_lexer_next(&look);
                        continue;
                    }
                    if (parens == 0 && brackets == 0 && braces == 0) {
                        if (look.curr.kind == KILN_DYN_COMMA) {
                            expect_name = 1;
                            kiln_dyn_lexer_next(&look);
                            continue;
                        }
                        if (look.curr.kind == KILN_DYN_SEMI || look.curr.kind == KILN_DYN_RPAREN ||
                            look.curr.kind == KILN_DYN_RBRACE || look.curr.kind == KILN_DYN_KW_IN ||
                            look.curr.kind == KILN_DYN_KW_OF)
                            break;
                    }
                    if (look.curr.kind == KILN_DYN_LPAREN) parens++;
                    else if (look.curr.kind == KILN_DYN_RPAREN && parens > 0) parens--;
                    else if (look.curr.kind == KILN_DYN_LBRACKET) brackets++;
                    else if (look.curr.kind == KILN_DYN_RBRACKET && brackets > 0) brackets--;
                    else if (look.curr.kind == KILN_DYN_LBRACE) braces++;
                    else if (look.curr.kind == KILN_DYN_RBRACE && braces > 0) braces--;
                    kiln_dyn_lexer_next(&look);
                }
                kiln_dyn_token_free(&look.curr);
            }
            if (kiln_dyn_callable_scan_advance(
                    &scan, previous, &brace_depth, &callable_depth, &paren_depth,
                    &function_header, &function_paren_depth, &pending_callable_body,
                    &previous_previous_is_method_key, &previous_is_control_head,
                    &callable_braces, &callable_capacity, &callable_parens, &paren_capacity) < 0) {
                free(callable_braces); free(callable_parens);
                kiln_dyn_token_free(&scan.curr); free(*out); *out = NULL; *out_count = 0;
                return -1;
            }
            previous = kind;
            kiln_dyn_lexer_next(&scan);
        }
        kiln_dyn_token_free(&scan.curr);
        free(callable_braces);
        free(callable_parens);
    }
    return 1;
}

static int kiln_dyn_switch_bindings(const kiln_dyn_switch_clause *clauses,
                                    size_t clause_count,
                                    kiln_dyn_switch_binding **out,
                                    size_t *out_count) {
    size_t capacity = 0;
    kiln_dyn_switch_var *vars = NULL;
    size_t var_count = 0;
    int vars_status = kiln_dyn_switch_vars(clauses, clause_count, &vars, &var_count);
    if (vars_status < 0) return -1;
    *out = NULL;
    *out_count = 0;
    for (size_t clause = 0; clause < clause_count; ++clause) {
        kiln_dyn_lexer scan;
        memset(&scan, 0, sizeof scan);
        scan.src = clauses[clause].body;
        scan.len = clauses[clause].body_len;
        kiln_dyn_lexer_next(&scan);
        int parens = 0, brackets = 0, braces = 0;
        size_t previous_end = 0;
        while (scan.curr.kind != KILN_DYN_EOF) {
            const char *function_site = scan.curr.start;
            size_t token_start = scan.curr.start == NULL
                                     ? scan.pos
                                     : (size_t)(scan.curr.start - scan.src);
            int line_terminated = previous_end <= token_start && previous_end < scan.len &&
                kiln_dyn_has_line_terminator(scan.src, previous_end, token_start);
            int previous_can_end_statement = scan.prev_kind == KILN_DYN_IDENT ||
                scan.prev_kind == KILN_DYN_NUM || scan.prev_kind == KILN_DYN_BIGINT ||
                scan.prev_kind == KILN_DYN_STR ||
                scan.prev_kind == KILN_DYN_RPAREN || scan.prev_kind == KILN_DYN_RBRACKET ||
                scan.prev_kind == KILN_DYN_KW_RETURN ||
                scan.prev_kind == KILN_DYN_KW_BREAK ||
                scan.prev_kind == KILN_DYN_KW_CONTINUE;
            int direct_function = parens == 0 && brackets == 0 && braces == 0 &&
                (scan.curr.kind == KILN_DYN_KW_FUNCTION ||
                 scan.curr.kind == KILN_DYN_KW_ASYNC) &&
                (scan.prev_kind == KILN_DYN_EOF || scan.prev_kind == KILN_DYN_SEMI ||
                 scan.prev_kind == KILN_DYN_RBRACE ||
                 (line_terminated && previous_can_end_statement));
            if (direct_function && scan.curr.kind == KILN_DYN_KW_ASYNC) {
                kiln_dyn_lexer look = scan;
                size_t async_end = look.pos;
                kiln_dyn_lexer_next(&look);
                direct_function = look.curr.kind == KILN_DYN_KW_FUNCTION &&
                    !kiln_dyn_has_line_terminator(
                        look.src, async_end, (size_t)(look.curr.start - look.src));
                kiln_dyn_token_free(&look.curr);
            }
            if (direct_function) {
                const char *site = function_site;
                int async_declaration = scan.curr.kind == KILN_DYN_KW_ASYNC;
                if (async_declaration) kiln_dyn_lexer_next(&scan);
                kiln_dyn_lexer_next(&scan);
                int generator = scan.curr.kind == KILN_DYN_STAR;
                if (generator) kiln_dyn_lexer_next(&scan);
                if (scan.curr.kind != KILN_DYN_IDENT) {
                    kiln_dyn_token_free(&scan.curr);
                    free(*out); free(vars); *out = NULL; *out_count = 0;
                    return 0;
                }
                const char *name = scan.curr.start;
                size_t name_len = scan.curr.len;
                for (size_t var_index = 0; var_index < var_count; ++var_index) {
                    if (vars[var_index].name_len == name_len &&
                        memcmp(vars[var_index].name, name, name_len) == 0) {
                        kiln_dyn_token_free(&scan.curr);
                        free(*out); free(vars); *out = NULL; *out_count = 0;
                        return 0;
                    }
                }
                for (size_t index = 0; index < *out_count; ++index) {
                    if ((*out)[index].name_len == name_len &&
                        memcmp((*out)[index].name, name, name_len) == 0) {
                        kiln_dyn_token_free(&scan.curr);
                        free(*out); free(vars); *out = NULL; *out_count = 0;
                        return 0;
                    }
                }
                if (*out_count == capacity) {
                    size_t next = capacity == 0 ? 4 : capacity * 2;
                    kiln_dyn_switch_binding *grown = realloc(*out, next * sizeof **out);
                    if (grown == NULL) {
                        kiln_dyn_token_free(&scan.curr);
                        free(*out); free(vars); *out = NULL; *out_count = 0;
                        return -1;
                    }
                    *out = grown;
                    capacity = next;
                }
                (*out)[(*out_count)++] = (kiln_dyn_switch_binding){
                    .name = name, .name_len = name_len, .is_const = 0,
                    .is_function = 1, .is_annex_b = !async_declaration && !generator,
                    .function_site = site,
                    .clause_index = clause,
                    .annex_b_alias = 0,
                };
                kiln_dyn_lexer_next(&scan);
                if (scan.curr.kind != KILN_DYN_LPAREN) {
                    kiln_dyn_token_free(&scan.curr);
                    free(*out); free(vars); *out = NULL; *out_count = 0;
                    return 0;
                }
                int function_parens = 1;
                while (function_parens > 0 && scan.curr.kind != KILN_DYN_EOF) {
                    kiln_dyn_lexer_next(&scan);
                    if (scan.curr.kind == KILN_DYN_LPAREN) function_parens++;
                    else if (scan.curr.kind == KILN_DYN_RPAREN) function_parens--;
                }
                if (function_parens != 0) {
                    kiln_dyn_token_free(&scan.curr);
                    free(*out); free(vars); *out = NULL; *out_count = 0;
                    return 0;
                }
                kiln_dyn_lexer_next(&scan);
                if (scan.curr.kind != KILN_DYN_LBRACE) {
                    kiln_dyn_token_free(&scan.curr);
                    free(*out); free(vars); *out = NULL; *out_count = 0;
                    return 0;
                }
                const char *function_body;
                size_t function_body_len;
                if (!kiln_dyn_capture_block(&scan, &function_body, &function_body_len)) {
                    kiln_dyn_token_free(&scan.curr);
                    free(*out); free(vars); *out = NULL; *out_count = 0;
                    return 0;
                }
                (void)function_body;
                (void)function_body_len;
                if (scan.curr.kind == KILN_DYN_SEMI) kiln_dyn_lexer_next(&scan);
                continue;
            }
            int direct_lexical = parens == 0 && brackets == 0 && braces == 0 &&
                (scan.curr.kind == KILN_DYN_KW_LET || scan.curr.kind == KILN_DYN_KW_CONST) &&
                scan.prev_kind != KILN_DYN_DOT;
            if (direct_lexical) {
                int is_const = scan.curr.kind == KILN_DYN_KW_CONST;
                kiln_dyn_lexer_next(&scan);
                for (;;) {
                    if (scan.curr.kind != KILN_DYN_IDENT) {
                        kiln_dyn_token_free(&scan.curr);
                        free(*out);
                        free(vars);
                        *out = NULL;
                        *out_count = 0;
                        return 0;
                    }
                    const char *name = scan.curr.start;
                    size_t name_len = scan.curr.len;
                    int conflicts_with_var = 0;
                    for (size_t var_index = 0; var_index < var_count; ++var_index) {
                        if (vars[var_index].name_len == name_len &&
                            memcmp(vars[var_index].name, name, name_len) == 0) {
                            conflicts_with_var = 1;
                            break;
                        }
                    }
                    if (conflicts_with_var) {
                        kiln_dyn_token_free(&scan.curr);
                        free(*out);
                        free(vars);
                        *out = NULL;
                        *out_count = 0;
                        return 0;
                    }
                    for (size_t index = 0; index < *out_count; ++index) {
                        if ((*out)[index].name_len == name_len &&
                            memcmp((*out)[index].name, name, name_len) == 0) {
                            kiln_dyn_token_free(&scan.curr);
                            free(*out);
                            free(vars);
                            *out = NULL;
                            *out_count = 0;
                            return 0;
                        }
                    }
                    kiln_dyn_lexer_next(&scan);
                    int has_initializer = scan.curr.kind == KILN_DYN_ASSIGN;
                    if (is_const && !has_initializer) {
                        kiln_dyn_token_free(&scan.curr);
                        free(*out);
                        free(vars);
                        *out = NULL;
                        *out_count = 0;
                        return 0;
                    }
                    if (*out_count == capacity) {
                        size_t next = capacity == 0 ? 4 : capacity * 2;
                        kiln_dyn_switch_binding *grown = realloc(*out, next * sizeof **out);
                        if (grown == NULL) {
                            kiln_dyn_token_free(&scan.curr);
                            free(*out);
                            free(vars);
                            *out = NULL;
                            *out_count = 0;
                            return -1;
                        }
                        *out = grown;
                        capacity = next;
                    }
                    (*out)[(*out_count)++] = (kiln_dyn_switch_binding){
                        .name = name, .name_len = name_len, .is_const = is_const,
                        .is_function = 0, .is_annex_b = 0, .function_site = NULL,
                        .clause_index = clause,
                        .annex_b_alias = 0,
                    };
                    if (has_initializer) {
                        kiln_dyn_lexer_next(&scan);
                        if (scan.curr.kind == KILN_DYN_EOF ||
                            scan.curr.kind == KILN_DYN_COMMA ||
                            scan.curr.kind == KILN_DYN_SEMI) {
                            kiln_dyn_token_free(&scan.curr);
                            free(*out);
                            free(vars);
                            *out = NULL;
                            *out_count = 0;
                            return 0;
                        }
                        int init_parens = 0, init_brackets = 0, init_braces = 0;
                        size_t initializer_previous_end =
                            (size_t)(scan.curr.start - scan.src) + scan.curr.len;
                        while (scan.curr.kind != KILN_DYN_EOF) {
                            size_t token_start = scan.curr.start == NULL
                                                     ? scan.pos
                                                     : (size_t)(scan.curr.start - scan.src);
                            if (init_parens == 0 && init_brackets == 0 && init_braces == 0 &&
                                (scan.curr.kind == KILN_DYN_KW_LET ||
                                 scan.curr.kind == KILN_DYN_KW_CONST ||
                                 scan.curr.kind == KILN_DYN_KW_VAR ||
                                 scan.curr.kind == KILN_DYN_KW_FUNCTION ||
                                 scan.curr.kind == KILN_DYN_KW_ASYNC) &&
                                scan.prev_kind != KILN_DYN_DOT &&
                                kiln_dyn_has_line_terminator(scan.src,
                                    initializer_previous_end, token_start)) {
                                previous_end = initializer_previous_end;
                                break;
                            }
                            if (init_parens == 0 && init_brackets == 0 && init_braces == 0 &&
                                (scan.curr.kind == KILN_DYN_COMMA ||
                                 scan.curr.kind == KILN_DYN_SEMI))
                                break;
                            if (scan.curr.kind == KILN_DYN_LPAREN) init_parens++;
                            else if (scan.curr.kind == KILN_DYN_RPAREN && init_parens > 0)
                                init_parens--;
                            else if (scan.curr.kind == KILN_DYN_LBRACKET) init_brackets++;
                            else if (scan.curr.kind == KILN_DYN_RBRACKET && init_brackets > 0)
                                init_brackets--;
                            else if (scan.curr.kind == KILN_DYN_LBRACE) init_braces++;
                            else if (scan.curr.kind == KILN_DYN_RBRACE && init_braces > 0)
                                init_braces--;
                            initializer_previous_end = token_start + scan.curr.len;
                            kiln_dyn_lexer_next(&scan);
                        }
                    }
                    if (scan.curr.kind != KILN_DYN_COMMA) break;
                    kiln_dyn_lexer_next(&scan);
                }
                continue;
            }
            if (scan.curr.start != NULL)
                previous_end = (size_t)(scan.curr.start - scan.src) + scan.curr.len;
            else
                previous_end = scan.pos;
            if (scan.curr.kind == KILN_DYN_LPAREN) parens++;
            else if (scan.curr.kind == KILN_DYN_RPAREN && parens > 0) parens--;
            else if (scan.curr.kind == KILN_DYN_LBRACKET) brackets++;
            else if (scan.curr.kind == KILN_DYN_RBRACKET && brackets > 0) brackets--;
            else if (scan.curr.kind == KILN_DYN_LBRACE) braces++;
            else if (scan.curr.kind == KILN_DYN_RBRACE && braces > 0) braces--;
            kiln_dyn_lexer_next(&scan);
        }
        kiln_dyn_token_free(&scan.curr);
    }
    free(vars);
    return 1;
}

static int kiln_dyn_switch_predeclare(kiln_dyn_scope *scope,
                                     kiln_dyn_switch_binding *bindings,
                                     size_t count) {
    for (size_t index = 0; index < count; ++index) {
        if (!kiln_dyn_scope_set(scope, bindings[index].name, bindings[index].name_len,
                                kiln_c99_uninitialized()))
            return KILN_C99_OUT_OF_MEMORY;
        if (bindings[index].is_function &&
            !kiln_dyn_scope_add_function_declaration(scope, bindings[index].function_site))
            return KILN_C99_OUT_OF_MEMORY;
        for (size_t slot = 0; slot < scope->count; ++slot) {
            if (scope->bindings[slot].name_len != bindings[index].name_len ||
                memcmp(scope->bindings[slot].name, bindings[index].name,
                       bindings[index].name_len) != 0)
                continue;
            int status = kiln_c99_make_cell(&scope->bindings[slot].value);
            if (status != KILN_C99_OK) return status;
            kiln_value rooted_cell = scope->bindings[slot].value;
            kiln_c99_gc_keep_alive(kiln_c99_gc_value_pointer(rooted_cell));
            status = kiln_c99_put(NULL, scope->roots, scope->bindings[slot].name,
                                  scope->bindings[slot].name_len, rooted_cell, 0, NULL);
            if (status != KILN_C99_OK) return status;
            break;
        }
        if (!bindings[index].is_function &&
            !kiln_dyn_scope_mark_restriction(scope, bindings[index].name,
                bindings[index].name_len,
                bindings[index].is_const ? KILN_DYN_BINDING_READ_ONLY
                                         : KILN_DYN_BINDING_LEXICAL))
            return KILN_C99_OUT_OF_MEMORY;
        if (bindings[index].is_function && bindings[index].is_annex_b &&
            scope->var_target != NULL && !scope->strict) {
            int has_local = 0;
            size_t slot = 0;
            for (; slot < scope->var_target->count; ++slot) {
                if (scope->var_target->bindings[slot].name_len == bindings[index].name_len &&
                    memcmp(scope->var_target->bindings[slot].name, bindings[index].name,
                           bindings[index].name_len) == 0) {
                    has_local = 1;
                    break;
                }
            }
            unsigned char restriction = has_local
                ? scope->var_target->bindings[slot].restriction
                : KILN_DYN_BINDING_MUTABLE;
            int outer_lexical = 0;
            for (kiln_dyn_scope *outer = scope->parent;
                 outer != NULL && outer != scope->var_target;
                 outer = outer->parent) {
                for (size_t outer_slot = 0; outer_slot < outer->count; ++outer_slot) {
                    if (outer->bindings[outer_slot].name_len == bindings[index].name_len &&
                        memcmp(outer->bindings[outer_slot].name, bindings[index].name,
                               bindings[index].name_len) == 0) {
                        outer_lexical = 1;
                        break;
                    }
                }
                if (outer_lexical) break;
            }
            if (restriction != KILN_DYN_BINDING_PARAMETER &&
                restriction != KILN_DYN_BINDING_READ_ONLY &&
                restriction != KILN_DYN_BINDING_FUNCTION_NAME &&
                restriction != KILN_DYN_BINDING_LEXICAL && !outer_lexical) {
                if (!has_local &&
                    !kiln_dyn_scope_set(scope->var_target, bindings[index].name,
                                        bindings[index].name_len, kiln_c99_undefined()))
                    return KILN_C99_OUT_OF_MEMORY;
                if (!kiln_dyn_scope_mark_restriction(scope->var_target, bindings[index].name,
                                                     bindings[index].name_len,
                                                     KILN_DYN_BINDING_ANNEX_B_VAR))
                    return KILN_C99_OUT_OF_MEMORY;
                if (!kiln_dyn_scope_add_annex_b_declaration(scope,
                                                            bindings[index].function_site))
                    return KILN_C99_OUT_OF_MEMORY;
                bindings[index].annex_b_alias = 1;
            }
        }
    }
    return KILN_C99_OK;
}

/* Lexical block function declarations are initialized when their scope is entered, before
 * statements or case tests run. Reuse the ordinary dynamic function parser and closure capture
 * path, keyed by the declaration's source position. */
static int kiln_dyn_switch_initialize_functions(kiln_host *host,
                                                kiln_dyn_scope *scope,
                                                const kiln_dyn_switch_clause *clauses,
                                                size_t clause_count,
                                                const kiln_dyn_switch_binding *bindings,
                                                size_t binding_count,
                                                kiln_value receiver,
                                                const kiln_value *args, size_t argc,
                                                int depth, kiln_value *thrown) {
    for (size_t index = 0; index < binding_count; ++index) {
        if (!bindings[index].is_function) continue;
        size_t clause = bindings[index].clause_index;
        if (clause >= clause_count) return KILN_C99_INVALID_ARGUMENT;
        kiln_dyn_lexer function_lex;
        memset(&function_lex, 0, sizeof function_lex);
        function_lex.src = clauses[clause].body;
        function_lex.len = clauses[clause].body_len;
        function_lex.pos = (size_t)(bindings[index].function_site - function_lex.src);
        function_lex.function_declaration = 1;
        function_lex.strict = scope->strict;
        kiln_dyn_lexer_next(&function_lex);
        kiln_value function = kiln_c99_undefined();
        int status = kiln_dyn_eval_primary(host, &function_lex, scope, receiver, args, argc,
                                           depth, &function, thrown);
        kiln_dyn_token_free(&function_lex.curr);
        if (status != KILN_C99_OK) return status;
        if (function.tag != KILN_VALUE_FUNCTION ||
            !kiln_dyn_scope_set(scope, bindings[index].name, bindings[index].name_len,
                                function))
            return function.tag == KILN_VALUE_FUNCTION ? KILN_C99_OUT_OF_MEMORY
                                                       : KILN_C99_INVALID_ARGUMENT;
        kiln_c99_gc_keep_alive(&function);
    }
    return KILN_C99_OK;
}

static int kiln_dyn_consume_function_declaration(kiln_dyn_lexer *lex,
                                                  kiln_dyn_scope *scope,
                                                  kiln_host *host, int depth,
                                                  kiln_value *thrown) {
    const char *site = lex->curr.start;
    if (!kiln_dyn_scope_has_function_declaration(scope, site))
        KILN_C99_REFUSE("block-level function declarations in a dynamic Function body");
    int annex_b_alias = kiln_dyn_scope_has_annex_b_declaration(scope, site);
    if (lex->curr.kind == KILN_DYN_KW_ASYNC) {
        kiln_dyn_lexer_next(lex);
        if (lex->curr.kind != KILN_DYN_KW_FUNCTION)
            return kiln_dyn_throw_parse_error(lex, thrown,
                                        "Expected function after async");
    }
    kiln_dyn_lexer_next(lex);
    if (lex->curr.kind == KILN_DYN_STAR) kiln_dyn_lexer_next(lex);
    if (lex->curr.kind != KILN_DYN_IDENT)
        return kiln_dyn_throw_parse_error(lex, thrown,
                                    "Expected function declaration name");
    const char *function_name = lex->curr.start;
    size_t function_name_len = lex->curr.len;
    if (annex_b_alias) {
        kiln_value function = kiln_c99_undefined();
        int found = 0;
        int status = kiln_dyn_scope_get(host, scope, function_name, function_name_len,
                                        depth, &function, &found, thrown);
        if (status != KILN_C99_OK || !found) {
            if (status == KILN_C99_OK) status = KILN_C99_INVALID_ARGUMENT;
            return status;
        }
        kiln_c99_gc_keep_alive(&function);
        if (scope->var_target == NULL ||
            !kiln_dyn_scope_set(scope->var_target, function_name, function_name_len, function))
            return KILN_C99_OUT_OF_MEMORY;
    }
    kiln_dyn_lexer_next(lex);
    if (lex->curr.kind != KILN_DYN_LPAREN)
        return kiln_dyn_throw_parse_error(lex, thrown,
                                    "Expected '(' after function name");
    int depth = 0;
    do {
        if (lex->curr.kind == KILN_DYN_LPAREN) depth++;
        else if (lex->curr.kind == KILN_DYN_RPAREN) depth--;
        kiln_dyn_lexer_next(lex);
    } while (depth > 0 && lex->curr.kind != KILN_DYN_EOF);
    if (depth != 0 || lex->curr.kind != KILN_DYN_LBRACE)
        return kiln_dyn_throw_parse_error(lex, thrown,
                                    "Expected function body");
    const char *body;
    size_t body_len;
    if (!kiln_dyn_capture_block(lex, &body, &body_len))
        return kiln_dyn_throw_parse_error(lex, thrown,
                                    "Unterminated function body");
    (void)body;
    (void)body_len;
    return KILN_C99_OK;
}

static int kiln_dyn_prepare_block_scope(kiln_host *host, kiln_dyn_scope *parent,
                                       kiln_dyn_scope *block_scope, const char *body,
                                       size_t body_len, kiln_value receiver,
                                       const kiln_value *args, size_t argc, int depth,
                                       kiln_dyn_lexer *lex, kiln_value *thrown) {
    kiln_dyn_scope_init(block_scope, parent);
    block_scope->var_target = parent->var_target != NULL ? parent->var_target : parent;
    int status = kiln_dyn_scope_predeclare_lexicals(block_scope, body, body_len, SIZE_MAX);
    if (status != KILN_C99_OK) return status;
    if (!kiln_dyn_source_may_have_function_declaration(body, body_len)) return KILN_C99_OK;

    kiln_dyn_switch_clause clause = {
        .test = NULL, .test_len = 0, .body = body, .body_len = body_len, .is_default = 0,
    };
    kiln_dyn_switch_binding *bindings = NULL;
    size_t binding_count = 0;
    int binding_status = kiln_dyn_switch_bindings(&clause, 1, &bindings, &binding_count);
    if (binding_status <= 0) {
        free(bindings);
        return binding_status < 0
                   ? KILN_C99_OUT_OF_MEMORY
                   : kiln_dyn_throw_parse_error(lex, thrown,
                                                 "Invalid or duplicate block lexical declaration");
    }
    size_t function_count = 0;
    for (size_t index = 0; index < binding_count; ++index) {
        if (!bindings[index].is_function) continue;
        if (function_count != index) bindings[function_count] = bindings[index];
        function_count++;
    }
    if (function_count != 0) {
        status = kiln_dyn_switch_predeclare(block_scope, bindings, function_count);
        if (status == KILN_C99_OK)
            status = kiln_dyn_switch_initialize_functions(host, block_scope, &clause, 1,
                                                          bindings, function_count, receiver,
                                                          args, argc, depth, thrown);
    }
    free(bindings);
    return status;
}

static int kiln_dyn_eval_stmt(kiln_host *host, kiln_dyn_lexer *lex, kiln_dyn_scope *scope,
                              kiln_value receiver, const kiln_value *args, size_t argc,
                              int depth, kiln_value *result, int *returned, kiln_value *thrown) {
    if (lex->curr.kind == KILN_DYN_KW_WITH) {
        kiln_value object;
        kiln_dyn_scope with_scope;
        int status;
        kiln_dyn_lexer_next(lex);
        if (lex->curr.kind != KILN_DYN_LPAREN) {
            return kiln_dyn_throw_parse_error(lex, thrown,
                                        "Expected '(' after with");
        }
        kiln_dyn_lexer_next(lex);
        status = kiln_dyn_eval_expr(host, lex, scope, receiver, args, argc, depth, &object,
                                    thrown);
        if (status != KILN_C99_OK) return status;
        if (lex->curr.kind != KILN_DYN_RPAREN) {
            return kiln_dyn_throw_parse_error(lex, thrown,
                                        "Expected ')' after with object");
        }
        kiln_dyn_lexer_next(lex);
        kiln_dyn_scope_init(&with_scope, scope);
        with_scope.with_object = object;
        with_scope.has_with_object = 1;
        kiln_c99_gc_keep_alive(&object);
        if (lex->curr.kind == KILN_DYN_LBRACE) {
            kiln_dyn_lexer_next(lex);
            while (lex->curr.kind != KILN_DYN_RBRACE && lex->curr.kind != KILN_DYN_EOF) {
                status = kiln_dyn_eval_stmt(host, lex, &with_scope, receiver, args, argc,
                                            depth, result, returned, thrown);
                if (status != KILN_C99_OK || *returned) {
                    kiln_dyn_scope_destroy(&with_scope);
                    return status;
                }
            }
            if (lex->curr.kind != KILN_DYN_RBRACE) {
                kiln_dyn_scope_destroy(&with_scope);
                return kiln_dyn_throw_parse_error(lex, thrown,
                                            "Expected '}' after with body");
            }
            kiln_dyn_lexer_next(lex);
        } else {
            status = kiln_dyn_eval_stmt(host, lex, &with_scope, receiver, args, argc, depth,
                                        result, returned, thrown);
            if (status != KILN_C99_OK || *returned) {
                kiln_dyn_scope_destroy(&with_scope);
                return status;
            }
        }
        kiln_dyn_scope_destroy(&with_scope);
        return KILN_C99_OK;
    }
    if (lex->curr.kind == KILN_DYN_IDENT && lex->curr.len == 3 &&
        memcmp(lex->curr.start, "try", 3) == 0) {
        const char *try_body;
        size_t try_len;
        kiln_dyn_lexer block;
        kiln_dyn_scope try_scope;
        kiln_dyn_lexer_next(lex);
        if (!kiln_dyn_capture_block(lex, &try_body, &try_len)) {
            return kiln_dyn_throw_parse_error(lex, thrown,
                                        "Expected block after try");
        }
        int status = kiln_dyn_prepare_block_scope(
            host, scope, &try_scope, try_body, try_len, receiver, args, argc, depth, lex, thrown);
        if (status != KILN_C99_OK) {
            kiln_dyn_scope_destroy(&try_scope);
            return status;
        }
        memset(&block, 0, sizeof(block));
        block.src = try_body;
        block.len = try_len;
        block.strict = try_scope.strict;
        block.active_labels = lex->active_labels;
        block.break_label = lex->break_label;
        block.break_label_len = lex->break_label_len;
        kiln_dyn_lexer_next(&block);
        status = KILN_C99_OK;
        while (block.curr.kind != KILN_DYN_EOF) {
            status = kiln_dyn_eval_stmt(host, &block, &try_scope, receiver, args, argc, depth,
                                        result, returned, thrown);
            if (status != KILN_C99_OK || *returned) break;
        }
        lex->break_label = block.break_label;
        lex->break_label_len = block.break_label_len;
        kiln_dyn_token_free(&block.curr);
        kiln_dyn_scope_destroy(&try_scope);
        int did_throw = status == KILN_C99_THROWN;
        kiln_value exception = did_throw && thrown != NULL ? *thrown : kiln_c99_undefined();
        int completion_status = status;
        int completion_return = *returned;
        kiln_value completion_result = *result;
        kiln_value completion_exception = exception;
        kiln_c99_gc_keep_alive(&completion_result);
        kiln_c99_gc_keep_alive(&completion_exception);
        if (did_throw && thrown != NULL) *thrown = kiln_c99_undefined();
        int has_catch = lex->curr.kind == KILN_DYN_IDENT && lex->curr.len == 5 &&
                        memcmp(lex->curr.start, "catch", 5) == 0;
        if (has_catch) {
            const char *catch_name = NULL;
            size_t catch_name_len = 0;
            const char *catch_body;
            size_t catch_len;
            kiln_dyn_lexer_next(lex);
            if (lex->curr.kind != KILN_DYN_LPAREN) {
                return kiln_dyn_throw_parse_error(lex, thrown,
                                            "Expected '(' after catch");
            }
            kiln_dyn_lexer_next(lex);
            if (lex->curr.kind == KILN_DYN_IDENT) {
                catch_name = lex->curr.start;
                catch_name_len = lex->curr.len;
                kiln_dyn_lexer_next(lex);
            }
            if (lex->curr.kind != KILN_DYN_RPAREN) {
                return kiln_dyn_throw_parse_error(lex, thrown,
                                            "Expected ')' after catch binding");
            }
            kiln_dyn_lexer_next(lex);
            if (!kiln_dyn_capture_block(lex, &catch_body, &catch_len)) {
                return kiln_dyn_throw_parse_error(lex, thrown,
                                            "Expected catch block");
            }
            if (did_throw) {
                kiln_dyn_scope catch_scope;
                kiln_dyn_scope_init(&catch_scope, scope);
                catch_scope.var_target = scope->var_target != NULL ? scope->var_target : scope;
                if (catch_name != NULL &&
                    !kiln_dyn_scope_set(&catch_scope, catch_name, catch_name_len, exception)) {
                    kiln_dyn_scope_destroy(&catch_scope);
                    return KILN_C99_OUT_OF_MEMORY;
                }
                kiln_dyn_scope catch_block_scope;
                int catch_prepare_status = kiln_dyn_prepare_block_scope(
                    host, &catch_scope, &catch_block_scope, catch_body, catch_len, receiver,
                    args, argc, depth, lex, thrown);
                if (catch_prepare_status != KILN_C99_OK) {
                    kiln_dyn_scope_destroy(&catch_block_scope);
                    kiln_dyn_scope_destroy(&catch_scope);
                    return catch_prepare_status;
                }
                int catch_returned = 0;
                memset(&block, 0, sizeof(block));
                block.src = catch_body;
                block.len = catch_len;
                block.strict = catch_block_scope.strict;
                block.active_labels = lex->active_labels;
                block.break_label = lex->break_label;
                block.break_label_len = lex->break_label_len;
                kiln_dyn_lexer_next(&block);
                while (block.curr.kind != KILN_DYN_EOF) {
                    status = kiln_dyn_eval_stmt(host, &block, &catch_block_scope, receiver, args,
                                                argc, depth, result, &catch_returned, thrown);
                    if (status != KILN_C99_OK || catch_returned) break;
                }
                lex->break_label = block.break_label;
                lex->break_label_len = block.break_label_len;
                kiln_dyn_token_free(&block.curr);
                kiln_dyn_scope_destroy(&catch_block_scope);
                kiln_dyn_scope_destroy(&catch_scope);
                completion_status = status;
                completion_return = catch_returned;
                completion_result = *result;
                completion_exception = status == KILN_C99_THROWN && thrown != NULL
                                           ? *thrown : kiln_c99_undefined();
                kiln_c99_gc_keep_alive(&completion_result);
                kiln_c99_gc_keep_alive(&completion_exception);
            }
        }
        int has_finally = lex->curr.kind == KILN_DYN_IDENT && lex->curr.len == 7 &&
                          memcmp(lex->curr.start, "finally", 7) == 0;
        if (has_finally) {
            const char *finally_body;
            size_t finally_len;
            kiln_dyn_lexer_next(lex);
            if (!kiln_dyn_capture_block(lex, &finally_body, &finally_len))
                return kiln_dyn_throw_parse_error(lex, thrown,
                                                  "Expected finally block");
            kiln_dyn_scope finally_block_scope;
            int finally_prepare_status = kiln_dyn_prepare_block_scope(
                host, scope, &finally_block_scope, finally_body, finally_len, receiver,
                args, argc, depth, lex, thrown);
            if (finally_prepare_status != KILN_C99_OK) {
                kiln_dyn_scope_destroy(&finally_block_scope);
                return finally_prepare_status;
            }
            int finally_returned = 0;
            kiln_value finally_result = kiln_c99_undefined();
            const char *pending_break_label = lex->break_label;
            size_t pending_break_label_len = lex->break_label_len;
            memset(&block, 0, sizeof(block));
            block.src = finally_body;
            block.len = finally_len;
            block.strict = finally_block_scope.strict;
            block.active_labels = lex->active_labels;
            block.break_label = lex->break_label;
            block.break_label_len = lex->break_label_len;
            kiln_dyn_lexer_next(&block);
            int finally_status = KILN_C99_OK;
            while (block.curr.kind != KILN_DYN_EOF) {
                finally_status = kiln_dyn_eval_stmt(host, &block, &finally_block_scope, receiver,
                    args, argc, depth, &finally_result, &finally_returned, thrown);
                if (finally_status != KILN_C99_OK || finally_returned) break;
            }
            if (finally_status != KILN_C99_OK || finally_returned) {
                lex->break_label = block.break_label;
                lex->break_label_len = block.break_label_len;
            } else {
                /* A normally completing finally preserves the pending abrupt completion,
                 * including its label. A nested loop in the finalizer may consume its own
                 * unlabeled break and must not erase the label being propagated outside. */
                lex->break_label = pending_break_label;
                lex->break_label_len = pending_break_label_len;
            }
            kiln_dyn_token_free(&block.curr);
            kiln_dyn_scope_destroy(&finally_block_scope);
            if (finally_status != KILN_C99_OK || finally_returned) {
                completion_status = finally_status;
                completion_return = finally_returned;
                if (finally_status == KILN_C99_OK && finally_returned == 1) {
                    completion_result = finally_result;
                    kiln_c99_gc_keep_alive(&completion_result);
                }
                completion_exception = finally_status == KILN_C99_THROWN && thrown != NULL
                                           ? *thrown : kiln_c99_undefined();
                kiln_c99_gc_keep_alive(&completion_exception);
            }
        } else if (completion_status == KILN_C99_THROWN && thrown != NULL) {
            *thrown = completion_exception;
        }
        *returned = completion_return;
        *result = completion_result;
        kiln_c99_gc_keep_alive(&completion_result);
        if (completion_status == KILN_C99_THROWN && thrown != NULL)
            *thrown = completion_exception;
        return completion_status;
    }
    if (lex->curr.kind == KILN_DYN_KW_THROW) {
        kiln_value exception;
        kiln_dyn_lexer_next(lex);
        int status = kiln_dyn_eval_expr(host, lex, scope, receiver, args, argc, depth,
                                        &exception, thrown);
        if (status != KILN_C99_OK) return status;
        if (lex->curr.kind == KILN_DYN_SEMI) kiln_dyn_lexer_next(lex);
        if (thrown != NULL) *thrown = exception;
        return KILN_C99_THROWN;
    }
    if (lex->curr.kind == KILN_DYN_IDENT) {
        size_t saved_pos = lex->pos;
        kiln_dyn_token saved_tok = lex->curr;
        kiln_dyn_lexer_next(lex);
        if (lex->curr.kind == KILN_DYN_COLON) {
            kiln_dyn_active_label label = {
                .name = saved_tok.start,
                .name_len = saved_tok.len,
                .iteration_target = kiln_dyn_labeled_statement_targets_iteration(lex),
                .parent = lex->active_labels,
            };
            lex->active_labels = &label;
            kiln_dyn_lexer_next(lex);
            int status = kiln_dyn_eval_stmt(host, lex, scope, receiver, args, argc, depth, result, returned, thrown);
            lex->active_labels = label.parent;
            if (status == KILN_C99_OK && *returned == 2 &&
                lex->break_label != NULL && label.name_len == lex->break_label_len &&
                memcmp(label.name, lex->break_label, label.name_len) == 0) {
                lex->break_label = NULL;
                lex->break_label_len = 0;
                *returned = 0;
            }
            return status;
        }
        lex->pos = saved_pos;
        lex->curr = saved_tok;
    }
    if (lex->curr.kind == KILN_DYN_SEMI) {
        kiln_dyn_lexer_next(lex);
        return KILN_C99_OK;
    }
    if (lex->curr.kind == KILN_DYN_KW_RETURN) {
        kiln_dyn_lexer_next(lex);
        if (lex->curr.kind != KILN_DYN_SEMI && lex->curr.kind != KILN_DYN_RBRACE && lex->curr.kind != KILN_DYN_EOF) {
            int status = kiln_dyn_eval_expr(host, lex, scope, receiver, args, argc, depth, result, thrown);
            if (status != KILN_C99_OK) return status;
        } else {
            *result = kiln_c99_undefined();
        }
        if (lex->curr.kind == KILN_DYN_SEMI) kiln_dyn_lexer_next(lex);
        *returned = 1;
        return KILN_C99_OK;
    }
    if (lex->curr.kind == KILN_DYN_KW_IF) {
        kiln_dyn_lexer_next(lex);
        if (lex->curr.kind != KILN_DYN_LPAREN) {
            return kiln_dyn_throw_parse_error(lex, thrown, "Expected '(' after if");
        }
        kiln_dyn_lexer_next(lex);
        kiln_value cond;
        int status = kiln_dyn_eval_expr(host, lex, scope, receiver, args, argc, depth, &cond, thrown);
        if (status != KILN_C99_OK) return status;
        if (lex->curr.kind != KILN_DYN_RPAREN) {
            return kiln_dyn_throw_parse_error(lex, thrown, "Expected ')' after if condition");
        }
        kiln_dyn_lexer_next(lex);
        int is_true = kiln_c99_truthy(cond);
        if (is_true) {
            status = kiln_dyn_eval_stmt(host, lex, scope, receiver, args, argc, depth, result, returned, thrown);
            if (status != KILN_C99_OK || *returned) return status;
            if (lex->curr.kind == KILN_DYN_KW_ELSE) {
                kiln_dyn_lexer_next(lex);
                kiln_dyn_skip_statement(lex);
            }
        } else {
            kiln_dyn_skip_statement(lex);
            if (lex->curr.kind == KILN_DYN_KW_ELSE) {
                kiln_dyn_lexer_next(lex);
                status = kiln_dyn_eval_stmt(host, lex, scope, receiver, args, argc, depth, result, returned, thrown);
                if (status != KILN_C99_OK || *returned) return status;
            }
        }
        return KILN_C99_OK;
    }
    if (lex->curr.kind == KILN_DYN_KW_BREAK) {
        kiln_dyn_lexer_next(lex);
        if (lex->curr.kind == KILN_DYN_IDENT) {
            lex->break_label = lex->curr.start;
            lex->break_label_len = lex->curr.len;
            kiln_dyn_lexer_next(lex);
        } else {
            lex->break_label = NULL;
            lex->break_label_len = 0;
        }
        if (lex->curr.kind == KILN_DYN_SEMI) kiln_dyn_lexer_next(lex);
        *returned = 2;
        return KILN_C99_OK;
    }
    if (lex->curr.kind == KILN_DYN_KW_CONTINUE) {
        kiln_dyn_lexer_next(lex);
        if (lex->curr.kind == KILN_DYN_IDENT) {
            lex->break_label = lex->curr.start;
            lex->break_label_len = lex->curr.len;
            kiln_dyn_lexer_next(lex);
        } else {
            lex->break_label = NULL;
            lex->break_label_len = 0;
        }
        if (lex->curr.kind == KILN_DYN_SEMI) kiln_dyn_lexer_next(lex);
        *returned = 3;
        return KILN_C99_OK;
    }
    if (lex->curr.kind == KILN_DYN_KW_SWITCH) {
        lex->active_labels = NULL;
        kiln_dyn_lexer_next(lex);
        if (lex->curr.kind != KILN_DYN_LPAREN)
            return kiln_dyn_throw_parse_error(lex, thrown, "Expected '(' after switch");
        kiln_dyn_lexer_next(lex);
        const char *discriminant_source = lex->curr.start;
        size_t discriminant_start = discriminant_source == NULL
                                        ? lex->pos
                                        : (size_t)(discriminant_source - lex->src);
        int parens = 0, brackets = 0, braces = 0;
        while (lex->curr.kind != KILN_DYN_EOF) {
            if (parens == 0 && brackets == 0 && braces == 0 &&
                lex->curr.kind == KILN_DYN_RPAREN)
                break;
            if (lex->curr.kind == KILN_DYN_LPAREN) parens++;
            else if (lex->curr.kind == KILN_DYN_RPAREN && parens > 0) parens--;
            else if (lex->curr.kind == KILN_DYN_LBRACKET) brackets++;
            else if (lex->curr.kind == KILN_DYN_RBRACKET && brackets > 0) brackets--;
            else if (lex->curr.kind == KILN_DYN_LBRACE) braces++;
            else if (lex->curr.kind == KILN_DYN_RBRACE && braces > 0) braces--;
            kiln_dyn_lexer_next(lex);
        }
        if (lex->curr.kind != KILN_DYN_RPAREN)
            return kiln_dyn_throw_parse_error(lex, thrown, "Expected ')' after switch value");
        size_t discriminant_end = (size_t)(lex->curr.start - lex->src);
        if (discriminant_end == discriminant_start)
            return kiln_dyn_throw_parse_error(lex, thrown, "Expected switch value");
        kiln_dyn_lexer_next(lex);
        const char *body;
        size_t body_len;
        if (!kiln_dyn_capture_block(lex, &body, &body_len))
            return kiln_dyn_throw_parse_error(lex, thrown, "Expected switch body");
        kiln_dyn_switch_clause *clauses = NULL;
        size_t clause_count = 0;
        int scanned = kiln_dyn_switch_clauses(body, body_len, &clauses, &clause_count);
        if (scanned <= 0) {
            free(clauses);
            if (scanned < 0) return KILN_C99_OUT_OF_MEMORY;
            return kiln_dyn_throw_parse_error(lex, thrown, "Malformed switch clause");
        }
        size_t default_clause = SIZE_MAX;
        size_t selected_clause = SIZE_MAX;
        // Validate the complete clause inventory before evaluating any case expression.
        for (size_t index = 0; index < clause_count; ++index) {
            if (!clauses[index].is_default) continue;
            if (default_clause != SIZE_MAX) {
                free(clauses);
                return kiln_dyn_throw_parse_error(lex, thrown, "Duplicate default clause");
            }
            default_clause = index;
        }
        kiln_dyn_switch_binding *bindings = NULL;
        size_t binding_count = 0;
        int binding_scan = kiln_dyn_switch_bindings(clauses, clause_count,
                                                    &bindings, &binding_count);
        if (binding_scan <= 0) {
            free(bindings);
            free(clauses);
            if (binding_scan < 0) return KILN_C99_OUT_OF_MEMORY;
            return kiln_dyn_throw_parse_error(lex, thrown,
                                              "Invalid or duplicate switch lexical declaration");
        }
        kiln_dyn_scope switch_scope;
        kiln_dyn_scope_init(&switch_scope, scope);
        switch_scope.var_target = scope->var_target != NULL ? scope->var_target : scope;
        int status = kiln_dyn_switch_predeclare(&switch_scope, bindings, binding_count);
        if (status != KILN_C99_OK) {
            kiln_dyn_scope_destroy(&switch_scope);
            free(bindings);
            free(clauses);
            return status;
        }
        kiln_dyn_lexer discriminant_lex;
        memset(&discriminant_lex, 0, sizeof discriminant_lex);
        discriminant_lex.src = lex->src + discriminant_start;
        discriminant_lex.len = discriminant_end - discriminant_start;
        discriminant_lex.strict = scope->strict;
        kiln_dyn_lexer_next(&discriminant_lex);
        kiln_value discriminant;
        /* The switch discriminant runs before entering the CaseBlock lexical environment.
         * Keep it in the surrounding scope; case tests and statements below use switch_scope. */
        status = kiln_dyn_eval_expr(host, &discriminant_lex, scope, receiver,
                                    args, argc, depth, &discriminant, thrown);
        if (status == KILN_C99_OK && discriminant_lex.curr.kind != KILN_DYN_EOF)
            status = kiln_dyn_throw_parse_error(&discriminant_lex, thrown,
                                                "Unexpected token in switch value");
        kiln_dyn_token_free(&discriminant_lex.curr);
        if (status != KILN_C99_OK) {
            kiln_dyn_scope_destroy(&switch_scope);
            free(bindings);
            free(clauses);
            return status;
        }
        kiln_c99_gc_keep_alive(&discriminant);
        status = kiln_dyn_switch_initialize_functions(host, &switch_scope, clauses,
                                                      clause_count, bindings, binding_count,
                                                      receiver, args, argc, depth, thrown);
        kiln_c99_gc_keep_alive(&discriminant);
        if (status != KILN_C99_OK) {
            kiln_dyn_scope_destroy(&switch_scope);
            free(bindings);
            free(clauses);
            return status;
        }
        for (size_t index = 0; index < clause_count; ++index) {
            if (clauses[index].is_default) continue;
            if (selected_clause != SIZE_MAX) continue;
            kiln_dyn_lexer test_lex;
            memset(&test_lex, 0, sizeof test_lex);
            test_lex.src = clauses[index].test;
            test_lex.len = clauses[index].test_len;
            test_lex.strict = switch_scope.strict;
            kiln_dyn_lexer_next(&test_lex);
            kiln_value test_value;
            status = kiln_dyn_eval_expr(host, &test_lex, &switch_scope, receiver, args, argc,
                                        depth, &test_value, thrown);
            if (status == KILN_C99_OK && test_lex.curr.kind != KILN_DYN_EOF)
                status = kiln_dyn_throw_parse_error(&test_lex, thrown,
                                                   "Unexpected token in switch case");
            kiln_dyn_token_free(&test_lex.curr);
            kiln_c99_gc_keep_alive(&discriminant);
            if (status != KILN_C99_OK) {
                kiln_dyn_scope_destroy(&switch_scope);
                free(bindings);
                free(clauses);
                return status;
            }
            if (kiln_c99_strict_equal(discriminant, test_value)) selected_clause = index;
        }
        if (selected_clause == SIZE_MAX) selected_clause = default_clause;
        for (size_t index = selected_clause; index < clause_count && selected_clause != SIZE_MAX;
             ++index) {
            kiln_dyn_lexer clause_lex;
            memset(&clause_lex, 0, sizeof clause_lex);
            clause_lex.src = clauses[index].body;
            clause_lex.len = clauses[index].body_len;
            clause_lex.strict = switch_scope.strict;
            // The switch's label belongs to the switch, not to an inner loop in a case.
            // Labeled abrupt completions propagate to the enclosing label evaluator.
            clause_lex.break_label = lex->break_label;
            clause_lex.break_label_len = lex->break_label_len;
            kiln_dyn_lexer_next(&clause_lex);
            while (clause_lex.curr.kind != KILN_DYN_EOF && *returned == 0) {
                status = kiln_dyn_eval_stmt(host, &clause_lex, &switch_scope, receiver, args,
                                            argc, depth, result, returned, thrown);
                if (status != KILN_C99_OK) break;
            }
            lex->break_label = clause_lex.break_label;
            lex->break_label_len = clause_lex.break_label_len;
            kiln_dyn_token_free(&clause_lex.curr);
            if (status != KILN_C99_OK) break;
            if (*returned == 2 && lex->break_label == NULL) {
                *returned = 0;
                break;
            }
            if (*returned != 0) break;
        }
        kiln_dyn_scope_destroy(&switch_scope);
        free(bindings);
        free(clauses);
        kiln_c99_gc_keep_alive(&discriminant);
        return status;
    }
    if (lex->curr.kind == KILN_DYN_IDENT && lex->curr.len == 5 &&
        memcmp(lex->curr.start, "while", 5) == 0) {
        kiln_dyn_active_label *my_labels = lex->active_labels;
        lex->active_labels = NULL;
        kiln_dyn_lexer_next(lex);
        if (lex->curr.kind != KILN_DYN_LPAREN) {
            return kiln_dyn_throw_parse_error(lex, thrown, "Expected '(' after while");
        }
        kiln_dyn_lexer_next(lex);
        size_t cond_start = lex->curr.start != NULL
                                ? (size_t)(lex->curr.start - lex->src)
                                : lex->pos;
        int parens = 1;
        while (lex->curr.kind != KILN_DYN_EOF && parens != 0) {
            if (lex->curr.kind == KILN_DYN_LPAREN) parens++;
            else if (lex->curr.kind == KILN_DYN_RPAREN) parens--;
            if (parens != 0) kiln_dyn_lexer_next(lex);
        }
        if (parens != 0) {
            return kiln_dyn_throw_parse_error(lex, thrown, "Expected ')' after while condition");
        }
        size_t cond_end = lex->curr.start != NULL
                              ? (size_t)(lex->curr.start - lex->src)
                              : lex->pos;
        kiln_dyn_lexer_next(lex);
        size_t body_start = lex->curr.start != NULL
                                ? (size_t)(lex->curr.start - lex->src)
                                : lex->pos;
        kiln_dyn_skip_statement(lex);
        size_t body_end = lex->curr.start != NULL
                              ? (size_t)(lex->curr.start - lex->src)
                              : lex->len;
        const char *cond_src = &lex->src[cond_start];
        size_t cond_len = cond_end > cond_start ? cond_end - cond_start : 0;
        const char *body_src = &lex->src[body_start];
        size_t body_len = body_end > body_start ? body_end - body_start : 0;
        int break_loop = 0;
        while (!break_loop) {
            kiln_dyn_lexer clex;
            memset(&clex, 0, sizeof clex);
            clex.src = cond_src;
            clex.len = cond_len;
            clex.strict = scope->strict;
            kiln_dyn_lexer_next(&clex);
            kiln_value condition;
            int status = kiln_dyn_eval_expr(host, &clex, scope, receiver, args, argc, depth,
                                            &condition, thrown);
            kiln_dyn_token_free(&clex.curr);
            if (status != KILN_C99_OK) return status;
            if (!kiln_c99_truthy(condition)) break;

            kiln_dyn_lexer blex;
            memset(&blex, 0, sizeof blex);
            blex.src = body_src;
            blex.len = body_len;
            blex.strict = scope->strict;
            blex.break_label = lex->break_label;
            blex.break_label_len = lex->break_label_len;
            kiln_dyn_lexer_next(&blex);
            while (blex.curr.kind != KILN_DYN_EOF) {
                int flow = 0;
                status = kiln_dyn_eval_stmt(host, &blex, scope, receiver, args, argc,
                                            depth, result, &flow, thrown);
                lex->break_label = blex.break_label;
                lex->break_label_len = blex.break_label_len;
                if (status != KILN_C99_OK || flow == 1) {
                    *returned = flow;
                    kiln_dyn_token_free(&blex.curr);
                    return status;
                }
                if (flow == 2) {
                    if (lex->break_label != NULL) {
                        if (kiln_dyn_label_matches(my_labels, lex->break_label,
                                                   lex->break_label_len)) {
                            lex->break_label = NULL;
                            lex->break_label_len = 0;
                        } else {
                            *returned = 2;
                        }
                    }
                    break_loop = 1;
                    break;
                }
                if (flow == 3) {
                    if (lex->break_label != NULL) {
                        if (kiln_dyn_label_matches(my_labels, lex->break_label,
                                                   lex->break_label_len)) {
                            lex->break_label = NULL;
                            lex->break_label_len = 0;
                        } else {
                            *returned = 3;
                            break_loop = 1;
                        }
                    }
                    break;
                }
            }
            kiln_dyn_token_free(&blex.curr);
            if (*returned == 2 || *returned == 3) return KILN_C99_OK;
        }
        if (*returned == 2 && lex->break_label != NULL &&
            kiln_dyn_label_matches(my_labels, lex->break_label, lex->break_label_len)) {
            lex->break_label = NULL;
            lex->break_label_len = 0;
            *returned = 0;
        }
        return KILN_C99_OK;
    }
    if (lex->curr.kind == KILN_DYN_IDENT && lex->curr.len == 2 &&
        memcmp(lex->curr.start, "do", 2) == 0) {
        kiln_dyn_active_label *my_labels = lex->active_labels;
        lex->active_labels = NULL;
        kiln_dyn_lexer_next(lex);
        size_t body_start = lex->curr.start != NULL
                                ? (size_t)(lex->curr.start - lex->src) : lex->pos;
        kiln_dyn_skip_statement(lex);
        size_t body_end = lex->curr.start != NULL
                              ? (size_t)(lex->curr.start - lex->src) : lex->len;
        if (lex->curr.kind != KILN_DYN_IDENT || lex->curr.len != 5 ||
            memcmp(lex->curr.start, "while", 5) != 0)
            return kiln_dyn_throw_parse_error(lex, thrown,
                                              "Expected while after do body");
        kiln_dyn_lexer_next(lex);
        if (lex->curr.kind != KILN_DYN_LPAREN)
            return kiln_dyn_throw_parse_error(lex, thrown,
                                              "Expected '(' after do-while");
        kiln_dyn_lexer_next(lex);
        size_t cond_start = lex->curr.start != NULL
                                ? (size_t)(lex->curr.start - lex->src) : lex->pos;
        int parens = 1;
        while (lex->curr.kind != KILN_DYN_EOF && parens != 0) {
            if (lex->curr.kind == KILN_DYN_LPAREN) parens++;
            else if (lex->curr.kind == KILN_DYN_RPAREN) parens--;
            if (parens != 0) kiln_dyn_lexer_next(lex);
        }
        if (parens != 0)
            return kiln_dyn_throw_parse_error(lex, thrown,
                                              "Expected ')' after do-while condition");
        size_t cond_end = lex->curr.start != NULL
                              ? (size_t)(lex->curr.start - lex->src) : lex->pos;
        kiln_dyn_lexer_next(lex);
        if (lex->curr.kind == KILN_DYN_SEMI) kiln_dyn_lexer_next(lex);
        const char *body_src = lex->src + body_start;
        size_t body_len = body_end > body_start ? body_end - body_start : 0;
        const char *cond_src = lex->src + cond_start;
        size_t cond_len = cond_end > cond_start ? cond_end - cond_start : 0;
        int break_loop = 0;
        while (!break_loop) {
            kiln_dyn_lexer blex;
            memset(&blex, 0, sizeof blex);
            blex.src = body_src;
            blex.len = body_len;
            blex.strict = scope->strict;
            blex.break_label = lex->break_label;
            blex.break_label_len = lex->break_label_len;
            kiln_dyn_lexer_next(&blex);
            while (blex.curr.kind != KILN_DYN_EOF) {
                int flow = 0;
                int status = kiln_dyn_eval_stmt(host, &blex, scope, receiver, args, argc,
                                                depth, result, &flow, thrown);
                lex->break_label = blex.break_label;
                lex->break_label_len = blex.break_label_len;
                if (status != KILN_C99_OK || flow == 1) {
                    *returned = flow;
                    kiln_dyn_token_free(&blex.curr);
                    return status;
                }
                if (flow == 2) {
                    if (lex->break_label != NULL) {
                        if (kiln_dyn_label_matches(my_labels, lex->break_label,
                                                   lex->break_label_len)) {
                            lex->break_label = NULL;
                            lex->break_label_len = 0;
                        } else {
                            *returned = 2;
                        }
                    }
                    break_loop = 1;
                    break;
                }
                if (flow == 3) {
                    if (lex->break_label != NULL) {
                        if (kiln_dyn_label_matches(my_labels, lex->break_label,
                                                   lex->break_label_len)) {
                            lex->break_label = NULL;
                            lex->break_label_len = 0;
                        } else {
                            *returned = 3;
                            break_loop = 1;
                        }
                    }
                    break;
                }
            }
            kiln_dyn_token_free(&blex.curr);
            if (*returned == 2 || *returned == 3) return KILN_C99_OK;
            if (break_loop) break;

            kiln_dyn_lexer clex;
            memset(&clex, 0, sizeof clex);
            clex.src = cond_src;
            clex.len = cond_len;
            clex.strict = scope->strict;
            kiln_dyn_lexer_next(&clex);
            kiln_value condition;
            int status = kiln_dyn_eval_expr(host, &clex, scope, receiver, args, argc,
                                            depth, &condition, thrown);
            if (status == KILN_C99_OK && clex.curr.kind != KILN_DYN_EOF)
                status = kiln_dyn_throw_parse_error(&clex, thrown,
                                                    "Unexpected token in do-while condition");
            kiln_dyn_token_free(&clex.curr);
            if (status != KILN_C99_OK) return status;
            if (!kiln_c99_truthy(condition)) break;
        }
        if (*returned == 2 && lex->break_label != NULL &&
            kiln_dyn_label_matches(my_labels, lex->break_label, lex->break_label_len)) {
            lex->break_label = NULL;
            lex->break_label_len = 0;
            *returned = 0;
        }
        return KILN_C99_OK;
    }
    if (lex->curr.kind == KILN_DYN_KW_FOR) {
        kiln_dyn_active_label *my_labels = lex->active_labels;
        lex->active_labels = NULL;

        kiln_dyn_lexer_next(lex);
        if (lex->curr.kind != KILN_DYN_LPAREN) {
            return kiln_dyn_throw_parse_error(lex, thrown, "Expected '(' after for");
        }
        kiln_dyn_lexer_next(lex);

        size_t saved_pos = lex->pos;
        kiln_dyn_token saved_tok = lex->curr;
        int is_decl = (lex->curr.kind == KILN_DYN_KW_VAR || lex->curr.kind == KILN_DYN_KW_LET || lex->curr.kind == KILN_DYN_KW_CONST);
        if (is_decl) kiln_dyn_lexer_next(lex);
        if (lex->curr.kind == KILN_DYN_IDENT) {
            const char *var_name = lex->curr.start;
            size_t var_name_len = lex->curr.len;
            kiln_dyn_lexer_next(lex);
            if (lex->curr.kind == KILN_DYN_KW_IN || lex->curr.kind == KILN_DYN_KW_OF) {
                int is_for_in = (lex->curr.kind == KILN_DYN_KW_IN);
                kiln_dyn_lexer_next(lex);
                kiln_value target;
                int status = kiln_dyn_eval_expr(host, lex, scope, receiver, args, argc, depth, &target, thrown);
                if (status != KILN_C99_OK) return status;
                if (lex->curr.kind != KILN_DYN_RPAREN) {
                    return kiln_dyn_throw_parse_error(lex, thrown, "Expected ')' after for target");
                }
                kiln_dyn_lexer_next(lex);

                /* A loop body is a statement, not necessarily a block or expression ending at
                 * the next semicolon. In particular, `for (x in xs) if (p(x)) { ... }` is a
                 * single statement whose nested braces and optional `else` must stay together.
                 * Use the same token-aware statement walker as conditional branches; this also
                 * keeps strings, comments, regexps and nested control statements opaque. */
                size_t body_start = lex->curr.start != NULL
                                        ? (size_t)(lex->curr.start - lex->src)
                                        : lex->pos;
                kiln_dyn_skip_statement(lex);
                size_t body_end = lex->curr.start != NULL
                                      ? (size_t)(lex->curr.start - lex->src)
                                      : lex->len;
                const char *body_src = &lex->src[body_start];
                size_t body_len = (body_end > body_start) ? (body_end - body_start) : 0;

                if (is_for_in) {
                    kiln_value keys;
                    status = kiln_c99_own_keys(host, target, &keys, depth, thrown);
                    if (status != KILN_C99_OK) return status;
                    if (keys.tag == KILN_VALUE_ARRAY) {
                        kiln_array *arr = kiln_c99_array_of(keys);
                        for (size_t i = 0; i < arr->count; ++i) {
                            kiln_dyn_scope_set(scope, var_name, var_name_len, kiln_val_to_boundary(arr->elements[i]));
                            kiln_dyn_lexer blex;
                            memset(&blex, 0, sizeof(blex));
                            blex.src = body_src;
                            blex.len = body_len;
                            blex.strict = scope->strict;
                            blex.pos = 0;
                            blex.break_label = lex->break_label;
                            blex.break_label_len = lex->break_label_len;
                            kiln_dyn_lexer_next(&blex);
                            int break_loop = 0;
                            while (blex.curr.kind != KILN_DYN_EOF) {
                                int flow = 0;
                                status = kiln_dyn_eval_stmt(host, &blex, scope, receiver, args, argc, depth, result, &flow, thrown);
                                lex->break_label = blex.break_label;
                                lex->break_label_len = blex.break_label_len;
                                if (status != KILN_C99_OK || flow == 1) {
                                    *returned = flow;
                                    kiln_dyn_token_free(&blex.curr);
                                    return status;
                                }
                                if (flow == 2) {
                                    if (lex->break_label != NULL) {
                                        if (kiln_dyn_label_matches(my_labels, lex->break_label,
                                                                   lex->break_label_len)) {
                                            lex->break_label = NULL;
                                            lex->break_label_len = 0;
                                        } else {
                                            *returned = 2;
                                        }
                                    }
                                    break_loop = 1;
                                    kiln_dyn_token_free(&blex.curr);
                                    break;
                                }
                                if (flow == 3) {
                                    if (lex->break_label != NULL) {
                                        if (kiln_dyn_label_matches(my_labels, lex->break_label,
                                                                   lex->break_label_len)) {
                                            lex->break_label = NULL;
                                            lex->break_label_len = 0;
                                        } else {
                                            *returned = 3;
                                        }
                                    }
                                    kiln_dyn_token_free(&blex.curr);
                                    break;
                                }
                            }
                            kiln_dyn_token_free(&blex.curr);
                            if (*returned == 3) return KILN_C99_OK;
                            if (break_loop) break;
                        }
                    }
                } else {
                    if (target.tag == KILN_VALUE_ARRAY) {
                        kiln_array *arr = kiln_c99_array_of(target);
                        for (size_t i = 0; i < arr->count; ++i) {
                            kiln_value elem = arr->elements[i] == KILN_VAL_HOLE
                                                  ? kiln_c99_undefined()
                                                  : kiln_val_to_boundary(arr->elements[i]);
                            kiln_dyn_scope_set(scope, var_name, var_name_len, elem);
                            kiln_dyn_lexer blex;
                            memset(&blex, 0, sizeof(blex));
                            blex.src = body_src;
                            blex.len = body_len;
                            blex.strict = scope->strict;
                            blex.pos = 0;
                            blex.break_label = lex->break_label;
                            blex.break_label_len = lex->break_label_len;
                            kiln_dyn_lexer_next(&blex);
                            int break_loop = 0;
                            while (blex.curr.kind != KILN_DYN_EOF) {
                                int flow = 0;
                                status = kiln_dyn_eval_stmt(host, &blex, scope, receiver, args, argc, depth, result, &flow, thrown);
                                lex->break_label = blex.break_label;
                                lex->break_label_len = blex.break_label_len;
                                if (status != KILN_C99_OK || flow == 1) {
                                    *returned = flow;
                                    kiln_dyn_token_free(&blex.curr);
                                    return status;
                                }
                                if (flow == 2) {
                                    if (lex->break_label != NULL) {
                                        if (kiln_dyn_label_matches(my_labels, lex->break_label,
                                                                   lex->break_label_len)) {
                                            lex->break_label = NULL;
                                            lex->break_label_len = 0;
                                        } else {
                                            *returned = 2;
                                        }
                                    }
                                    break_loop = 1;
                                    kiln_dyn_token_free(&blex.curr);
                                    break;
                                }
                                if (flow == 3) {
                                    if (lex->break_label != NULL) {
                                        if (kiln_dyn_label_matches(my_labels, lex->break_label,
                                                                   lex->break_label_len)) {
                                            lex->break_label = NULL;
                                            lex->break_label_len = 0;
                                        } else {
                                            *returned = 3;
                                        }
                                    }
                                    kiln_dyn_token_free(&blex.curr);
                                    break;
                                }
                            }
                            kiln_dyn_token_free(&blex.curr);
                            if (*returned == 3) return KILN_C99_OK;
                            if (break_loop) break;
                        }
                    }
                }
                if (*returned == 2 && lex->break_label != NULL &&
                    kiln_dyn_label_matches(my_labels, lex->break_label,
                                           lex->break_label_len)) {
                    lex->break_label = NULL;
                    lex->break_label_len = 0;
                    *returned = 0;
                }
                return KILN_C99_OK;
            }
        }
        lex->pos = saved_pos;
        lex->curr = saved_tok;

        if (lex->curr.kind != KILN_DYN_SEMI) {
            int status = kiln_dyn_eval_stmt(host, lex, scope, receiver, args, argc, depth, result, returned, thrown);
            if (status != KILN_C99_OK || *returned) return status;
        } else {
            kiln_dyn_lexer_next(lex);
        }
        size_t cond_start = lex->pos - lex->curr.len;
        int pdepth = 0;
        while (lex->curr.kind != KILN_DYN_EOF && (lex->curr.kind != KILN_DYN_SEMI || pdepth > 0)) {
            if (lex->curr.kind == KILN_DYN_LPAREN) pdepth++;
            else if (lex->curr.kind == KILN_DYN_RPAREN) pdepth--;
            kiln_dyn_lexer_next(lex);
        }
        size_t cond_end = lex->pos - lex->curr.len;
        if (lex->curr.kind == KILN_DYN_SEMI) kiln_dyn_lexer_next(lex);

        size_t step_start = lex->pos - lex->curr.len;
        pdepth = 1;
        while (lex->curr.kind != KILN_DYN_EOF && pdepth > 0) {
            if (lex->curr.kind == KILN_DYN_LPAREN) pdepth++;
            else if (lex->curr.kind == KILN_DYN_RPAREN) {
                pdepth--;
                if (pdepth == 0) break;
            }
            kiln_dyn_lexer_next(lex);
        }
        size_t step_end = lex->pos - lex->curr.len;
        if (lex->curr.kind == KILN_DYN_RPAREN) kiln_dyn_lexer_next(lex);

        size_t body_start = lex->curr.start != NULL
                                ? (size_t)(lex->curr.start - lex->src)
                                : lex->pos;
        kiln_dyn_skip_statement(lex);
        size_t body_end = lex->curr.start != NULL
                              ? (size_t)(lex->curr.start - lex->src)
                              : lex->len;

        const char *cond_src = &lex->src[cond_start];
        size_t cond_len = (cond_end > cond_start) ? (cond_end - cond_start) : 0;
        const char *step_src = &lex->src[step_start];
        size_t step_len = (step_end > step_start) ? (step_end - step_start) : 0;
        const char *body_src = &lex->src[body_start];
        size_t body_len = (body_end > body_start) ? (body_end - body_start) : 0;

        while (1) {
            if (cond_len > 0) {
                kiln_dyn_lexer clex;
                memset(&clex, 0, sizeof clex);
                clex.src = cond_src;
                clex.len = cond_len;
                clex.strict = scope->strict;
                kiln_dyn_lexer_next(&clex);
                kiln_value cval;
                int status = kiln_dyn_eval_expr(host, &clex, scope, receiver, args, argc, depth, &cval, thrown);
                kiln_dyn_token_free(&clex.curr);
                if (status != KILN_C99_OK) return status;
                if (!kiln_c99_truthy(cval)) break;
            }
            if (body_len > 0) {
                kiln_dyn_lexer blex;
                memset(&blex, 0, sizeof blex);
                blex.src = body_src;
                blex.len = body_len;
                blex.strict = scope->strict;
                blex.pos = 0;
                blex.break_label = lex->break_label;
                blex.break_label_len = lex->break_label_len;
                kiln_dyn_lexer_next(&blex);
                int break_loop = 0;
                while (blex.curr.kind != KILN_DYN_EOF) {
                    int flow = 0;
                    int status = kiln_dyn_eval_stmt(host, &blex, scope, receiver, args, argc, depth, result, &flow, thrown);
                    lex->break_label = blex.break_label;
                    lex->break_label_len = blex.break_label_len;
                    if (status != KILN_C99_OK || flow == 1) {
                        *returned = flow;
                        kiln_dyn_token_free(&blex.curr);
                        return status;
                    }
                    if (flow == 2) {
                        if (lex->break_label != NULL) {
                            if (kiln_dyn_label_matches(my_labels, lex->break_label,
                                                       lex->break_label_len)) {
                                lex->break_label = NULL;
                                lex->break_label_len = 0;
                            } else {
                                *returned = 2;
                            }
                        }
                        break_loop = 1;
                        kiln_dyn_token_free(&blex.curr);
                        break;
                    }
                    if (flow == 3) {
                        if (lex->break_label != NULL) {
                            if (kiln_dyn_label_matches(my_labels, lex->break_label,
                                                       lex->break_label_len)) {
                                lex->break_label = NULL;
                                lex->break_label_len = 0;
                            } else {
                                *returned = 3;
                            }
                        }
                        kiln_dyn_token_free(&blex.curr);
                        break;
                    }
                }
                kiln_dyn_token_free(&blex.curr);
                if (*returned == 3) return KILN_C99_OK;
                if (break_loop) break;
            }
            if (step_len > 0) {
                kiln_dyn_lexer slex;
                memset(&slex, 0, sizeof slex);
                slex.src = step_src;
                slex.len = step_len;
                slex.strict = scope->strict;
                slex.pos = 0;
                kiln_dyn_lexer_next(&slex);
                while (slex.curr.kind != KILN_DYN_EOF) {
                    kiln_value dummy;
                    int status = kiln_dyn_eval_expr(host, &slex, scope, receiver, args, argc, depth, &dummy, thrown);
                    if (status != KILN_C99_OK) {
                        kiln_dyn_token_free(&slex.curr);
                        return status;
                    }
                    if (slex.curr.kind == KILN_DYN_COMMA) {
                        kiln_dyn_lexer_next(&slex);
                    } else {
                        break;
                    }
                }
                kiln_dyn_token_free(&slex.curr);
            }
        }
        if (*returned == 2 && lex->break_label != NULL &&
            kiln_dyn_label_matches(my_labels, lex->break_label, lex->break_label_len)) {
            lex->break_label = NULL;
            lex->break_label_len = 0;
            *returned = 0;
        }
        return KILN_C99_OK;
    }
    if (lex->curr.kind == KILN_DYN_KW_VAR || lex->curr.kind == KILN_DYN_KW_LET || lex->curr.kind == KILN_DYN_KW_CONST) {
        int is_var = lex->curr.kind == KILN_DYN_KW_VAR;
        int is_const = lex->curr.kind == KILN_DYN_KW_CONST;
        kiln_dyn_lexer_next(lex);
        for (;;) {
            if (lex->curr.kind != KILN_DYN_IDENT) {
                return kiln_dyn_throw_parse_error(lex, thrown,
                                            "Expected identifier");
            }
            const char *name = lex->curr.start;
            size_t name_len = lex->curr.len;
            kiln_dyn_lexer_next(lex);
            kiln_value init = kiln_c99_undefined();
            int has_initializer = lex->curr.kind == KILN_DYN_ASSIGN;
            if (has_initializer) {
                kiln_dyn_lexer_next(lex);
                int status = kiln_dyn_eval_expr(host, lex, scope, receiver, args, argc, depth,
                                                &init, thrown);
                if (status != KILN_C99_OK) return status;
            }
            kiln_dyn_scope *decl_scope = is_var && scope->var_target != NULL
                                             ? scope->var_target : scope;
            int existing_local = 0;
            for (size_t index = 0; index < decl_scope->count; ++index) {
                if (decl_scope->bindings[index].name_len == name_len &&
                    memcmp(decl_scope->bindings[index].name, name, name_len) == 0) {
                    existing_local = 1;
                    break;
                }
            }
            /* `var x;` does not overwrite a parameter or hoisted function declaration. */
            if ((!is_var || has_initializer || !existing_local) &&
                !kiln_dyn_scope_set(decl_scope, name, name_len, init))
                return KILN_C99_OUT_OF_MEMORY;
            if (is_const)
                (void)kiln_dyn_scope_mark_restriction(decl_scope, name, name_len,
                                                      KILN_DYN_BINDING_READ_ONLY);
            else if (!is_var)
                (void)kiln_dyn_scope_mark_restriction(decl_scope, name, name_len,
                                                      KILN_DYN_BINDING_LEXICAL);
            if (lex->curr.kind != KILN_DYN_COMMA) break;
            kiln_dyn_lexer_next(lex);
        }
        if (lex->curr.kind == KILN_DYN_SEMI) kiln_dyn_lexer_next(lex);
        return KILN_C99_OK;
    }
    if (lex->curr.kind == KILN_DYN_KW_FUNCTION)
        return kiln_dyn_consume_function_declaration(lex, scope, host, depth, thrown);
    if (lex->curr.kind == KILN_DYN_KW_ASYNC) {
        kiln_dyn_lexer look;
        memset(&look, 0, sizeof look);
        look.src = lex->src;
        look.len = lex->len;
        look.pos = lex->pos;
        look.prev_kind = lex->prev_kind;
        kiln_dyn_lexer_next(&look);
        int async_declaration = look.curr.kind == KILN_DYN_KW_FUNCTION &&
            !kiln_dyn_has_line_terminator(
                lex->src, lex->pos, (size_t)(look.curr.start - lex->src));
        kiln_dyn_token_free(&look.curr);
        if (async_declaration)
            return kiln_dyn_consume_function_declaration(lex, scope, host, depth, thrown);
    }
    if (lex->curr.kind == KILN_DYN_LBRACE) {
        const char *body = NULL;
        size_t body_len = 0;
        if (!kiln_dyn_capture_block(lex, &body, &body_len))
            return kiln_dyn_throw_parse_error(lex, thrown, "Expected '}' after block");
        kiln_dyn_scope block_scope;
        kiln_dyn_scope_init(&block_scope, scope);
        block_scope.var_target = scope->var_target != NULL ? scope->var_target : scope;
        int status = kiln_dyn_scope_predeclare_lexicals(&block_scope, body, body_len, SIZE_MAX);
        if (status != KILN_C99_OK) {
            kiln_dyn_scope_destroy(&block_scope);
            return status;
        }
        if (kiln_dyn_source_may_have_function_declaration(body, body_len)) {
            kiln_dyn_switch_clause block_clause = {
                .test = NULL, .test_len = 0, .body = body, .body_len = body_len,
                .is_default = 0,
            };
            kiln_dyn_switch_binding *block_bindings = NULL;
            size_t block_binding_count = 0;
            int binding_status = kiln_dyn_switch_bindings(&block_clause, 1,
                                                           &block_bindings,
                                                           &block_binding_count);
            if (binding_status <= 0) {
                free(block_bindings);
                kiln_dyn_scope_destroy(&block_scope);
                return binding_status < 0
                           ? KILN_C99_OUT_OF_MEMORY
                           : kiln_dyn_throw_parse_error(
                                 lex, thrown, "Invalid or duplicate block lexical declaration");
            }
            size_t function_count = 0;
            for (size_t index = 0; index < block_binding_count; ++index) {
                if (!block_bindings[index].is_function) continue;
                if (function_count != index)
                    block_bindings[function_count] = block_bindings[index];
                function_count++;
            }
            if (function_count != 0) {
                status = kiln_dyn_switch_predeclare(&block_scope, block_bindings, function_count);
                if (status == KILN_C99_OK)
                    status = kiln_dyn_switch_initialize_functions(
                        host, &block_scope, &block_clause, 1, block_bindings, function_count,
                        receiver, args, argc, depth, thrown);
            }
            free(block_bindings);
        }
        if (status != KILN_C99_OK) {
            kiln_dyn_scope_destroy(&block_scope);
            return status;
        }
        kiln_dyn_lexer block;
        memset(&block, 0, sizeof block);
        block.src = body;
        block.len = body_len;
        block.strict = block_scope.strict;
        block.break_label = lex->break_label;
        block.break_label_len = lex->break_label_len;
        block.active_labels = lex->active_labels;
        kiln_dyn_lexer_next(&block);
        while (block.curr.kind != KILN_DYN_EOF && *returned == 0) {
            status = kiln_dyn_eval_stmt(host, &block, &block_scope, receiver, args, argc,
                                        depth, result, returned, thrown);
            if (status != KILN_C99_OK) break;
        }
        lex->break_label = block.break_label;
        lex->break_label_len = block.break_label_len;
        kiln_dyn_token_free(&block.curr);
        kiln_dyn_scope_destroy(&block_scope);
        return status;
    }

    kiln_value expr_val;
    int status = kiln_dyn_eval_expr(host, lex, scope, receiver, args, argc, depth, &expr_val, thrown);
    if (status != KILN_C99_OK) return status;
    if (lex->curr.kind == KILN_DYN_SEMI) kiln_dyn_lexer_next(lex);
    return KILN_C99_OK;
}

typedef struct {
    const char *name;
    size_t name_len;
    size_t offset;
} kiln_dyn_function_declaration;

typedef struct {
    const char *name;
    size_t name_len;
} kiln_dyn_var_declaration;

static int kiln_dyn_function_statement_boundary(kiln_dyn_token_kind kind) {
    return kind == KILN_DYN_EOF || kind == KILN_DYN_SEMI || kind == KILN_DYN_RBRACE;
}

static int kiln_dyn_collect_lexicals(const char *source, size_t source_len,
                                    kiln_dyn_lexical_declaration **out,
                                    size_t *out_count) {
    size_t capacity = 0;
    *out = NULL;
    *out_count = 0;
    kiln_dyn_lexer scan;
    memset(&scan, 0, sizeof scan);
    scan.src = source;
    scan.len = source_len;
    kiln_dyn_lexer_next(&scan);
    int brace_depth = 0, callable_depth = 0, paren_depth = 0;
    int function_header = 0, function_paren_depth = -1, pending_callable_body = 0;
    int previous_previous_is_method_key = 0, previous_is_control_head = 0;
    kiln_dyn_token_kind previous = KILN_DYN_EOF;
    int *callable_braces = NULL;
    size_t callable_capacity = 0;
    unsigned char *callable_parens = NULL;
    size_t paren_capacity = 0;
    size_t *scope_starts = NULL;
    size_t scope_count = 0, scope_capacity = 0;
    while (scan.curr.kind != KILN_DYN_EOF) {
        kiln_dyn_token_kind kind = scan.curr.kind;
        if ((kind == KILN_DYN_KW_LET || kind == KILN_DYN_KW_CONST) &&
            callable_depth == 0 && paren_depth == 0 && scan.prev_kind != KILN_DYN_DOT) {
            kiln_dyn_lexer look = scan;
            kiln_dyn_lexer_next(&look);
            for (;;) {
                if (look.curr.kind != KILN_DYN_IDENT) break;
                if (*out_count == capacity) {
                    size_t next_capacity = capacity == 0 ? 8 : capacity * 2;
                    if (next_capacity <= capacity ||
                        next_capacity > SIZE_MAX / sizeof **out) {
                        kiln_dyn_token_free(&look.curr);
                        goto oom;
                    }
                    kiln_dyn_lexical_declaration *next = realloc(
                        *out, next_capacity * sizeof *next);
                    if (next == NULL) {
                        kiln_dyn_token_free(&look.curr);
                        goto oom;
                    }
                    *out = next;
                    capacity = next_capacity;
                }
                (*out)[(*out_count)++] = (kiln_dyn_lexical_declaration){
                    .name = look.curr.start,
                    .name_len = look.curr.len,
                    .scope_id = scope_count == 0 ? SIZE_MAX : scope_starts[scope_count - 1],
                    .is_const = kind == KILN_DYN_KW_CONST,
                };
                kiln_dyn_lexer_next(&look);
                if (look.curr.kind == KILN_DYN_ASSIGN) {
                    kiln_dyn_lexer_next(&look);
                    int init_parens = 0, init_brackets = 0, init_braces = 0;
                    size_t previous_end = look.curr.start == NULL ? look.pos :
                        (size_t)(look.curr.start - source) + look.curr.len;
                    while (look.curr.kind != KILN_DYN_EOF) {
                        size_t token_start = look.curr.start == NULL ? look.pos :
                            (size_t)(look.curr.start - source);
                        if (init_parens == 0 && init_brackets == 0 && init_braces == 0) {
                            if (look.curr.kind == KILN_DYN_COMMA ||
                                look.curr.kind == KILN_DYN_SEMI ||
                                look.curr.kind == KILN_DYN_RBRACE)
                                break;
                            if ((look.curr.kind == KILN_DYN_KW_LET ||
                                 look.curr.kind == KILN_DYN_KW_CONST ||
                                 look.curr.kind == KILN_DYN_KW_VAR) &&
                                kiln_dyn_has_line_terminator(source, previous_end, token_start))
                                break;
                        }
                        if (look.curr.kind == KILN_DYN_LPAREN) init_parens++;
                        else if (look.curr.kind == KILN_DYN_RPAREN && init_parens > 0)
                            init_parens--;
                        else if (look.curr.kind == KILN_DYN_LBRACKET) init_brackets++;
                        else if (look.curr.kind == KILN_DYN_RBRACKET && init_brackets > 0)
                            init_brackets--;
                        else if (look.curr.kind == KILN_DYN_LBRACE) init_braces++;
                        else if (look.curr.kind == KILN_DYN_RBRACE && init_braces > 0)
                            init_braces--;
                        previous_end = token_start + look.curr.len;
                        kiln_dyn_lexer_next(&look);
                    }
                }
                if (look.curr.kind != KILN_DYN_COMMA) break;
                kiln_dyn_lexer_next(&look);
            }
            kiln_dyn_token_free(&look.curr);
        }
        if (kind == KILN_DYN_LBRACE) {
            if (scope_count == scope_capacity) {
                size_t next_capacity = scope_capacity == 0 ? 16 : scope_capacity * 2;
                if (next_capacity <= scope_capacity ||
                    next_capacity > SIZE_MAX / sizeof *scope_starts)
                    goto oom;
                size_t *next = realloc(scope_starts, next_capacity * sizeof *next);
                if (next == NULL) goto oom;
                scope_starts = next;
                scope_capacity = next_capacity;
            }
            size_t brace_start = scan.curr.start == NULL ? scan.pos :
                (size_t)(scan.curr.start - source);
            scope_starts[scope_count++] = brace_start;
        }
        if (kiln_dyn_callable_scan_advance(
                &scan, previous, &brace_depth, &callable_depth, &paren_depth,
                &function_header, &function_paren_depth, &pending_callable_body,
                &previous_previous_is_method_key, &previous_is_control_head,
                &callable_braces, &callable_capacity, &callable_parens, &paren_capacity) < 0)
            goto oom;
        previous = kind;
        if (kind == KILN_DYN_RBRACE && scope_count != 0) scope_count--;
        kiln_dyn_lexer_next(&scan);
    }
    kiln_dyn_token_free(&scan.curr);
    free(callable_braces);
    free(callable_parens);
    free(scope_starts);
    return 1;
oom:
    if (scan.curr.kind != KILN_DYN_EOF) kiln_dyn_token_free(&scan.curr);
    free(callable_braces);
    free(callable_parens);
    free(scope_starts);
    free(*out);
    *out = NULL;
    *out_count = 0;
    return -1;
}

static int kiln_dyn_scope_predeclare_lexicals(kiln_dyn_scope *scope,
                                               const char *source, size_t source_len,
                                               size_t target_scope_id) {
    kiln_dyn_lexical_declaration *lexicals = NULL;
    size_t count = 0;
    if (kiln_dyn_collect_lexicals(source, source_len, &lexicals, &count) < 0)
        return KILN_C99_OUT_OF_MEMORY;
    for (size_t index = 0; index < count; ++index) {
        if (lexicals[index].scope_id != target_scope_id) continue;
        int duplicate = 0;
        for (size_t prior = 0; prior < index; ++prior) {
            if (lexicals[prior].scope_id == target_scope_id &&
                lexicals[prior].name_len == lexicals[index].name_len &&
                memcmp(lexicals[prior].name, lexicals[index].name,
                       lexicals[index].name_len) == 0) {
                duplicate = 1;
                break;
            }
        }
        if (duplicate) continue;
        int already_present = 0;
        for (size_t slot = 0; slot < scope->count; ++slot) {
            if (scope->bindings[slot].name_len == lexicals[index].name_len &&
                memcmp(scope->bindings[slot].name, lexicals[index].name,
                       lexicals[index].name_len) == 0) {
                already_present = 1;
                break;
            }
        }
        if (already_present) continue;
        if (!kiln_dyn_scope_set(scope, lexicals[index].name, lexicals[index].name_len,
                                kiln_c99_uninitialized())) {
            free(lexicals);
            return KILN_C99_OUT_OF_MEMORY;
        }
        for (size_t slot = 0; slot < scope->count; ++slot) {
            if (scope->bindings[slot].name_len != lexicals[index].name_len ||
                memcmp(scope->bindings[slot].name, lexicals[index].name,
                       lexicals[index].name_len) != 0)
                continue;
            int status = kiln_c99_make_cell(&scope->bindings[slot].value);
            if (status != KILN_C99_OK) {
                free(lexicals);
                return status;
            }
            kiln_value rooted_cell = scope->bindings[slot].value;
            kiln_c99_gc_keep_alive(kiln_c99_gc_value_pointer(rooted_cell));
            status = kiln_c99_put(NULL, scope->roots, scope->bindings[slot].name,
                                  scope->bindings[slot].name_len, rooted_cell, 0, NULL);
            if (status != KILN_C99_OK) {
                free(lexicals);
                return status;
            }
            break;
        }
        if (!kiln_dyn_scope_mark_restriction(scope, lexicals[index].name,
                lexicals[index].name_len,
                lexicals[index].is_const ? KILN_DYN_BINDING_READ_ONLY
                                         : KILN_DYN_BINDING_LEXICAL)) {
            free(lexicals);
            return KILN_C99_OUT_OF_MEMORY;
        }
    }
    free(lexicals);
    return KILN_C99_OK;
}

/* Most dynamic blocks contain no function declarations. Avoid building the full declaration
 * inventory for those blocks; this token-spelling precheck is intentionally permissive (comments
 * and strings may cause a false positive) but cannot skip a real function declaration. */
static int kiln_dyn_source_may_have_function_declaration(const char *source, size_t source_len) {
    static const char keyword[] = "function";
    if (source_len < sizeof keyword - 1) return 0;
    for (size_t index = 0; index <= source_len - (sizeof keyword - 1); ++index) {
        if (memcmp(source + index, keyword, sizeof keyword - 1) == 0) return 1;
    }
    return 0;
}

/* Add eligible sloppy CaseBlock function names to the function environment at declaration
 * instantiation time. Their values remain undefined until execution reaches the declaration. */
static int kiln_dyn_hoist_switch_aliases(kiln_dyn_lexer *source_lex, kiln_dyn_scope *scope,
                                         size_t scope_id,
                                         const kiln_dyn_lexical_declaration *lexicals,
                                         size_t lexical_count,
                                         kiln_dyn_var_declaration **vars, size_t *var_count,
                                         size_t *var_capacity) {
    if (source_lex->curr.kind != KILN_DYN_KW_SWITCH || scope->strict) return 1;
    kiln_dyn_lexer scan = *source_lex;
    kiln_dyn_lexer_next(&scan);
    if (scan.curr.kind != KILN_DYN_LPAREN) {
        kiln_dyn_token_free(&scan.curr);
        return 1;
    }
    int parens = 1, brackets = 0, braces = 0;
    kiln_dyn_lexer_next(&scan);
    while (scan.curr.kind != KILN_DYN_EOF && parens != 0) {
        if (scan.curr.kind == KILN_DYN_LPAREN) parens++;
        else if (scan.curr.kind == KILN_DYN_RPAREN) {
            parens--;
            if (parens == 0) break;
        }
        else if (scan.curr.kind == KILN_DYN_LBRACKET) brackets++;
        else if (scan.curr.kind == KILN_DYN_RBRACKET && brackets > 0) brackets--;
        else if (scan.curr.kind == KILN_DYN_LBRACE) braces++;
        else if (scan.curr.kind == KILN_DYN_RBRACE && braces > 0) braces--;
        kiln_dyn_lexer_next(&scan);
    }
    if (scan.curr.kind != KILN_DYN_RPAREN) {
        kiln_dyn_token_free(&scan.curr);
        return 1;
    }
    kiln_dyn_lexer_next(&scan);
    const char *body = NULL;
    size_t body_len = 0;
    if (!kiln_dyn_capture_block(&scan, &body, &body_len)) {
        kiln_dyn_token_free(&scan.curr);
        return 1;
    }
    kiln_dyn_switch_clause *clauses = NULL;
    size_t clause_count = 0;
    int clause_status = kiln_dyn_switch_clauses(body, body_len, &clauses, &clause_count);
    if (clause_status <= 0) {
        free(clauses);
        kiln_dyn_token_free(&scan.curr);
        return clause_status < 0 ? -1 : 1;
    }
    kiln_dyn_switch_binding *bindings = NULL;
    size_t binding_count = 0;
    int binding_status = kiln_dyn_switch_bindings(clauses, clause_count,
                                                   &bindings, &binding_count);
    free(clauses);
    kiln_dyn_token_free(&scan.curr);
    if (binding_status <= 0) {
        free(bindings);
        return binding_status < 0 ? -1 : 1;
    }
    for (size_t index = 0; index < binding_count; ++index) {
        if (!bindings[index].is_function) continue;
        int lexical_collision = 0;
        for (size_t lexical = 0; lexical < lexical_count; ++lexical) {
            if (lexicals[lexical].scope_id == scope_id &&
                lexicals[lexical].name_len == bindings[index].name_len &&
                memcmp(lexicals[lexical].name, bindings[index].name,
                       bindings[index].name_len) == 0) {
                lexical_collision = 1;
                break;
            }
        }
        if (lexical_collision) continue;
        unsigned char restriction = KILN_DYN_BINDING_MUTABLE;
        int local = 0;
        for (size_t slot = 0; slot < scope->count; ++slot) {
            if (scope->bindings[slot].name_len == bindings[index].name_len &&
                memcmp(scope->bindings[slot].name, bindings[index].name,
                       bindings[index].name_len) == 0) {
                restriction = scope->bindings[slot].restriction;
                local = 1;
                break;
            }
        }
        if (local && restriction != KILN_DYN_BINDING_MUTABLE &&
            restriction != KILN_DYN_BINDING_ANNEX_B_VAR)
            continue;
        int duplicate = 0;
        for (size_t prior = 0; prior < *var_count; ++prior) {
            if ((*vars)[prior].name_len == bindings[index].name_len &&
                memcmp((*vars)[prior].name, bindings[index].name,
                       bindings[index].name_len) == 0) {
                duplicate = 1;
                break;
            }
        }
        if (duplicate) continue;
        if (*var_count == *var_capacity) {
            size_t capacity = *var_capacity == 0 ? 8 : *var_capacity * 2;
            if (capacity <= *var_capacity || capacity > SIZE_MAX / sizeof **vars) {
                free(bindings);
                return -1;
            }
            kiln_dyn_var_declaration *next = realloc(*vars, capacity * sizeof *next);
            if (next == NULL) {
                free(bindings);
                return -1;
            }
            *vars = next;
            *var_capacity = capacity;
        }
        (*vars)[(*var_count)++] = (kiln_dyn_var_declaration){
            .name = bindings[index].name, .name_len = bindings[index].name_len,
        };
    }
    free(bindings);
    return 1;
}

/* Function declarations are initialized before the first body statement. Pre-create their
 * mutable cells, then make each closure against that shared scope so forward and mutual
 * references resolve to the initialized declaration. This scanner intentionally recognizes
 * declarations at the function-body level only; block lexical declarations remain a separate
 * scope feature and are not reinterpreted as function-scoped bindings. */
static int kiln_dyn_hoist_function_declarations(kiln_host *host, const char *source,
                                                 size_t source_len, kiln_dyn_scope *scope,
                                                 kiln_value receiver, const kiln_value *args,
                                                 size_t argc, int depth, kiln_value *thrown) {
    kiln_dyn_function_declaration *decls = NULL;
    size_t decl_count = 0;
    size_t decl_capacity = 0;
    kiln_dyn_var_declaration *vars = NULL;
    size_t var_count = 0;
    size_t var_capacity = 0;
    kiln_dyn_lexical_declaration *lexicals = NULL;
    size_t lexical_count = 0;
    size_t *scope_starts = NULL;
    size_t scope_count = 0;
    size_t scope_capacity = 0;
    int *callable_braces = NULL;
    size_t callable_brace_capacity = 0;
    unsigned char *callable_parens = NULL;
    size_t callable_paren_capacity = 0;
    kiln_dyn_lexer scan;
    int brace_depth = 0;
    int callable_depth = 0;
    int paren_depth = 0;
    int function_header = 0;
    int function_paren_depth = -1;
    int pending_callable_body = 0;
    int previous_previous_is_method_key = 0;
    int previous_is_control_head = 0;
    int statement_start = 1;
    kiln_dyn_token_kind previous = KILN_DYN_EOF;
    size_t previous_end = 0;
    int status = KILN_C99_OK;

    memset(&scan, 0, sizeof scan);
    if (kiln_dyn_collect_lexicals(source, source_len, &lexicals, &lexical_count) < 0) {
        status = KILN_C99_OUT_OF_MEMORY;
        goto done;
    }

    scan.src = source;
    scan.len = source_len;
    kiln_dyn_lexer_next(&scan);
    while (scan.curr.kind != KILN_DYN_EOF) {
        kiln_dyn_token_kind kind = scan.curr.kind;
        size_t token_start = scan.curr.start == NULL
                                 ? scan.pos
                                 : (size_t)(scan.curr.start - source);
        if (kind == KILN_DYN_KW_SWITCH && callable_depth == 0 && !scope->strict) {
            size_t scope_id = scope_count == 0 ? SIZE_MAX : scope_starts[scope_count - 1];
            int alias_status = kiln_dyn_hoist_switch_aliases(&scan, scope, scope_id,
                                                              lexicals, lexical_count, &vars,
                                                              &var_count, &var_capacity);
            if (alias_status < 0) {
                status = KILN_C99_OUT_OF_MEMORY;
                goto done;
            }
        }
        int line_terminated = previous_end <= token_start && previous_end < source_len &&
                              memchr(source + previous_end, '\n', token_start - previous_end) !=
                                  NULL &&
                              (previous == KILN_DYN_IDENT || previous == KILN_DYN_NUM ||
                               previous == KILN_DYN_BIGINT ||
                               previous == KILN_DYN_STR || previous == KILN_DYN_RPAREN ||
                               previous == KILN_DYN_RBRACKET ||
                               previous == KILN_DYN_KW_RETURN ||
                               previous == KILN_DYN_KW_ASYNC);
        int declaration_start = statement_start ||
                                kiln_dyn_function_statement_boundary(previous) ||
                                line_terminated;
        /* `var` bindings exist before function declarations are closed over this scope. The
         * dynamic evaluator executes declarations sequentially, so without this instantiation
         * pass an earlier-created function misses a later `var` binding entirely. */
        if (kind == KILN_DYN_KW_VAR && callable_depth == 0) {
            kiln_dyn_lexer look;
            memset(&look, 0, sizeof look);
            look.src = source;
            look.len = source_len;
            look.pos = token_start;
            kiln_dyn_lexer_next(&look);
            kiln_dyn_lexer_next(&look);
            int expect_name = 1;
            int parens = 0, brackets = 0, braces = 0;
            while (look.curr.kind != KILN_DYN_EOF) {
                if (expect_name && look.curr.kind == KILN_DYN_IDENT) {
                    if (var_count == var_capacity) {
                        size_t capacity = var_capacity == 0 ? 8 : var_capacity * 2;
                        kiln_dyn_var_declaration *next = (kiln_dyn_var_declaration *)realloc(
                            vars, capacity * sizeof *next);
                        if (next == NULL) {
                            kiln_dyn_token_free(&look.curr);
                            status = KILN_C99_OUT_OF_MEMORY;
                            goto done;
                        }
                        vars = next;
                        var_capacity = capacity;
                    }
                    vars[var_count++] = (kiln_dyn_var_declaration){
                        .name = look.curr.start, .name_len = look.curr.len,
                    };
                    expect_name = 0;
                    kiln_dyn_lexer_next(&look);
                    continue;
                }
                if (parens == 0 && brackets == 0 && braces == 0) {
                    if (look.curr.kind == KILN_DYN_COMMA) {
                        expect_name = 1;
                        kiln_dyn_lexer_next(&look);
                        continue;
                    }
                    if (look.curr.kind == KILN_DYN_SEMI || look.curr.kind == KILN_DYN_RPAREN ||
                        look.curr.kind == KILN_DYN_RBRACE || look.curr.kind == KILN_DYN_KW_IN ||
                        look.curr.kind == KILN_DYN_KW_OF)
                        break;
                }
                if (look.curr.kind == KILN_DYN_LPAREN) parens++;
                else if (look.curr.kind == KILN_DYN_RPAREN && parens > 0) parens--;
                else if (look.curr.kind == KILN_DYN_LBRACKET) brackets++;
                else if (look.curr.kind == KILN_DYN_RBRACKET && brackets > 0) brackets--;
                else if (look.curr.kind == KILN_DYN_LBRACE) braces++;
                else if (look.curr.kind == KILN_DYN_RBRACE && braces > 0) braces--;
                kiln_dyn_lexer_next(&look);
            }
            kiln_dyn_token_free(&look.curr);
        }
        if (brace_depth == 0 && declaration_start &&
            (kind == KILN_DYN_KW_FUNCTION || kind == KILN_DYN_KW_ASYNC)) {
            kiln_dyn_lexer look;
            memset(&look, 0, sizeof look);
            look.src = source;
            look.len = source_len;
            look.pos = token_start;
            kiln_dyn_lexer_next(&look);
            int async_declaration = kind == KILN_DYN_KW_ASYNC;
            if (async_declaration) {
                size_t async_end = look.pos;
                kiln_dyn_lexer_next(&look);
                if (look.curr.kind != KILN_DYN_KW_FUNCTION ||
                    kiln_dyn_has_line_terminator(
                        source, async_end, (size_t)(look.curr.start - source))) {
                    kiln_dyn_token_free(&look.curr);
                    goto declaration_scan_done;
                }
            }
            kiln_dyn_lexer_next(&look);
            if (look.curr.kind == KILN_DYN_IDENT) {
                if (decl_count == decl_capacity) {
                    size_t capacity = decl_capacity == 0 ? 8 : decl_capacity * 2;
                    kiln_dyn_function_declaration *next =
                        (kiln_dyn_function_declaration *)realloc(
                            decls, capacity * sizeof *next);
                    if (next == NULL) {
                        kiln_dyn_token_free(&look.curr);
                        status = KILN_C99_OUT_OF_MEMORY;
                        goto done;
                    }
                    decls = next;
                    decl_capacity = capacity;
                }
                decls[decl_count++] = (kiln_dyn_function_declaration){
                    .name = look.curr.start,
                    .name_len = look.curr.len,
                    .offset = token_start,
                };
            }
            kiln_dyn_token_free(&look.curr);
        }
declaration_scan_done:
        if (kiln_dyn_callable_scan_advance(
                &scan, previous, &brace_depth, &callable_depth, &paren_depth,
                &function_header, &function_paren_depth, &pending_callable_body,
                &previous_previous_is_method_key, &previous_is_control_head,
                &callable_braces, &callable_brace_capacity,
                &callable_parens, &callable_paren_capacity) < 0) {
            status = KILN_C99_OUT_OF_MEMORY;
            goto done;
        }
        if (kind == KILN_DYN_LBRACE) {
            if (scope_count == scope_capacity) {
                size_t capacity = scope_capacity == 0 ? 16 : scope_capacity * 2;
                if (capacity <= scope_capacity || capacity > SIZE_MAX / sizeof *scope_starts) {
                    status = KILN_C99_OUT_OF_MEMORY;
                    goto done;
                }
                size_t *next = realloc(scope_starts, capacity * sizeof *next);
                if (next == NULL) {
                    status = KILN_C99_OUT_OF_MEMORY;
                    goto done;
                }
                scope_starts = next;
                scope_capacity = capacity;
            }
            scope_starts[scope_count++] = token_start;
        } else if (kind == KILN_DYN_RBRACE && scope_count != 0) {
            scope_count--;
        }
        if (kind == KILN_DYN_LBRACE && brace_depth == 1) {
            statement_start = 0;
        } else if (kind == KILN_DYN_RBRACE && brace_depth == 0) {
            statement_start = 1;
        } else if (kind == KILN_DYN_SEMI && brace_depth == 0) {
            statement_start = 1;
        } else if (brace_depth == 0) {
            statement_start = 0;
        }
        previous = kind;
        previous_end = scan.pos;
        kiln_dyn_lexer_next(&scan);
    }
    kiln_dyn_token_free(&scan.curr);

    status = kiln_dyn_scope_predeclare_lexicals(scope, source, source_len, SIZE_MAX);
    if (status != KILN_C99_OK) goto done;

    /* Preserve parameters and existing local declarations. These bindings must exist before
     * the declaration closures below capture the surrounding environment. */
    for (size_t index = 0; index < var_count; ++index) {
        int already_local = 0;
        for (size_t slot = 0; slot < scope->count; ++slot) {
            if (scope->bindings[slot].name_len == vars[index].name_len &&
                memcmp(scope->bindings[slot].name, vars[index].name,
                       vars[index].name_len) == 0) {
                already_local = 1;
                break;
            }
        }
        if (!already_local &&
            !kiln_dyn_scope_set(scope, vars[index].name, vars[index].name_len,
                                kiln_c99_undefined())) {
            status = KILN_C99_OUT_OF_MEMORY;
            goto done;
        }
    }

    /* Allocate all local declaration cells before any closure captures the environment. */
    for (size_t index = 0; index < decl_count; ++index) {
        if (!kiln_dyn_scope_add_function_declaration(scope,
                                                     source + decls[index].offset)) {
            status = KILN_C99_OUT_OF_MEMORY;
            goto done;
        }
        int found = 0;
        for (size_t prior = 0; prior < index; ++prior) {
            if (decls[prior].name_len == decls[index].name_len &&
                memcmp(decls[prior].name, decls[index].name, decls[index].name_len) == 0) {
                found = 1;
                break;
            }
        }
        if (found) continue;
        if (!kiln_dyn_scope_set(scope, decls[index].name, decls[index].name_len,
                                kiln_c99_undefined())) {
            status = KILN_C99_OUT_OF_MEMORY;
            goto done;
        }
        for (size_t slot = 0; slot < scope->count; ++slot) {
            if (scope->bindings[slot].name_len == decls[index].name_len &&
                memcmp(scope->bindings[slot].name, decls[index].name,
                       decls[index].name_len) == 0) {
                if (scope->bindings[slot].value.tag != KILN_VALUE_CELL) {
                    status = kiln_c99_make_cell(&scope->bindings[slot].value);
                    if (status != KILN_C99_OK) goto done;
                    kiln_value rooted_cell = scope->bindings[slot].value;
                    kiln_c99_gc_keep_alive(kiln_c99_gc_value_pointer(rooted_cell));
                    status = kiln_c99_put(NULL, scope->roots, scope->bindings[slot].name,
                                          scope->bindings[slot].name_len,
                                          rooted_cell, 0, NULL);
                    if (status != KILN_C99_OK) goto done;
                }
                break;
            }
        }
    }

    for (size_t index = 0; index < decl_count; ++index) {
        kiln_dyn_lexer function_lex;
        kiln_value function;
        memset(&function_lex, 0, sizeof function_lex);
        function_lex.src = source;
        function_lex.len = source_len;
        function_lex.pos = decls[index].offset;
        function_lex.function_declaration = 1;
        function_lex.strict = scope->strict;
        kiln_dyn_lexer_next(&function_lex);
        status = kiln_dyn_eval_primary(host, &function_lex, scope, receiver, args, argc,
                                       depth, &function, thrown);
        kiln_dyn_token_free(&function_lex.curr);
        if (status != KILN_C99_OK) goto done;
        if (function.tag != KILN_VALUE_FUNCTION ||
            !kiln_dyn_scope_set(scope, decls[index].name, decls[index].name_len, function)) {
            status = function.tag == KILN_VALUE_FUNCTION ? KILN_C99_OUT_OF_MEMORY
                                                        : KILN_C99_INVALID_ARGUMENT;
            goto done;
        }
        kiln_c99_gc_keep_alive(&function);
    }

done:
    if (scan.curr.kind != KILN_DYN_EOF) kiln_dyn_token_free(&scan.curr);
    free(vars);
    free(decls);
    free(lexicals);
    free(scope_starts);
    free(callable_braces);
    free(callable_parens);
    return status;
}

static int kiln_c99_dynamic_body_trampoline(kiln_call_context *ctx, kiln_val *result, kiln_val *upvalues,
                                            const kiln_val *receiver_ptr, const kiln_val *args, size_t argc) {
    kiln_host *host = ctx ? ctx->host : NULL;
    kiln_value *thrown = ctx ? ctx->thrown : NULL;
    struct kiln_generator *frame = ctx ? ctx->frame : NULL;
    int depth = ctx ? ctx->depth : 0;
    kiln_value receiver = receiver_ptr ? kiln_val_to_boundary(*receiver_ptr) : kiln_c99_undefined();
    (void)frame;
    kiln_value params_val = kiln_val_to_boundary(upvalues[0]);
    kiln_value body_val = kiln_val_to_boundary(upvalues[1]);
    kiln_value env_val = kiln_val_to_boundary(upvalues[2]);
    kiln_value bargs[argc > 0 ? argc : 1];
    for (size_t i = 0; i < argc; ++i) {
        bargs[i] = kiln_val_to_boundary(args[i]);
    }

    /* Parameters and declarations are a new function environment. Keep the captured lexical
     * cells in its parent so a same-named parameter shadows instead of replacing an outer cell. */
    kiln_dyn_scope captured_scope;
    kiln_dyn_scope_init(&captured_scope, NULL);
    kiln_dyn_scope scope;
    kiln_dyn_scope_init(&scope, &captured_scope);

    if (env_val.tag == KILN_VALUE_OBJECT) {
        captured_scope.captured_environment = env_val;
        captured_scope.has_captured_environment = 1;
        int records_present = 0;
        kiln_value records = kiln_c99_undefined();
        if (kiln_dyn_private_environment_get(env_val, "\004", 1, &records,
                                             &records_present) == KILN_C99_OK &&
            records_present && records.tag == KILN_VALUE_ARRAY) {
            captured_scope.captured_records = records;
            captured_scope.has_captured_records = 1;
        }
        int strict_present = 0;
        kiln_value strict_marker = kiln_c99_undefined();
        if (kiln_dyn_private_environment_get(env_val, "\003", 1, &strict_marker,
                                             &strict_present) == KILN_C99_OK &&
            strict_present)
            captured_scope.strict = kiln_c99_truthy(strict_marker);
    }
    scope.strict = captured_scope.strict;



    size_t params_len = 0;
    uint8_t *params_owned = NULL;
    if (kiln_dyn_string_to_source(params_val, &params_owned, &params_len) != KILN_C99_OK) {
        kiln_dyn_scope_destroy(&scope);
        kiln_dyn_scope_destroy(&captured_scope);
        return KILN_C99_OUT_OF_MEMORY;
    }
    const char *params_bytes = (const char *)params_owned;
    if (params_len > 0) {
        const char *p = params_bytes;
        size_t plen = params_len;
        size_t ppos = 0;
        size_t arg_idx = 0;
        while (ppos < plen) {
            while (ppos < plen && (p[ppos] == ' ' || p[ppos] == '\t' || p[ppos] == ',')) ppos++;
            if (ppos >= plen) break;
            if (p[ppos] == '{') {
                kiln_value options_arg = (arg_idx < argc) ? bargs[arg_idx] : kiln_c99_undefined();
                ppos++;
                while (ppos < plen && p[ppos] != '}') {
                    while (ppos < plen && (p[ppos] == ' ' || p[ppos] == '\t' || p[ppos] == ',')) ppos++;
                    if (ppos >= plen || p[ppos] == '}') break;
                    size_t istart = ppos;
                    while (ppos < plen && p[ppos] != '=' && p[ppos] != ',' && p[ppos] != '}' && p[ppos] != ' ' && p[ppos] != '\t') ppos++;
                    size_t iname_len = ppos - istart;
                    while (ppos < plen && (p[ppos] == ' ' || p[ppos] == '\t')) ppos++;
                    kiln_value dval = kiln_c99_undefined();
                    if (ppos < plen && p[ppos] == '=') {
                        ppos++;
                        while (ppos < plen && (p[ppos] == ' ' || p[ppos] == '\t')) ppos++;
                        size_t vstart = ppos;
                        if (ppos < plen && (p[ppos] == '"' || p[ppos] == '\'')) {
                            char q = p[ppos++];
                            vstart = ppos;
                            while (ppos < plen && p[ppos] != q) ppos++;
                            dval = kiln_c99_copy_text(&p[vstart], ppos - vstart);
                            if (ppos < plen) ppos++;
                        } else {
                            while (ppos < plen && p[ppos] != ',' && p[ppos] != '}' && p[ppos] != ' ' && p[ppos] != '\t') ppos++;
                            size_t vlen = ppos - vstart;
                            int scope_found = 0;
                            int scope_status = kiln_dyn_scope_get(host, &scope, &p[vstart],
                                                                  vlen, depth, &dval,
                                                                  &scope_found, thrown);
                            if (scope_status != KILN_C99_OK) {
                                free(params_owned);
                                kiln_dyn_scope_destroy(&scope);
                                kiln_dyn_scope_destroy(&captured_scope);
                                return scope_status;
                            }
                            if (!scope_found) {
                                dval = kiln_c99_undefined();
                            }
                        }
                    }
                    kiln_value actual_val = kiln_c99_undefined();
                    if (options_arg.tag == KILN_VALUE_OBJECT) {
                        (void)kiln_c99_get(host, options_arg, &p[istart], iname_len, &actual_val, depth, NULL);
                    }
                    if (actual_val.tag == KILN_VALUE_UNDEFINED) {
                        actual_val = dval;
                    }
                    kiln_dyn_scope_set(&scope, &p[istart], iname_len, actual_val);
                    (void)kiln_dyn_scope_mark_restriction(
                        &scope, &p[istart], iname_len, KILN_DYN_BINDING_PARAMETER);
                    while (ppos < plen && (p[ppos] == ' ' || p[ppos] == '\t')) ppos++;
                    if (ppos < plen && p[ppos] == ',') ppos++;
                }
                if (ppos < plen && p[ppos] == '}') ppos++;
                arg_idx++;
                continue;
            }
            size_t pstart = ppos;
            while (ppos < plen && p[ppos] != ' ' && p[ppos] != '\t' && p[ppos] != ',' && p[ppos] != '=') ppos++;
            size_t pname_len = ppos - pstart;
            kiln_value arg_val = (arg_idx < argc) ? bargs[arg_idx] : kiln_c99_undefined();
            kiln_dyn_scope_set(&scope, &p[pstart], pname_len, arg_val);
            (void)kiln_dyn_scope_mark_restriction(&scope, &p[pstart], pname_len,
                                                  KILN_DYN_BINDING_PARAMETER);
            while (ppos < plen && p[ppos] != ',') ppos++;
            arg_idx++;
        }
    }

    free(params_owned);
    size_t body_len = 0;
    uint8_t *body_owned = NULL;
    const char *bsrc;
    if (kiln_dyn_string_to_source(body_val, &body_owned, &body_len) != KILN_C99_OK) {
        kiln_dyn_scope_destroy(&scope);
        kiln_dyn_scope_destroy(&captured_scope);
        return KILN_C99_OUT_OF_MEMORY;
    }
    bsrc = (const char *)body_owned;
    size_t blen = body_len;
    scope.strict = captured_scope.strict || kiln_dyn_body_has_use_strict(bsrc, blen);
    int hoist_status = kiln_dyn_hoist_function_declarations(host, bsrc, blen, &scope,
                                                            receiver,
                                                            argc > 0 ? bargs : NULL,
                                                            argc, depth, thrown);
    if (hoist_status != KILN_C99_OK) {
        free(body_owned);
        kiln_dyn_scope_destroy(&scope);
        kiln_dyn_scope_destroy(&captured_scope);
        return hoist_status;
    }
    kiln_dyn_lexer lex;
    memset(&lex, 0, sizeof(lex));
    lex.src = bsrc;
    lex.len = blen;
    lex.pos = 0;
    lex.dynamic_function_body = 1;
    lex.strict = scope.strict;
    kiln_dyn_lexer_next(&lex);

    int returned = 0;
    kiln_value bresult = kiln_c99_undefined();
    int status = KILN_C99_OK;

    while (lex.curr.kind != KILN_DYN_EOF) {
        status = kiln_dyn_eval_stmt(host, &lex, &scope, receiver, argc > 0 ? bargs : NULL, argc, depth, &bresult, &returned, thrown);
        if (status != KILN_C99_OK || returned == 1) {
            break;
        }
    }

    *result = kiln_val_from_boundary(bresult);
    free(body_owned);
    kiln_dyn_token_free(&lex.curr);
    kiln_dyn_scope_destroy(&scope);
    kiln_dyn_scope_destroy(&captured_scope);
    return status;
}

static int kiln_c99_dynamic_new_function(kiln_host *host, kiln_value *result,
                                         const kiln_value *args, size_t argc,
                                         int depth, kiln_value *thrown) {
    kiln_value upvalues[3];
    upvalues[2] = kiln_c99_undefined();
    if (argc == 0) {
        upvalues[0] = kiln_c99_copy_text("", 0);
        kiln_c99_gc_keep_alive(&upvalues[0]);
        upvalues[1] = kiln_c99_copy_text("", 0);
    } else if (argc == 1) {
        upvalues[0] = kiln_c99_copy_text("", 0);
        kiln_c99_gc_keep_alive(&upvalues[0]);
        kiln_value primitive;
        int status = kiln_c99_to_primitive(host, args[0], 1 /* string */,
                                           &primitive, depth, thrown);
        if (status != KILN_C99_OK) return status;
        kiln_c99_gc_keep_alive(&upvalues[0]);
        kiln_c99_gc_keep_alive(&args[0]);
        if (primitive.tag == KILN_VALUE_SYMBOL)
            return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                        "Cannot convert a Symbol value to a string");
        upvalues[1] = kiln_c99_text(primitive);
        kiln_c99_gc_keep_alive(&upvalues[0]);
        kiln_c99_gc_keep_alive(&primitive);
    } else {
        size_t i;
        upvalues[0] = kiln_c99_from_units(NULL, 0);
        kiln_c99_gc_keep_alive(&upvalues[0]);
        for (i = 0; i < argc - 1; ++i) {
            kiln_value primitive;
            kiln_value text;
            int status = kiln_c99_to_primitive(host, args[i], 1 /* string */,
                                               &primitive, depth, thrown);
            if (status != KILN_C99_OK) return status;
            kiln_c99_gc_keep_alive(&upvalues[0]);
            kiln_c99_gc_keep_alive(&args[i]);
            if (primitive.tag == KILN_VALUE_SYMBOL)
                return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                            "Cannot convert a Symbol value to a string");
            text = kiln_c99_text(primitive);
            kiln_c99_gc_keep_alive(&upvalues[0]);
            kiln_c99_gc_keep_alive(&primitive);
            kiln_c99_gc_keep_alive(&text);
            kiln_value next = kiln_c99_concat(upvalues[0], text);
            kiln_c99_gc_keep_alive(&upvalues[0]);
            kiln_c99_gc_keep_alive(&text);
            kiln_c99_gc_keep_alive(&next);
            if (next.tag != KILN_VALUE_STRING) return KILN_C99_OUT_OF_MEMORY;
            upvalues[0] = next;
            if (i + 1 < argc - 1) {
                kiln_c99_gc_keep_alive(&upvalues[0]);
                next = kiln_c99_concat(upvalues[0], kiln_c99_static_text(",", 1));
                kiln_c99_gc_keep_alive(&upvalues[0]);
                kiln_c99_gc_keep_alive(&next);
                if (next.tag != KILN_VALUE_STRING) return KILN_C99_OUT_OF_MEMORY;
                upvalues[0] = next;
            }
        }
        kiln_value primitive;
        int status = kiln_c99_to_primitive(host, args[argc - 1], 1 /* string */,
                                           &primitive, depth, thrown);
        if (status != KILN_C99_OK) return status;
        kiln_c99_gc_keep_alive(&upvalues[0]);
        kiln_c99_gc_keep_alive(&args[argc - 1]);
        if (primitive.tag == KILN_VALUE_SYMBOL)
            return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                        "Cannot convert a Symbol value to a string");
        upvalues[1] = kiln_c99_text(primitive);
        kiln_c99_gc_keep_alive(&upvalues[0]);
        kiln_c99_gc_keep_alive(&primitive);
    }
    if (upvalues[0].tag != KILN_VALUE_STRING || upvalues[1].tag != KILN_VALUE_STRING)
        return KILN_C99_OUT_OF_MEMORY;
    kiln_c99_gc_keep_alive(&upvalues[0]);
    kiln_c99_gc_keep_alive(&upvalues[1]);

    kiln_val val_upvalues[3] = {
        kiln_val_from_boundary(upvalues[0]),
        kiln_val_from_boundary(upvalues[1]),
        kiln_val_from_boundary(upvalues[2])
    };

    uint8_t *body_source = NULL;
    size_t body_source_len = 0;
    if (kiln_dyn_string_to_source(upvalues[1], &body_source,
                                  &body_source_len) != KILN_C99_OK)
        return KILN_C99_OUT_OF_MEMORY;
    int body_strict = kiln_dyn_body_has_use_strict((const char *)body_source,
                                                   body_source_len);
    int validation_status = kiln_dyn_validate_source((const char *)body_source,
                                                     body_source_len, body_strict, thrown);
    free(body_source);
    if (validation_status != KILN_C99_OK) return validation_status;

    kiln_value reflected_source;
    int source_status = kiln_dyn_function_constructor_source(upvalues[0], upvalues[1],
                                                             &reflected_source);
    if (source_status != KILN_C99_OK) return source_status;
    kiln_c99_gc_keep_alive(&upvalues[0]);
    kiln_c99_gc_keep_alive(&upvalues[1]);
    kiln_c99_gc_keep_alive(&reflected_source);

    int status = kiln_c99_make_closure(result, kiln_c99_dynamic_body_trampoline,
                                       NULL, "", 0,
                                       "eval", 0, (uint32_t)(argc > 1 ? argc - 1 : 0),
                                       0, 0, 0, 0, 0, body_strict, KILN_VAL_UNDEFINED,
                                       KILN_VAL_UNDEFINED, 0, 16, 0, 0,
                                       val_upvalues, 3);
    kiln_c99_gc_keep_alive(val_upvalues);
    kiln_c99_gc_keep_alive(&upvalues[0]);
    kiln_c99_gc_keep_alive(&upvalues[1]);
    kiln_c99_gc_keep_alive(&reflected_source);
    if (status == KILN_C99_OK) {
        kiln_c99_heap_store_value(kiln_c99_closure_of(*result),
                                  &kiln_c99_closure_of(*result)->source_value,
                                  reflected_source);
    }
    return status;
}
/* Dynamic import option validation is shared by every generated importer. Keeping it in the
 * runtime means the launcher and shared wrapper cannot drift in their object/enumeration rules. */
static int kiln_c99_validate_dynamic_import_options(kiln_host *host, kiln_value options, int depth,
                                                    kiln_value *thrown, int *attribute_type) {
    kiln_value attrs;
    if (attribute_type != NULL) *attribute_type = 0;
    kiln_value keys;
    kiln_value entries;
    size_t index;
    int status;
    if (options.tag == KILN_VALUE_UNDEFINED) return KILN_C99_OK;
    if (options.tag == KILN_VALUE_NULL || options.tag == KILN_VALUE_BOOLEAN ||
        options.tag == KILN_VALUE_NUMBER || options.tag == KILN_VALUE_STRING ||
        options.tag == KILN_VALUE_SYMBOL || options.tag == KILN_VALUE_BIGINT) {
        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                    "import options must be an object");
    }
    status = kiln_c99_get(host, options, "with", 4, &attrs, depth, thrown);
    if (status != KILN_C99_OK) return status;
    if (attrs.tag == KILN_VALUE_UNDEFINED) return KILN_C99_OK;
    if (attrs.tag == KILN_VALUE_NULL || attrs.tag == KILN_VALUE_BOOLEAN ||
        attrs.tag == KILN_VALUE_NUMBER || attrs.tag == KILN_VALUE_STRING ||
        attrs.tag == KILN_VALUE_SYMBOL || attrs.tag == KILN_VALUE_BIGINT) {
        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                    "import attributes must be an object");
    }
    status = kiln_c99_own_keys(host, attrs, &keys, depth, thrown);
    if (status != KILN_C99_OK) return status;
    status = kiln_c99_new_array(&entries);
    if (status != KILN_C99_OK) return status;
    for (index = 0; index < kiln_c99_array_of(keys)->count; ++index) {
        kiln_value key = kiln_val_to_boundary(kiln_c99_array_of(keys)->elements[index]);
        kiln_value value;
        if (key.tag != KILN_VALUE_STRING) continue;
        size_t key_len = 0;
        const char *key_bytes = kiln_c99_string_ascii_bytes(key, &key_len);
        if (key_bytes == NULL) {
            return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                        "unsupported import attribute");
        }
        status = kiln_c99_get(host, attrs, key_bytes, key_len, &value, depth, thrown);
        if (status != KILN_C99_OK) return status;
        kiln_value entry;
        status = kiln_c99_new_array(&entry);
        if (status == KILN_C99_OK) status = kiln_c99_array_push(entry, key);
        if (status == KILN_C99_OK) status = kiln_c99_array_push(entry, value);
        if (status == KILN_C99_OK) status = kiln_c99_array_push(entries, entry);
        if (status != KILN_C99_OK) return status;
    }
    /* Validate only after all enumerable getters have run. */
    for (index = 0; index < kiln_c99_array_of(entries)->count; ++index) {
        kiln_value entry = kiln_val_to_boundary(kiln_c99_array_of(entries)->elements[index]);
        kiln_value key = kiln_val_to_boundary(kiln_c99_array_of(entry)->elements[0]);
        kiln_value value = kiln_val_to_boundary(kiln_c99_array_of(entry)->elements[1]);
        size_t key_len = 0;
        const char *key_bytes = kiln_c99_string_ascii_bytes(key, &key_len);
        if (key_bytes == NULL || key_len != 4 || memcmp(key_bytes, "type", 4) != 0) continue;
        if (value.tag != KILN_VALUE_STRING) {
            return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                        "import attribute values must be strings");
        }
        size_t value_len = 0;
        const char *value_bytes = kiln_c99_string_ascii_bytes(value, &value_len);
        if (value_bytes == NULL || value_len != 4 || memcmp(value_bytes, "file", 4) != 0) {
            return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                        "unsupported import attribute `type`");
        }
        if (attribute_type != NULL) *attribute_type = 1;
    }
    /* Unknown keys are rejected only after all enumerable values have been read. */
    for (index = 0; index < kiln_c99_array_of(entries)->count; ++index) {
        kiln_value entry = kiln_val_to_boundary(kiln_c99_array_of(entries)->elements[index]);
        kiln_value key = kiln_val_to_boundary(kiln_c99_array_of(entry)->elements[0]);
        size_t key_len = 0;
        const char *key_bytes = kiln_c99_string_ascii_bytes(key, &key_len);
        if (key.tag == KILN_VALUE_STRING &&
            (key_bytes == NULL || key_len != 4 || memcmp(key_bytes, "type", 4) != 0)) {
            return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                        "unsupported import attribute");
        }
    }
    return KILN_C99_OK;
}
