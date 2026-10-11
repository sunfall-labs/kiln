
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

/* Fixed C method tables are an intrinsic-name adapter only. Keep the canonical headed key for
 * storage and identity; borrow Latin1 payloads, or copy an all-ASCII wide key into a bounded
 * scratch buffer, and refuse no generic lookup merely because the name is non-ASCII. */
static int kiln_c99_key_ascii_view(const kiln_string *key, char scratch[256], const char **bytes,
                                   size_t *length) {
    size_t index;
    if (key == NULL || bytes == NULL || length == NULL) return 0;
    if (!kiln_string_is_utf16(key)) {
        for (index = 0; index < key->unit_len; ++index) {
            uint32_t unit = kiln_string_unit_at(key, index);
            if (unit > 0x7f) return 0;
        }
        *bytes = (const char *)kiln_string_latin1(key);
        *length = key->unit_len;
        return 1;
    }
    if (key->unit_len >= 256) return 0;
    for (index = 0; index < key->unit_len; ++index) {
        uint32_t unit = kiln_string_unit_at(key, index);
        if (unit > 0x7f) return 0;
        scratch[index] = (char)unit;
    }
    *bytes = scratch;
    *length = key->unit_len;
    return 1;
}

static void kiln_c99_typed_own_key(kiln_value target, const kiln_string *key, int *found,
                                   int *enumerable) {
    double index = kiln_c99_key_index_key(key);
    kiln_typed_array *view = kiln_c99_typed_array_of(target);
    *found = index >= 0.0 && (size_t)index < view->length;
    *enumerable = *found;
}

static int kiln_c99_typed_prototype_own_key(kiln_value target, const kiln_string *key,
                                            const kiln_string *canonical, int *found,
                                            int *enumerable) {
    kiln_typed_kind typed_kind;
    int concrete = target.tag == KILN_VALUE_BUILTIN &&
                   kiln_c99_typed_prototype_kind(target.bits, &typed_kind);
    int common = target.tag == KILN_VALUE_BUILTIN &&
                 target.bits == KILN_C99_BUILTIN_TYPED_ARRAY_PROTOTYPE;
    (void)typed_kind;
    *found = 0;
    *enumerable = 0;
    if (!concrete && !common) return 0;
    if (concrete) {
        *found = kiln_c99_same_name_key(key, "constructor") ||
                 kiln_c99_same_name_key(key, "BYTES_PER_ELEMENT");
    }
    if (!*found) {
        kiln_value store;
        if (kiln_c99_callable_store(target, &store, 0) == KILN_C99_OK &&
            store.tag == KILN_VALUE_OBJECT) {
            kiln_property property;
            if (kiln_c99_property_read_own_value(store, key, canonical, &property)) {
                *found = 1;
                *enumerable = property.enumerable;
            }
        }
    }
    return 1;
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

/* `Array.prototype.map`/`reduce` are shared TypeScript now. Their sparse-array loop asks for a
 * candidate only on a concrete array whose indexed prototype chain is unchanged. In that case
 * HasProperty for a hole is false and the dense backing can skip a run of holes without one VM
 * instruction per index. Any array side store, index descriptor, indexed prototype mutation, or
 * custom prototype chain asks TypeScript to use ordinary `in` instead. */
static int kiln_c99_store_has_indexed_property(kiln_value store, int *answer) {
    kiln_object *object;
    size_t index;
    *answer = 0;
    if (store.tag == KILN_VALUE_UNDEFINED) return KILN_C99_OK;
    if (store.tag != KILN_VALUE_OBJECT) return KILN_C99_INVALID_ARGUMENT;
    object = kiln_c99_object_of(store);
    if (object->prototype.tag != KILN_VALUE_UNDEFINED) {
        *answer = 1;
        return KILN_C99_OK;
    }
    /* An index can live in any of the object's stores: its elements (where `o[1] = v` puts
     * it), its shape's slots, a sealed layout or the dictionary list. A yes only sends the
     * caller to the ordinary `in`, so anything not cheaply ruled out answers yes. */
    if (object->element_count != 0 || object->sealed != NULL) {
        *answer = 1;
        return KILN_C99_OK;
    }
    if (object->shape != NULL) {
        for (index = 0; index < object->shape->property_count; ++index) {
            if (object->shape->keys[index] != NULL &&
                kiln_c99_key_index_key(object->shape->keys[index]) >= 0.0) {
                *answer = 1;
                return KILN_C99_OK;
            }
        }
    }
    for (index = 0; index < object->count; ++index) {
        if (object->properties[index].key != NULL &&
            kiln_c99_key_index_key(object->properties[index].key) >= 0.0) {
            *answer = 1;
            return KILN_C99_OK;
        }
    }
    return KILN_C99_OK;
}

int kiln_c99_array_next_present_index(kiln_value value, kiln_value cursor_value,
                                      kiln_value limit_value, kiln_value reverse_value,
                                      kiln_value *result) {
    kiln_array *array;
    double cursor_number;
    double limit_number;
    int reverse;
    int has_index = 0;
    size_t index;
    size_t stop;
    kiln_value prototype_store;
    int status;
    if (result == NULL) return KILN_C99_INVALID_ARGUMENT;
    *result = kiln_c99_boolean(0);
    if (value.tag != KILN_VALUE_ARRAY || cursor_value.tag != KILN_VALUE_NUMBER ||
        limit_value.tag != KILN_VALUE_NUMBER || reverse_value.tag != KILN_VALUE_BOOLEAN) {
        return KILN_C99_OK;
    }
    cursor_number = kiln_c99_as_number(cursor_value);
    limit_number = kiln_c99_as_number(limit_value);
    reverse = kiln_c99_truthy(reverse_value);
    if (!isfinite(cursor_number) || cursor_number < 0.0 || floor(cursor_number) != cursor_number ||
        !isfinite(limit_number) || floor(limit_number) != limit_number) {
        return KILN_C99_OK;
    }
    array = kiln_c99_array_of(value);
    if (!reverse && cursor_number < (double)array->count &&
        array->elements[(size_t)cursor_number] != KILN_VAL_HOLE) {
        *result = kiln_c99_number(cursor_number);
        return KILN_C99_OK;
    }
    if (reverse && cursor_number < (double)array->count) {
        size_t current = (size_t)cursor_number;
        if (array->elements[current] != KILN_VAL_HOLE) {
            *result = kiln_c99_number(cursor_number);
            return KILN_C99_OK;
        }
    }
    if (array->properties.tag != KILN_VALUE_UNDEFINED || array->has_index_descriptors) {
        return KILN_C99_OK;
    }
    /* These two tables make up every default indexed-prototype lookup for an array in this
     * representation. If a program added an index, or moved either prototype onto a custom
     * chain, the fast query declines and the ordinary `in` operator remains authoritative. */
    status = kiln_c99_callable_store(kiln_c99_builtin(KILN_C99_BUILTIN_ARRAY_PROTOTYPE),
                                     &prototype_store, 0);
    if (status != KILN_C99_OK) return KILN_C99_OK;
    status = kiln_c99_store_has_indexed_property(prototype_store, &has_index);
    if (status != KILN_C99_OK || has_index) return KILN_C99_OK;
    status = kiln_c99_callable_store(kiln_c99_builtin(KILN_C99_BUILTIN_OBJECT_PROTOTYPE),
                                     &prototype_store, 0);
    if (status != KILN_C99_OK) return KILN_C99_OK;
    status = kiln_c99_store_has_indexed_property(prototype_store, &has_index);
    if (status != KILN_C99_OK || has_index) return KILN_C99_OK;
    if (!reverse) {
        if (limit_number <= cursor_number) {
            *result = kiln_c99_null();
            return KILN_C99_OK;
        }
        if (cursor_number > (double)SIZE_MAX || limit_number > (double)SIZE_MAX) {
            *result = kiln_c99_null();
            return KILN_C99_OK;
        }
        index = (size_t)cursor_number;
        stop = (size_t)limit_number;
        if (stop > array->count) stop = array->count;
        while (index < stop) {
            if (array->elements[index] != KILN_VAL_HOLE) {
                *result = kiln_c99_number((double)index);
                return KILN_C99_OK;
            }
            ++index;
        }
        *result = kiln_c99_null();
        return KILN_C99_OK;
    }
    if (limit_number != -1.0) return KILN_C99_OK;
    if (array->count == 0) {
        *result = kiln_c99_null();
        return KILN_C99_OK;
    }
    index = cursor_number >= (double)array->count ? array->count - 1 : (size_t)cursor_number;
    for (;;) {
        if (array->elements[index] != KILN_VAL_HOLE) {
            *result = kiln_c99_number((double)index);
            return KILN_C99_OK;
        }
        if (index == 0) break;
        --index;
    }
    *result = kiln_c99_null();
    return KILN_C99_OK;
}

static kiln_value kiln_c99_method(uint64_t code) {
    return kiln_c99_value(KILN_VALUE_METHOD, code, NULL, 0);
}

/* Internal context ownership contract: every language value reachable through the call
 * context belongs to this anchor's complete registered root inventory. The launcher supplies
 * the same visitor it registers for collection. Context and anchor outlive the callable;
 * this constructor neither registers roots nor creates a second module-root field list. */
int kiln_c99_commonjs_require_value_with_resolver(kiln_value *out,
    kiln_commonjs_require_fn call, kiln_commonjs_resolve_fn resolve, void *context,
    kiln_gc_visit_roots_fn visit_context, void *root_context) {
    kiln_commonjs_require *require;
    if (out == NULL || call == NULL || (visit_context != NULL && root_context == NULL))
        return KILN_C99_INVALID_ARGUMENT;
    /* Context storage is owned by the launcher and traced by its registered root inventory.
     * These fields are initialized before publication; no GC heap edge is hidden from transfer. */
    require = (kiln_commonjs_require *)kiln_c99_gc_alloc(KILN_GC_COMMONJS_REQUIRE,
                                                         sizeof *require);
    if (require == NULL) return KILN_C99_OUT_OF_MEMORY;
    require->call = call;
    require->resolve = resolve;
    kiln_c99_heap_store_value(require, &require->properties, kiln_c99_undefined());
    kiln_c99_heap_store_pointer(require, &require->context, context);
    require->visit_context = visit_context;
    kiln_c99_heap_store_pointer(require, &require->root_context, root_context);
    if (kiln_c99_candidate_context_publish(kiln_c99_gc_heap(), require) != KILN_C99_OK)
        return KILN_C99_OUT_OF_MEMORY;
    kiln_c99_candidate_record_owner_layout(kiln_c99_gc_heap(), ((kiln_gc_block *)require) - 1);
    *out = kiln_c99_value(KILN_VALUE_COMMONJS_REQUIRE, (uint64_t)(uintptr_t)require, NULL, 0);
    if (resolve != NULL) {
        kiln_value *process_slot;
        kiln_value process = kiln_c99_undefined();
        kiln_value native_modules = kiln_c99_undefined();
        kiln_value decorator = kiln_c99_undefined();
        kiln_value decorated = kiln_c99_undefined();
        int status = kiln_c99_realm_value_slot(KILN_C99_REALM_SLOT_PROCESS, &process_slot);
        if (status == KILN_C99_OK) process = *process_slot;
        if (status == KILN_C99_OK && process.tag == KILN_VALUE_OBJECT) {
            status = kiln_c99_get(NULL, process, "__kilnNativeModules", 19,
                                  &native_modules, 0, NULL);
            if (status == KILN_C99_OK)
                status = kiln_c99_get(NULL, native_modules, "__kilnDecorateCommonJsRequire", 29,
                                      &decorator, 0, NULL);
            kiln_c99_gc_keep_alive(&process);
            kiln_c99_gc_keep_alive(&native_modules);
        } else if (status == KILN_C99_OK) {
            decorator = kiln_c99_undefined();
        }
        if (status != KILN_C99_OK) return status;
        int found = decorator.tag != KILN_VALUE_UNDEFINED;
        if (found) {
            status = kiln_c99_call(NULL, &decorated, decorator, kiln_c99_undefined(), out, 1, 0, NULL);
            kiln_c99_gc_keep_alive(out);
            kiln_c99_gc_keep_alive(&decorator);
            kiln_c99_gc_keep_alive(&decorated);
            if (status != KILN_C99_OK) return status;
            *out = decorated;
        }
    }
    return KILN_C99_OK;
}

static int kiln_c99_commonjs_require_value_with_roots(kiln_value *out,
    kiln_commonjs_require_fn call, void *context, kiln_gc_visit_roots_fn visit_context,
    void *root_context) {
    return kiln_c99_commonjs_require_value_with_resolver(out, call, NULL, context,
                                                         visit_context, root_context);
}

static int kiln_c99_commonjs_require_value(kiln_value *out, kiln_commonjs_require_fn call,
                                           void *context) {
    return kiln_c99_commonjs_require_value_with_roots(out, call, context, NULL, NULL);
}

/* The name a console shows, which is the JavaScript name rather than the implementation's --
 * `[Function: push]`, the same text the interpreter produces. */
static const char *kiln_c99_method_name(uint64_t code) {
    /* A member of the shared surface is named by its own key, which is the one place that name is
     * written down. Asked first so that adding a row is the whole of adding a member: the switch
     * below cannot be missing a case for something the table already names, which is the state
     * fifteen of these rows were in. */
    const char *surface = kiln_c99_surface_method_name(code);
    if (surface != NULL) {
        return surface;
    }
#if KILN_RETAIN_GROUP_TEMPORAL
    if (kiln_c99_method_in_group(code, KILN_C99_GROUP_TEMPORAL_OP)) {
        return kiln_c99_temporal_op_name(code);
    }
    if (kiln_c99_method_in_group(code, KILN_C99_GROUP_TEMPORAL_NOW_READ)) {
        return kiln_c99_temporal_now_name(code);
    }
#endif
    switch (code) {
    /* What `require("node:events")` hands back: the module's callable default, EventEmitter. */
    case KILN_C99_METHOD_NODE_EVENT_EMITTER: return "EventEmitter";
    case KILN_C99_METHOD_NODE_EVENT_LISTENER_COUNT: return "listenerCount";
    case KILN_C99_METHOD_NODE_HTTP_SERVER: return "Server";
    case KILN_C99_METHOD_NODE_REQUEST_HEADERS_GET: return "headers";
    case KILN_C99_METHOD_NODE_REQUEST_HEADERS_SET: return "headers";
    case KILN_C99_METHOD_NODE_HTTP_CREATE_SERVER: return "createServer";
    case KILN_C99_METHOD_NODE_SERVER_IS_LISTENING: return "__serverIsListening";
    case KILN_C99_METHOD_NODE_HTTP_CREATE_FETCH_SERVER: return "createFetchServer";
    case KILN_C99_METHOD_NODE_HTTP_FETCH_RESPOND: return "fetchRespond";
    case KILN_C99_METHOD_NODE_HTTP_CANCEL_REQUEST: return "__cancelRequest";
    case KILN_C99_METHOD_NODE_CLIENT_SOCKET_ASSIGN: return "__kilnNodeClientSocketAssign";
    case KILN_C99_METHOD_NODE_SOCKET_CLOSE_TICK: return "__kilnNodeSocketCloseTick";
    case KILN_C99_METHOD_PROCESS_NEXT_TICK: return "nextTick";
    case KILN_C99_METHOD_PROCESS_KILL: return "kill";
    case KILN_C99_METHOD_PROCESS_EXIT: return "exit";
    case KILN_C99_METHOD_PROCESS_EXIT_CODE_GET: return "processExitCodeGet";
    case KILN_C99_METHOD_PROCESS_EXIT_CODE_SET: return "processExitCodeSet";
    case KILN_C99_METHOD_GC_REQUEST: return "gcRequest";
    case KILN_C99_METHOD_NODE_FS_READ_FILE_SYNC: return "readFileSync";
    case KILN_C99_METHOD_NODE_FS_WRITE_FILE_SYNC: return "writeFileSync";
    case KILN_C99_METHOD_NODE_FS_MKDIR_SYNC: return "mkdirSync";
    case KILN_C99_METHOD_NODE_FS_STAT_SYNC: return "statSync";
    case KILN_C99_METHOD_NODE_FS_LSTAT_SYNC: return "lstatSync";
    case KILN_C99_METHOD_NODE_FS_READDIR_SYNC: return "readdirSync";
    case KILN_C99_METHOD_NODE_FS_REALPATH_SYNC: return "realpathSync";
    case KILN_C99_METHOD_NODE_FS_UNLINK_SYNC: return "unlinkSync";
    case KILN_C99_METHOD_NODE_FS_RMDIR_SYNC: return "rmdirSync";
    case KILN_C99_METHOD_NODE_STAT_IS_SYMBOLIC_LINK: return "isSymbolicLink";
    case KILN_C99_METHOD_PROCESS_START_RAW: return "__kilnHostProcessStart";
    case KILN_C99_METHOD_PROCESS_CANCEL_RAW: return "__kilnHostProcessCancel";
    case KILN_C99_METHOD_NODE_MODULE_CREATE_REQUIRE: return "__kilnHostModuleCreateRequire";
    case KILN_C99_METHOD_MODULE_LOADER_RAW: return "__kilnHostModuleLoader";
    case KILN_C99_METHOD_COMMONJS_REQUIRE_RESOLVE: return "__kilnHostModuleRequireResolve";
    case KILN_C99_METHOD_INTL_SEGMENT_RAW: return "__kilnIntlSegment";
    case KILN_C99_METHOD_STRING_LOCALE_CASE_RAW: return "__kilnHostStringLocaleCase";
    case KILN_C99_METHOD_INTL_DATETIME_FORMAT_RAW: return "__kilnIntlDateTimeFormat";
    case KILN_C99_METHOD_CRYPTO_KEY_IMPORT_RAW: return "__kilnCryptoKeyImport";
    case KILN_C99_METHOD_CRYPTO_KEY_PUBLIC_RAW: return "__kilnCryptoKeyPublic";
    case KILN_C99_METHOD_CRYPTO_KEY_INFO_RAW: return "__kilnCryptoKeyInfo";
    case KILN_C99_METHOD_CRYPTO_KEY_EXPORT_RAW: return "__kilnCryptoKeyExport";
    case KILN_C99_METHOD_CRYPTO_KEY_FREE_RAW: return "__kilnCryptoKeyFree";
    case KILN_C99_METHOD_UINT8_TO_HEX_RAW: return "__kilnUint8ToHex";
    case KILN_C99_METHOD_BUFFER_TO_STRING_RAW: return "__kilnBufferToString";
    case KILN_C99_METHOD_URL_PARSE_RAW: return "__kilnUrlParse";
    case KILN_C99_METHOD_ARRAY_BUFFER_DETACH_RAW: return "__kilnArrayBufferDetach";
    case KILN_C99_METHOD_ARRAY_BUFFER_BYTE_LENGTH_RAW: return "__kilnArrayBufferByteLength";
    case KILN_C99_METHOD_CLONE_DATA_RAW: return "__kilnCloneData";
    case KILN_C99_METHOD_SHARE_RAW: return "__kilnShare";
    case KILN_C99_METHOD_SHARED_IS_RAW: return "__kilnSharedIs";
    case KILN_C99_METHOD_SHARED_WEAK_RAW: return "__kilnSharedWeak";
    case KILN_C99_METHOD_COLLECTION_SIZE_RAW: return "__kilnCollectionSize";
    case KILN_C99_METHOD_JSON_UNBOX_RAW: return "__kilnJsonUnbox";
    case KILN_C99_METHOD_JSON_RAW_CREATE: return "__kilnJsonRawCreate";
    case KILN_C99_METHOD_JSON_IS_RAW: return "__kilnJsonIsRaw";
    case KILN_C99_METHOD_FUNCTION_KIND_PROTOTYPE: return "__kilnFunctionKindPrototype";
    case KILN_C99_METHOD_FUNCTION_KIND_CONSTRUCTOR: return "__kilnFunctionKindConstructor";
    case KILN_C99_METHOD_REGEXP_LEGACY_CAPTURE_RAW: return "__kilnRegExpLegacyCapture";
    case KILN_C99_METHOD_TYPED_ARRAY_COPY_RAW: return "__kilnTypedArrayCopy";
    case KILN_C99_METHOD_TYPED_ARRAY_SORT_RAW: return "__kilnTypedArraySort";
    case KILN_C99_METHOD_DATA_VIEW_STATE_RAW: return "__kilnDataViewState";
    case KILN_C99_METHOD_IS_CONSTRUCTOR_RAW: return "__kilnIsConstructor";
    case KILN_C99_METHOD_IS_OTHER_REALM_ARRAY_CONSTRUCTOR_RAW: return "__kilnIsOtherRealmArrayConstructor";
    case KILN_C99_METHOD_ARRAY_NEXT_PRESENT_INDEX_RAW: return "__kilnArrayNextPresentIndex";
    case KILN_C99_METHOD_WORKER_OP_RAW: return "__kilnWorkerOp";
    case KILN_C99_METHOD_PORT_WAIT_RAW: return "__kilnPortWait";
    case KILN_C99_METHOD_SHARED_BUFFER_RAW: return "__kilnSharedBuffer";
    case KILN_C99_METHOD_ATOMICS_RAW: return "__kilnAtomics";
    case KILN_C99_METHOD_FS_DESCRIPTOR_RAW: return "__kilnFsDescriptor";
    case KILN_C99_METHOD_JOBS_PENDING_RAW: return "__kilnJobsPending";
    case KILN_C99_METHOD_QUEUE_IO_RAW: return "__kilnQueueIo";
    case KILN_C99_METHOD_TIMER_REF_RAW: return "__kilnTimerRef";
    case KILN_C99_METHOD_PROMISE_REJECTIONS_RAW: return "__kilnPromiseRejections";
    case KILN_C99_METHOD_URL_SET_RAW: return "__kilnUrlSet";
    case KILN_C99_METHOD_URL_FAST_RAW: return "__kilnUrlFast";
    case KILN_C99_METHOD_URL_ADOPT_RAW: return "__kilnUrlAdopt";
    case KILN_C99_METHOD_WEBCRYPTO_PRIMITIVE_RAW: return "__kilnWebCryptoPrimitive";
    case KILN_C99_METHOD_COMPRESSION_PRIMITIVE_RAW: return "__kilnCompressionPrimitive";
    case KILN_C99_METHOD_UINT8_TO_BASE64_RAW: return "__kilnUint8ToBase64";
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
    case KILN_C99_METHOD_DATE_NOW: return "now";
    case KILN_C99_METHOD_DATE_UTC: return "UTC";
    case KILN_C99_METHOD_DATE_PARSE: return "parse";
    case KILN_C99_METHOD_HAS_OWN_PROPERTY: return "hasOwnProperty";
    case KILN_C99_METHOD_OBJECT_TO_STRING: return "toString";
    case KILN_C99_METHOD_OBJECT_TO_LOCALE_STRING: return "toLocaleString";
    case KILN_C99_METHOD_OBJECT_VALUE_OF: return "valueOf";
    case KILN_C99_METHOD_BOOLEAN_VALUE_OF: return "valueOf";
    case KILN_C99_METHOD_SYMBOL_VALUE_OF: return "valueOf";
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
    case KILN_C99_METHOD_REFLECT_DEFINE_PROPERTY: return "defineProperty";
    case KILN_C99_METHOD_REFLECT_GET_PROTOTYPE_OF: return "getPrototypeOf";
    case KILN_C99_METHOD_GENERATOR_THROW: return "throw";
    case KILN_C99_METHOD_PROXY_REVOCABLE: return "revocable";
    case KILN_C99_METHOD_PROXY_REVOKE: return "revoke";
    case KILN_C99_METHOD_OBJECT_FROM_ENTRIES: return "fromEntries";
    case KILN_C99_METHOD_ARRAY_FROM: return "from";
    case KILN_C99_METHOD_SET_TIME: return "setTime";
    case KILN_C99_METHOD_STRING_RAW: return "raw";
    case KILN_C99_METHOD_STRING_FROM_CHAR_CODE: return "fromCharCode";
    case KILN_C99_METHOD_STRING_FROM_CODE_POINT: return "fromCodePoint";
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
    case KILN_C99_METHOD_NODE_READABLE_SET_RAW_MODE: return "setRawMode";
    case KILN_C99_METHOD_NODE_FS_MKDIR: return "mkdir";
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
