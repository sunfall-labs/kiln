
/* JavaScript's constructor return override is any object, including a function. Keep this
 * predicate separate from prototype eligibility: `null` is a legal prototype but a primitive
 * constructor return, while Temporal and host callables are objects even though they are not all
 * represented by ordinary object storage. */
static int kiln_c99_is_object_value(kiln_value value) {
    switch (value.tag) {
    case KILN_VALUE_OBJECT:
    case KILN_VALUE_FUNCTION:
    case KILN_VALUE_BUILTIN:
    case KILN_VALUE_ARRAY:
    case KILN_VALUE_METHOD:
    case KILN_VALUE_GENERATOR:
    case KILN_VALUE_DATE:
    case KILN_VALUE_COLLECTION:
    case KILN_VALUE_PROMISE:
    case KILN_VALUE_SETTLER:
    case KILN_VALUE_BOUND:
    case KILN_VALUE_REGEXP:
    case KILN_VALUE_ARRAY_BUFFER:
    case KILN_VALUE_TYPED_ARRAY:
    case KILN_VALUE_PROXY:
    case KILN_VALUE_TEMPORAL:
    case KILN_VALUE_COMMONJS_REQUIRE:
    case KILN_VALUE_DATA_VIEW:
        return 1;
    default:
        return 0;
    }
}

static int kiln_c99_get_prototype_from_constructor(kiln_host *host, kiln_value constructor,
                                                    kiln_value default_prototype,
                                                    kiln_value *out, int depth,
                                                    kiln_value *thrown) {
    kiln_value prototype;
    int status = kiln_c99_get(host, constructor, "prototype", 9, &prototype, depth, thrown);
    if (status != KILN_C99_OK) return status;
    *out = kiln_c99_is_object_value(prototype)
               ? prototype
               : default_prototype;
    return KILN_C99_OK;
}

static int kiln_c99_construct_with_new_target(kiln_host *host, kiln_value *result,
                                               kiln_value callee, const kiln_value *args,
                                               size_t argc, kiln_value new_target, int depth,
                                               kiln_value *thrown);
static int kiln_c99_is_constructor(kiln_value value);

static int kiln_c99_construct(kiln_host *host, kiln_value *result, kiln_value callee,
                              const kiln_value *args, size_t argc, int depth,
                              kiln_value *thrown) {
    return kiln_c99_construct_with_new_target(host, result, callee, args, argc, callee, depth,
                                              thrown);
}

static int kiln_c99_construct_with_new_target(kiln_host *host, kiln_value *result,
                                               kiln_value callee, const kiln_value *args,
                                               size_t argc, kiln_value new_target, int depth,
                                               kiln_value *thrown) {
    kiln_value prototype;
    kiln_value instance;
    kiln_value produced;
    kiln_value saved_new_target;
    kiln_value saved_adopted;
    kiln_value adopted;
    int status;
    if (callee.tag == KILN_VALUE_BUILTIN && callee.bits == KILN_C99_BUILTIN_TYPED_ARRAY) {
        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                    "Abstract class TypedArray not directly constructable");
    }
    if ((new_target.tag != callee.tag || new_target.bits != callee.bits) &&
        callee.tag != KILN_VALUE_FUNCTION && callee.tag != KILN_VALUE_PROXY &&
        callee.tag != KILN_VALUE_BOUND) {
        kiln_value target_prototype;
        if (callee.tag == KILN_VALUE_BUILTIN &&
            callee.bits == KILN_C99_BUILTIN_OBJECT) {
            status = kiln_c99_get_prototype_from_constructor(
                host, new_target, kiln_c99_builtin(KILN_C99_BUILTIN_OBJECT_PROTOTYPE),
                &target_prototype, depth, thrown);
            if (status != KILN_C99_OK) return status;
            status = kiln_c99_new_object(result);
            if (status != KILN_C99_OK) return status;
            return kiln_c99_set_prototype(*result, target_prototype);
        }
        status = kiln_c99_construct(host, result, callee, args, argc, depth, thrown);
        if (status != KILN_C99_OK) return status;
        kiln_value default_prototype;
        status = kiln_c99_get_prototype_from_constructor(
            host, callee, kiln_c99_builtin(KILN_C99_BUILTIN_OBJECT_PROTOTYPE),
            &default_prototype, depth, thrown);
        if (status != KILN_C99_OK) return status;
        status = kiln_c99_get_prototype_from_constructor(host, new_target, default_prototype,
                                                          &target_prototype, depth, thrown);
        if (status != KILN_C99_OK) return status;
        if (kiln_c99_is_object_value(*result)) {
            return kiln_c99_set_prototype(*result, target_prototype);
        }
        return KILN_C99_OK;
    }
    if (callee.tag == KILN_VALUE_BUILTIN && callee.bits == KILN_C99_BUILTIN_FUNCTION) {
        return kiln_c99_dynamic_new_function(host, result, args, argc, depth, thrown);
    }
    /* The request fast path (plan step B.3), ahead of construction: inside a `node:http`
     * handler the first `new URL(req.url, ...)` is answered with the object the runtime built
     * for the response, and not even the instance the constructor would have thrown away is
     * allocated. The class itself makes the same call, so a URL of anything else is its
     * business; this is only the case where the answer already exists. */
    if (callee.tag == KILN_VALUE_FUNCTION && kiln_c99_active_http_response != NULL &&
        !kiln_c99_active_http_response->url_used) {
        kiln_value *url_class;
        if (kiln_c99_realm_value_slot(KILN_C99_REALM_SLOT_URL_CLASS, &url_class) == KILN_C99_OK &&
            url_class->tag == KILN_VALUE_FUNCTION &&
            kiln_c99_closure_of(*url_class) == kiln_c99_closure_of(callee)) {
            kiln_value fast;
            int status = kiln_c99_url_fast_raw(&fast, args, argc, thrown);
            if (status != KILN_C99_OK) return status;
            if (fast.tag == KILN_VALUE_OBJECT) {
                *result = fast;
                return KILN_C99_OK;
            }
        }
    }
    /* `new 5` is a TypeError the program can catch, not a fault that stops it. Reaching the
     * prototype lookup below with a non-callable answered "a method this backend does not have"
     * instead, which exits the process: ordinary code like `try { new Thing() } catch {}` --
     * where `Thing` came from a table and turned out not to be a constructor -- could not be
     * written. Every callable is treated as constructible, which is the same latitude the
     * interpreter takes. */
    if (callee.tag == KILN_VALUE_BUILTIN && callee.bits == KILN_C99_BUILTIN_PROXY) {
        /* `new Proxy(target, handler)` is the constructor; a proxy *of* something constructible
         * is the `construct` trap below. */
        return kiln_c99_new_proxy(result, argc > 0 ? args[0] : kiln_c99_undefined(),
                                  argc > 1 ? args[1] : kiln_c99_undefined());
    }
#if KILN_RETAIN_GROUP_TEMPORAL
    if (callee.tag == KILN_VALUE_BUILTIN &&
        kiln_c99_temporal_kind_of_builtin(callee.bits) >= 0) {
        return kiln_c99_temporal_construct(host, result,
                                           kiln_c99_temporal_kind_of_builtin(callee.bits), args,
                                           argc, depth, thrown);
    }
#endif
    /* Demand-driven prelude: the Buffer constructor fast path is compiled out
     * when the executable retains no code that can name the Buffer global
     * (see `retained_method_groups`). Unreachable then, like a dropped
     * dispatch block. */
#if KILN_RETAIN_GROUP_BUFFER
    if (callee.tag == KILN_VALUE_BUILTIN && callee.bits == KILN_C99_BUILTIN_BUFFER) {
        return kiln_c99_buffer_from(result, argc > 0 ? args[0] : kiln_c99_number(0.0), args,
                                    argc, thrown);
    }
#endif
    if (callee.tag == KILN_VALUE_PROXY) {
        kiln_value trap;
        kiln_value inner = kiln_c99_proxy_of(callee)->target;
        kiln_value handler = kiln_c99_proxy_of(callee)->handler;
        status = kiln_c99_proxy_trap(host, callee, "construct", &trap, depth, thrown);
        if (status != KILN_C99_OK) {
            return status;
        }
        if (trap.tag == KILN_VALUE_UNDEFINED) {
            return kiln_c99_construct_with_new_target(host, result, inner, args, argc,
                                                       new_target, depth, thrown);
        }
        {
            kiln_value list;
            kiln_value arguments[3];
            size_t at;
            status = kiln_c99_new_array(&list);
            if (status != KILN_C99_OK) {
                return status;
            }
            for (at = 0; at < argc; ++at) {
                status = kiln_c99_array_push(list, args[at]);
                if (status != KILN_C99_OK) {
                    return status;
                }
            }
            arguments[0] = inner;
            arguments[1] = list;
            arguments[2] = new_target;
            return kiln_c99_call(host, result, trap, handler, arguments, 3, depth, thrown);
        }
    }
    if (!kiln_c99_is_callable(callee)) {
        return kiln_c99_throw_not_a(thrown, callee, "constructor");
    }
    if (callee.tag == KILN_VALUE_METHOD &&
        (callee.bits == KILN_C99_METHOD_NODE_EVENT_EMITTER ||
         callee.bits == KILN_C99_METHOD_NODE_HTTP_CREATE_SERVER ||
         callee.bits == KILN_C99_METHOD_NODE_STREAM ||
         callee.bits == KILN_C99_METHOD_NODE_READABLE ||
         callee.bits == KILN_C99_METHOD_NODE_WRITABLE ||
         callee.bits == KILN_C99_METHOD_NODE_HTTP_SERVER_RESPONSE ||
         callee.bits == KILN_C99_METHOD_NODE_HTTP_SERVER)) {
        /* Runtime-provided constructors are methods so their identity can cross generated
         * modules without exporting a backend-private closure. EventEmitter is constructible,
         * and its ordinary-call and `new` forms deliberately share one initializer.
         *
         * Demand-driven prelude: each branch below is compiled out when its
         * group is not retained (see `retained_method_groups`). A dropped
         * branch is unreachable: its method values only materialize through
         * the corresponding node builtin. */
#if KILN_RETAIN_GROUP_NODE_HTTP
        if (callee.bits == KILN_C99_METHOD_NODE_HTTP_CREATE_SERVER ||
            callee.bits == KILN_C99_METHOD_NODE_HTTP_SERVER) {
            if (argc > 0 && args[0].tag != KILN_VALUE_UNDEFINED &&
                args[0].tag != KILN_VALUE_NULL && !kiln_c99_is_callable(args[0]) &&
                args[0].tag != KILN_VALUE_OBJECT) {
                return kiln_c99_throw_node_invalid_arg_type(
                    thrown, "The \"options\" argument must be of type object");
            }
            return kiln_c99_node_http(host, result, callee.bits, kiln_c99_undefined(), args, argc,
                                      depth, thrown);
        }
#endif
#if KILN_RETAIN_GROUP_NODE_EVENT
        if (callee.bits == KILN_C99_METHOD_NODE_EVENT_EMITTER) {
            kiln_value prototype;
            status = kiln_c99_node_event(host, result, callee.bits, kiln_c99_undefined(), args,
                                         argc, depth, thrown);
            if (status == KILN_C99_OK)
                status = kiln_c99_callable_prototype(callee, &prototype);
            if (status == KILN_C99_OK)
                status = kiln_c99_set_prototype(*result, prototype);
            return status;
        }
#endif
#if KILN_RETAIN_GROUP_NODE_STREAM
        if (callee.bits == KILN_C99_METHOD_NODE_HTTP_SERVER_RESPONSE) {
            status = kiln_c99_node_stream(host, result, KILN_C99_METHOD_NODE_WRITABLE,
                                          kiln_c99_undefined(), args, argc, depth, thrown);
            if (status == KILN_C99_OK) {
                kiln_value prototype;
                status = kiln_c99_callable_prototype(callee, &prototype);
                if (status == KILN_C99_OK) status = kiln_c99_set_prototype(*result, prototype);
            }
            return status;
        }
        status = kiln_c99_node_stream(host, result, callee.bits, kiln_c99_undefined(), args, argc,
                                      depth, thrown);
        if (status == KILN_C99_OK &&
            (callee.bits == KILN_C99_METHOD_NODE_READABLE ||
             callee.bits == KILN_C99_METHOD_NODE_WRITABLE)) {
            kiln_value prototype;
            status = kiln_c99_callable_prototype(callee, &prototype);
            if (status == KILN_C99_OK) status = kiln_c99_set_prototype(*result, prototype);
        }
        return status;
#endif
    }
    if (callee.tag == KILN_VALUE_BUILTIN && (callee.bits == KILN_C99_BUILTIN_STRING ||
                                             callee.bits == KILN_C99_BUILTIN_NUMBER ||
                                             callee.bits == KILN_C99_BUILTIN_BOOLEAN)) {
        kiln_value primitive;
        /* String(symbol) is permitted, but `new String(symbol)` cannot erase the Symbol's type
         * and still be a String wrapper. Number and Boolean use their call conversion unchanged. */
        if (callee.bits == KILN_C99_BUILTIN_STRING && argc > 0) {
            /* The call form String(symbol) is exceptional and returns a descriptive string;
             * construction uses ordinary ToString after ToPrimitive, so both a primitive Symbol
             * and a boxed Symbol must throw. Reusing the call result lost that distinction. */
            int status = kiln_c99_to_primitive(host, args[0], 1, &primitive, depth, thrown);
            if (status != KILN_C99_OK) return status;
            if (primitive.tag == KILN_VALUE_SYMBOL) {
                return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                            "Cannot convert a Symbol value to a string");
            }
            primitive = kiln_c99_text(primitive);
            return kiln_c99_new_primitive_wrapper(result, primitive);
        }
        status = kiln_c99_call(host, &primitive, callee, kiln_c99_undefined(), args, argc,
                               depth, thrown);
        if (status != KILN_C99_OK) return status;
        return kiln_c99_new_primitive_wrapper(result, primitive);
    }
    if (callee.tag == KILN_VALUE_BUILTIN && callee.bits == KILN_C99_BUILTIN_REGEXP) {
        /* `new RegExp(source, flags)` builds exactly what `RegExp(source, flags)` builds -- the
         * language says so -- so it goes through the call path rather than repeating it. */
        return kiln_c99_call(host, result, callee, kiln_c99_undefined(), args, argc, depth,
                             thrown);
    }
    if (callee.tag == KILN_VALUE_BUILTIN &&
        callee.bits == KILN_C99_BUILTIN_AGGREGATE_ERROR) {
        return kiln_c99_aggregate_error_from_args(host, result, args, argc, depth, thrown);
    }
    if (callee.tag == KILN_VALUE_BUILTIN && callee.bits == KILN_C99_BUILTIN_PROMISE) {
        kiln_value promise;
        kiln_value settlers[2];
        kiln_value ignored;
        /* The executor is checked before anything is built, which is the order the specification
         * gives and the order that is observable: `new Promise()` throws a TypeError the program
         * can catch, and must not hand back a promise that can never settle. This backend was
         * building one and calling `undefined` as the executor, so the throw never happened and
         * the caller got a forever-pending promise -- a wrong answer, and a quiet one. */
        if (argc == 0 || !kiln_c99_is_callable(args[0])) {
            return kiln_c99_throw_not_a(thrown, argc > 0 ? args[0] : kiln_c99_undefined(),
                                        "function");
        }
        status = kiln_c99_new_promise(&promise);
        if (status == KILN_C99_OK) {
            status = kiln_c99_new_settler(&settlers[0], promise, 0);
        }
        if (status == KILN_C99_OK) {
            status = kiln_c99_new_settler(&settlers[1], promise, 1);
        }
        if (status != KILN_C99_OK) {
            return status;
        }
        /* The executor runs *now*, synchronously, which is what lets it capture something and
         * settle the promise later. One that throws rejects rather than failing the program:
         * `new Promise(() => { throw e })` is how a rejection is often written. */
        status = kiln_c99_call(host, &ignored, argc > 0 ? args[0] : kiln_c99_undefined(),
                               kiln_c99_undefined(), settlers, 2, depth, thrown);
        if (status == KILN_C99_THROWN) {
            status = kiln_c99_settle(promise, *thrown, 1);
            if (status != KILN_C99_OK) {
                return status;
            }
        } else if (status != KILN_C99_OK) {
            return status;
        }
        *result = promise;
        return KILN_C99_OK;
    }
    if (callee.tag == KILN_VALUE_BUILTIN && kiln_c99_error_name(callee.bits) != NULL) {
        int present = 0;
        status = kiln_c99_new_error(result, callee.bits,
                                    argc > 0 ? args[0] : kiln_c99_undefined());
        if (status != KILN_C99_OK) {
            return status;
        }
        /* `new Error(message, { cause })` carries the cause *by identity*, which is the whole
         * reason it exists: a wrapper has to be able to hand back the error it wrapped. Dropping
         * the options argument left the wrapping looking entirely right -- message, name and
         * stack were all there -- with only `wrapped.cause === original` saying otherwise.
         *
         * Presence rather than truthiness decides, so `{ cause: undefined }` installs one and a
         * plain `new Error(m)` has no such property at all. Non-enumerable like the other three,
         * which is what keeps `JSON.stringify(error)` at `{}`. */
        if (argc > 1 && args[1].tag == KILN_VALUE_OBJECT) {
            status = kiln_c99_has_property(host, args[1], "cause", 5, &present, depth, thrown);
            if (status != KILN_C99_OK) {
                return status;
            }
        }
        if (present) {
            kiln_value cause;
            kiln_object *object;
            size_t index;
            status = kiln_c99_get(host, args[1], "cause", 5, &cause, depth, thrown);
            if (status == KILN_C99_OK) {
                status = kiln_c99_put(NULL, *result, "cause", 5, cause, 0, NULL);
            }
            if (status != KILN_C99_OK) {
                return status;
            }
            object = kiln_c99_object_of(*result);
            for (index = 0; index < object->count; ++index) {
                if (kiln_c99_same_key(&object->properties[index], "cause", 5)) {
                    object->properties[index].enumerable = 0;
                }
            }
        }
        return KILN_C99_OK;
    }
    if (callee.tag == KILN_VALUE_BUILTIN && callee.bits == KILN_C99_BUILTIN_ARRAY) {
        return kiln_c99_array_constructor(result, args, argc, thrown);
    }
    if (callee.tag == KILN_VALUE_BUILTIN && callee.bits == KILN_C99_BUILTIN_OBJECT) {
        return kiln_c99_object_constructor(result, args, argc);
    }
    /* `new SharedArrayBuffer(length, options)`: the shared bootstrap's constructor
     * (`arraybuffer.ts`), which makes every check and asks `__kilnSharedBuffer` for the store. */
    if (callee.tag == KILN_VALUE_BUILTIN && callee.bits == KILN_C99_BUILTIN_SHARED_ARRAY_BUFFER) {
        kiln_value *slot;
        status = kiln_c99_realm_value_slot(KILN_C99_REALM_SLOT_SHARED_ARRAY_BUFFER, &slot);
        if (status != KILN_C99_OK) return status;
        if (!kiln_c99_is_callable(*slot))
            KILN_C99_REFUSE("SharedArrayBuffer before the runtime bootstrap defined it");
        return kiln_c99_call(host, result, *slot, kiln_c99_undefined(), args, argc, depth, thrown);
    }
    if (callee.tag == KILN_VALUE_BUILTIN && callee.bits == KILN_C99_BUILTIN_ARRAY_BUFFER) {
        size_t length;
        status = kiln_c99_to_index(argc > 0 ? args[0] : kiln_c99_undefined(), &length, thrown);
        if (status != KILN_C99_OK) {
            return status;
        }
        return kiln_c99_new_typed_buffer(result, length);
    }
    if (callee.tag == KILN_VALUE_BUILTIN && callee.bits == KILN_C99_BUILTIN_DATA_VIEW) {
        kiln_value source = argc > 0 ? args[0] : kiln_c99_undefined();
        kiln_value measure;
        size_t byte_offset = 0;
        size_t byte_length = 0;
        int has_length = argc > 2 && args[2].tag != KILN_VALUE_UNDEFINED;
        /* The order is the specification's, and it is observable: the buffer is checked before
         * either index is coerced, so a program that hands this a `Uint8Array` hears that its
         * argument is not an ArrayBuffer rather than a RangeError about an offset it never
         * wrote. */
        if (source.tag != KILN_VALUE_ARRAY_BUFFER) {
            return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                        "Method called on an incompatible receiver");
        }
        /* `ToNumber` before `ToIndex`, and through the pass that can call back into the program:
         * `kiln_c99_to_index` reads its argument with the pure `kiln_c99_as_number`, which
         * answers NaN for every object and would therefore take `{valueOf: () => 4}` as offset
         * zero rather than as four. Left to right, because two arguments that both throw must
         * throw the leftmost first. */
        status = kiln_c99_to_number_argument(host, argc > 1 ? args[1] : kiln_c99_undefined(),
                                             &measure, depth, thrown);
        if (status != KILN_C99_OK) {
            return status;
        }
        status = kiln_c99_to_index(measure, &byte_offset, thrown);
        if (status != KILN_C99_OK) {
            return status;
        }
        if (has_length) {
            status = kiln_c99_to_number_argument(host, args[2], &measure, depth, thrown);
            if (status != KILN_C99_OK) {
                return status;
            }
            status = kiln_c99_to_index(measure, &byte_length, thrown);
            if (status != KILN_C99_OK) {
                return status;
            }
        }
        return kiln_c99_new_data_view(result, source, byte_offset, byte_length, has_length,
                                      thrown);
    }
    if (callee.tag == KILN_VALUE_BUILTIN &&
        kiln_c99_builtin_typed_kind(callee.bits, &(kiln_typed_kind){0})) {
        kiln_typed_kind kind;
        kiln_value source = argc > 0 ? args[0] : kiln_c99_undefined();
        size_t byte_offset = 0;
        size_t length = 0;
        int has_length = 0;
        if (!kiln_c99_builtin_typed_kind(callee.bits, &kind)) {
            KILN_C99_REFUSE("a typed-array constructor with no element kind");
        }
        /* Offset and length are meaningful only for a backing buffer. Extra arguments to
         * array-like and numeric-length constructors are ignored without coercion. */
        if (source.tag == KILN_VALUE_ARRAY_BUFFER) {
            status = kiln_c99_to_index(argc > 1 ? args[1] : kiln_c99_undefined(), &byte_offset,
                                       thrown);
            if (status != KILN_C99_OK) {
                return status;
            }
            has_length = argc > 2 && args[2].tag != KILN_VALUE_UNDEFINED;
            if (has_length) {
                status = kiln_c99_to_index(args[2], &length, thrown);
                if (status != KILN_C99_OK) {
                    return status;
                }
            }
        }
        return kiln_c99_new_typed_array(result, kind, source, byte_offset, length, has_length,
                                        thrown);
    }
    if (callee.tag == KILN_VALUE_BUILTIN &&
        (callee.bits == KILN_C99_BUILTIN_MAP || callee.bits == KILN_C99_BUILTIN_SET ||
         callee.bits == KILN_C99_BUILTIN_WEAK_MAP ||
         callee.bits == KILN_C99_BUILTIN_WEAK_SET)) {
        int is_map = callee.bits == KILN_C99_BUILTIN_MAP ||
                     callee.bits == KILN_C99_BUILTIN_WEAK_MAP;
        int is_weak = callee.bits == KILN_C99_BUILTIN_WEAK_MAP ||
                      callee.bits == KILN_C99_BUILTIN_WEAK_SET;
        kiln_value built;
        status = kiln_c99_new_collection(&built, is_map, is_weak);
        if (status != KILN_C99_OK) {
            return status;
        }
        if (argc > 0 && args[0].tag != KILN_VALUE_UNDEFINED && args[0].tag != KILN_VALUE_NULL) {
            /* The argument is anything iterable: a Set takes its values and a Map takes
             * `[key, value]` pairs, which is why `new Map(map)` copies one. */
            kiln_value gathered;
            size_t index;
            status = kiln_c99_new_array(&gathered);
            if (status != KILN_C99_OK) {
                return status;
            }
            status = kiln_c99_iterate(host, gathered, args[0], depth, thrown);
            if (status != KILN_C99_OK) {
                return status;
            }
            {
                kiln_array *given = kiln_c99_array_of(gathered);
                for (index = 0; index < given->count; ++index) {
                    kiln_value key = kiln_val_to_boundary(given->elements[index]);
                    kiln_value member = key;
                    if (is_map) {
                        kiln_array *pair;
                        if (key.tag != KILN_VALUE_ARRAY) {
                            KILN_C99_REFUSE("a Map built from an entry that is not an array");
                        }
                        pair = kiln_c99_array_of(key);
                        key = pair->count > 0 && pair->elements[0] != KILN_VAL_HOLE
                                  ? kiln_val_to_boundary(pair->elements[0])
                                  : kiln_c99_undefined();
                        member = pair->count > 1 && pair->elements[1] != KILN_VAL_HOLE
                                     ? kiln_val_to_boundary(pair->elements[1])
                                     : kiln_c99_undefined();
                    }
                    status = kiln_c99_collection_put(built, key, member);
                    if (status != KILN_C99_OK) {
                        return status;
                    }
                }
            }
        }
        *result = built;
        return KILN_C99_OK;
    }
    if (callee.tag == KILN_VALUE_BUILTIN && callee.bits == KILN_C99_BUILTIN_DATE) {
        /* `new Date()` is the current instant, which is virtual here and never advances; one
         * argument is either milliseconds or an instant written down; more than one is the field
         * form, where the month is zero-based and the day defaults to 1. */
        double time;
        if (argc == 0) {
            time = 0.0;
        } else if (argc == 1) {
            if (args[0].tag == KILN_VALUE_DATE) {
                time = kiln_c99_date_time(args[0]);
            } else if (args[0].tag == KILN_VALUE_STRING) {
                time = kiln_c99_parse_date(args[0]);
            } else {
                kiln_value primitive;
                status = kiln_c99_to_primitive(host, args[0], 2 /* default */,
                                               &primitive, depth, thrown);
                if (status != KILN_C99_OK) {
                    return status;
                }
                if (primitive.tag == KILN_VALUE_STRING) {
                    time = kiln_c99_parse_date(primitive);
                } else if (primitive.tag == KILN_VALUE_SYMBOL) {
                    return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                                "Cannot convert a Symbol value to a number");
                } else if (primitive.tag == KILN_VALUE_BIGINT) {
                    return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                                "Cannot convert a BigInt value to a number");
                } else {
                    time = kiln_c99_as_number(primitive);
                }
            }
        } else {
            double fields[7];
            size_t index;
            for (index = 0; index < argc; ++index) {
                if (args[index].tag == KILN_VALUE_SYMBOL) {
                    return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                                "Cannot convert a Symbol value to a number");
                }
                if (kiln_c99_is_reference(args[index])) {
                    kiln_value primitive;
                    status = kiln_c99_to_primitive(host, args[index], 0 /* number */,
                                                   &primitive, depth, thrown);
                    if (status != KILN_C99_OK) {
                        return status;
                    }
                    if (primitive.tag == KILN_VALUE_SYMBOL) {
                        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                                    "Cannot convert a Symbol value to a number");
                    }
                    if (index < 7) {
                        fields[index] = kiln_c99_as_number(primitive);
                    }
                } else if (index < 7) {
                    fields[index] = kiln_c99_as_number(args[index]);
                }
            }
            for (index = argc; index < 7; ++index) {
                fields[index] = index == 2 ? 1.0 : 0.0;
            }
            time = kiln_c99_instant_from_parts(fields);
        }
        return kiln_c99_new_date(result, time);
    }
    if (callee.tag == KILN_VALUE_BUILTIN &&
        callee.bits == KILN_C99_BUILTIN_ABORT_CONTROLLER) {
        return kiln_c99_new_abort_controller(result);
    }
    if (callee.tag == KILN_VALUE_BUILTIN && callee.bits == KILN_C99_BUILTIN_ABORT_SIGNAL) {
        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                    "Illegal constructor");
    }
    if (callee.tag == KILN_VALUE_BUILTIN && callee.bits == KILN_C99_BUILTIN_TEXT_ENCODER) {
        return kiln_c99_new_text_encoder(result);
    }
    if (callee.tag == KILN_VALUE_BUILTIN && callee.bits == KILN_C99_BUILTIN_TEXT_DECODER) {
        return kiln_c99_new_text_decoder(result, args, argc, thrown);
    }
    if (callee.tag == KILN_VALUE_BOUND) {
        /* `[[Construct]]` on a bound function prepends the bound arguments and forwards to the
         * target. The bound *receiver* is dropped: `new` makes its own, which is the whole
         * difference between calling a bound function and constructing one. The target also
         * takes the wrapper's place as `new.target`, so the instance inherits from the target's
         * `prototype` -- a bound function has none. */
        kiln_bound *bound = (kiln_bound *)(uintptr_t)callee.bits;
        kiln_value *joined;
        if (bound->count == 0) {
            return kiln_c99_construct_with_new_target(
                host, result, bound->target, args, argc,
                (new_target.tag == callee.tag && new_target.bits == callee.bits)
                    ? bound->target : new_target,
                depth, thrown);
        }
        joined = (kiln_value *)malloc((bound->count + argc) * sizeof *joined);
        if (joined == NULL) {
            return KILN_C99_OUT_OF_MEMORY;
        }
        memcpy(joined, bound->arguments, bound->count * sizeof *joined);
        if (argc != 0) {
            memcpy(joined + bound->count, args, argc * sizeof *joined);
        }
        status = kiln_c99_construct_with_new_target(
            host, result, bound->target, joined, bound->count + argc,
            (new_target.tag == callee.tag && new_target.bits == callee.bits)
                ? bound->target : new_target,
            depth, thrown);
        free(joined);
        return status;
    }
    if (callee.tag != KILN_VALUE_FUNCTION) {
        KILN_C99_REFUSE("new on a callee that is not a generated function");
    }
    {
        kiln_closure *closure = kiln_c99_closure_of(callee);
        if (closure->arrow || closure->generator || closure->asynchronous || closure->is_method) {
            return kiln_c99_throw_not_a(thrown, callee, "constructor");
        }
    }
    status = kiln_c99_get_prototype_from_constructor(
        host, new_target, kiln_c99_builtin(KILN_C99_BUILTIN_OBJECT_PROTOTYPE), &prototype,
        depth, thrown);
    if (status != KILN_C99_OK) {
        return status;
    }
    status = kiln_c99_new_object(&instance);
    if (status != KILN_C99_OK) {
        return status;
    }
    kiln_c99_heap_store_value(kiln_c99_object_of(instance), &kiln_c99_object_of(instance)->prototype, prototype);
    /* Inside the body being constructed, `new.target` is the constructor the `new` named -- and
     * the call below is the one that installs it, so the flag that makes `super()` keep it has to
     * be off here whatever the caller was in the middle of. */
    saved_new_target = kiln_c99_new_target;
    saved_adopted = kiln_c99_adopted_this;
    kiln_c99_adopted_this = kiln_c99_undefined();
    kiln_c99_new_target = new_target;
    kiln_c99_inheriting_new_target = 1;
    status = kiln_c99_call(host, &produced, callee, instance, args, argc, depth, thrown);
    kiln_c99_inheriting_new_target = 0;
    kiln_c99_new_target = saved_new_target;
    adopted = kiln_c99_adopted_this;
    kiln_c99_adopted_this = saved_adopted;
    if (status != KILN_C99_OK) {
        return status;
    }
    /* An explicit `return someObject` wins, as the language says. Otherwise a derived
     * constructor whose `super()` built something exotic yields *that* -- the instance allocated
     * above was abandoned the moment `this` was rebound, and returning it would hand back an
     * ordinary object wearing the subclass's prototype. */
    *result = kiln_c99_is_object_value(produced) ? produced
              : adopted.tag != KILN_VALUE_UNDEFINED                                 ? adopted
                                                                                    : instance;
    return KILN_C99_OK;
}

/* `super(...)`: run the parent against the instance being built.
 *
 * A parent written in JavaScript *initialises* the instance -- an ordinary call with `this`
 * threaded through, keeping the `new.target` of the call that is building it, which is how a
 * base constructor can name the subclass it is being built for.
 *
 * A built-in parent *constructs* instead, and the difference is not cosmetic: calling a built-in
 * as a function is a different operation from constructing one. `Map()` without `new` is a
 * TypeError where `new Map()` is a map, and there is no spelling of `Error` as a call that
 * produces the object `super("boom")` has to hand back. So a built-in parent goes through the
 * construction path, and `kiln_c99_adopt_super` installs what it produced as `this`. */
static int kiln_c99_super_call(kiln_host *host, kiln_value *produced, kiln_value parent,
                               kiln_value receiver, const kiln_value *args, size_t argc,
                               int depth, kiln_value *thrown) {
    int status;
    if (parent.tag == KILN_VALUE_BOUND) {
        kiln_bound *bound = (kiln_bound *)(uintptr_t)parent.bits;
        size_t count = bound->count + argc;
        kiln_value *joined = count == 0 ? NULL : malloc(count * sizeof *joined);
        if (count != 0 && joined == NULL) return KILN_C99_OUT_OF_MEMORY;
        if (bound->count != 0) memcpy(joined, bound->arguments, bound->count * sizeof *joined);
        if (argc != 0) memcpy(joined + bound->count, args, argc * sizeof *joined);
        status = kiln_c99_super_call(host, produced, bound->target, receiver, joined, count, depth, thrown);
        free(joined);
        return status;
    }
    if (parent.tag == KILN_VALUE_BUILTIN) {
        return kiln_c99_construct(host, produced, parent, args, argc, depth, thrown);
    }
    {
        kiln_value saved = kiln_c99_adopted_this;
        kiln_c99_adopted_this = kiln_c99_undefined();
        kiln_c99_inheriting_new_target = 1;
        status = kiln_c99_call(host, produced, parent, receiver, args, argc, depth, thrown);
        kiln_c99_inheriting_new_target = 0;
        /* The parent was itself derived from a built-in and adopted what that produced. It
         * returned undefined, like every constructor, so the adoption has to be handed on here
         * or `class B extends A extends Array` would lose the array at the second link. */
        if (status == KILN_C99_OK && produced->tag == KILN_VALUE_UNDEFINED) {
            *produced = kiln_c99_adopted_this;
        }
        kiln_c99_adopted_this = saved;
    }
    return status;
}

/* What `super()` produced becomes `this`.
 *
 * A derived constructor does not initialise the instance `new` allocated -- the language has
 * `super()` *produce* it. For a parent written in JavaScript the two coincide, because a
 * constructor returns undefined and the instance already in hand stands. A built-in parent is
 * the case that differs: it builds an Error, an Array, a Map of its own, and that value is what
 * `this` has to be from there on. `class Stack extends Array` is only subclassing at all if its
 * instances *are* arrays -- an ordinary object wearing `Stack.prototype` with elements copied
 * onto it fails `Array.isArray`, has no live `length`, and stringifies as an object.
 *
 * The prototype the `new` chose moves across, or the subclass's own methods would be
 * unreachable and `instanceof Stack` false about an instance the class demonstrably made. A
 * kind with no prototype field of its own keeps it on the store, which is where the read and
 * the `instanceof` walk both look.
 *
 * Only one level deep, which is what the interpreter also does: the parent's frame is an
 * ordinary call and has no way to report an adoption back to *its* caller, so
 * `class B extends A extends Array` loses the array at the second link. Both engines lose it in
 * the same place, which is the property this backend is measured on. */
static int kiln_c99_adopt_super(kiln_value *receiver, kiln_value produced) {
    kiln_value prototype;
    kiln_value *slot;
    /* A parent that produced nothing -- every JavaScript constructor -- leaves `this` alone. */
    if (!kiln_c99_is_object_value(produced)) {
        return KILN_C99_OK;
    }
    if (receiver->tag != KILN_VALUE_OBJECT ||
        (produced.tag == receiver->tag && produced.bits == receiver->bits)) {
        return KILN_C99_OK;
    }
    prototype = kiln_c99_object_of(*receiver)->prototype;
    if (produced.tag == KILN_VALUE_OBJECT) {
        kiln_c99_heap_store_value(kiln_c99_object_of(produced), &kiln_c99_object_of(produced)->prototype, prototype);
        *receiver = produced;
        kiln_c99_adopted_this = produced;
        return KILN_C99_OK;
    }
    slot = kiln_c99_exotic_store(produced);
    if (slot == NULL) {
        /* An explicitly returned callable/host object retains its own prototype. */
        *receiver = produced;
        kiln_c99_adopted_this = produced;
        return KILN_C99_OK;
    }
    if (slot->tag != KILN_VALUE_OBJECT) {
        int status = kiln_c99_new_object_for_owner(slot, (const void *)(uintptr_t)produced.bits);
        if (status != KILN_C99_OK) {
            return status;
        }
    }
    kiln_c99_heap_store_value(kiln_c99_object_of(*slot), &kiln_c99_object_of(*slot)->prototype, prototype);
    *receiver = produced;
    kiln_c99_adopted_this = produced;
    return KILN_C99_OK;
}

static int kiln_c99_call_val(kiln_host *host, kiln_val *result, kiln_val callee,
                             kiln_val receiver, const kiln_val *args, size_t argc,
                             int depth, kiln_value *thrown) {
    if (KILN_LIKELY(KILN_VAL_IS_CLOSURE(callee))) {
        kiln_closure *plain = (kiln_closure *)KILN_VAL_PTR(callee);
        if (KILN_LIKELY(!plain->generator && !kiln_c99_inheriting_new_target && depth < KILN_C99_MAX_CALL_DEPTH)) {
            kiln_val vreceiver = plain->arrow ? plain->captured_this : receiver;
            if (!plain->arrow && !plain->strict && (vreceiver == KILN_VAL_UNDEFINED || vreceiver == KILN_VAL_NULL)) {
                (void)kiln_c99_global_object_val(&vreceiver);
            }
            *result = KILN_VAL_UNDEFINED;
            return kiln_c99_invoke_closure(host, result, plain, vreceiver, args, argc, depth, thrown, NULL, 0);
        }
    }
    kiln_value bcallee = kiln_val_to_boundary(callee);
    kiln_value breceiver = kiln_val_to_boundary(receiver);
    kiln_value inline_args[16];
    kiln_value *bargs = inline_args;
    if (argc > 16) {
        bargs = (kiln_value *)malloc(argc * sizeof(kiln_value));
        if (bargs == NULL) return KILN_C99_OUT_OF_MEMORY;
    }
    if (args != NULL) {
        for (size_t i = 0; i < argc; ++i) {
            bargs[i] = args[i] == KILN_VAL_HOLE ? kiln_c99_undefined() : kiln_val_to_boundary(args[i]);
        }
    }
    kiln_value bresult = kiln_c99_undefined();
    int status = kiln_c99_call(host, &bresult, bcallee, breceiver, (argc > 0 && args != NULL) ? bargs : NULL, argc, depth, thrown);
    if (bargs != inline_args) free(bargs);
    if (status == KILN_C99_OK) {
        *result = kiln_val_from_boundary(bresult);
    }
    return status;
}

static int kiln_c99_construct_val(kiln_host *host, kiln_val *result, kiln_val callee,
                                  const kiln_val *args, size_t argc,
                                  int depth, kiln_value *thrown) {
    kiln_value bcallee = kiln_val_to_boundary(callee);
    kiln_value inline_args[16];
    kiln_value *bargs = inline_args;
    if (argc > 16) {
        bargs = (kiln_value *)malloc(argc * sizeof(kiln_value));
        if (bargs == NULL) return KILN_C99_OUT_OF_MEMORY;
    }
    if (args != NULL) {
        for (size_t i = 0; i < argc; ++i) {
            bargs[i] = args[i] == KILN_VAL_HOLE ? kiln_c99_undefined() : kiln_val_to_boundary(args[i]);
        }
    }
    kiln_value bresult = kiln_c99_undefined();
    int status = kiln_c99_construct(host, &bresult, bcallee, (argc > 0 && args != NULL) ? bargs : NULL, argc, depth, thrown);
    if (bargs != inline_args) free(bargs);
    if (status == KILN_C99_OK) {
        *result = kiln_val_from_boundary(bresult);
    }
    return status;
}

static int kiln_c99_super_call_val(kiln_host *host, kiln_val *produced, kiln_val parent,
                                   kiln_val receiver, const kiln_val *args, size_t argc,
                                   int depth, kiln_value *thrown) {
    kiln_value bparent = kiln_val_to_boundary(parent);
    kiln_value breceiver = kiln_val_to_boundary(receiver);
    kiln_value inline_args[16];
    kiln_value *bargs = inline_args;
    if (argc > 16) {
        bargs = (kiln_value *)malloc(argc * sizeof(kiln_value));
        if (bargs == NULL) return KILN_C99_OUT_OF_MEMORY;
    }
    if (args != NULL) {
        for (size_t i = 0; i < argc; ++i) {
            bargs[i] = args[i] == KILN_VAL_HOLE ? kiln_c99_undefined() : kiln_val_to_boundary(args[i]);
        }
    }
    kiln_value bproduced = kiln_c99_undefined();
    int status = kiln_c99_super_call(host, &bproduced, bparent, breceiver, (argc > 0 && args != NULL) ? bargs : NULL, argc, depth, thrown);
    if (bargs != inline_args) free(bargs);
    if (status == KILN_C99_OK) {
        *produced = kiln_val_from_boundary(bproduced);
    }
    return status;
}

static int kiln_c99_adopt_super_val(kiln_val *receiver, kiln_val produced) {
    kiln_value breceiver = kiln_val_to_boundary(*receiver);
    kiln_value bproduced = kiln_val_to_boundary(produced);
    int status = kiln_c99_adopt_super(&breceiver, bproduced);
    if (status == KILN_C99_OK) {
        *receiver = kiln_val_from_boundary(breceiver);
    }
    return status;
}

/* Whether a value may *be* a prototype: an object of some kind, or null. Everything else is a
 * primitive, and a primitive has no chain to hang another object's reads off. The two operations
 * that set a prototype disagree about what to do with one -- `{ __proto__: 42 }` ignores it and
 * `Object.setPrototypeOf(o, 42)` throws -- so the test is shared and the reaction is not. */
static int kiln_c99_can_be_prototype(kiln_value value) {
    return value.tag == KILN_VALUE_NULL || value.tag == KILN_VALUE_OBJECT ||
           value.tag == KILN_VALUE_ARRAY || value.tag == KILN_VALUE_FUNCTION ||
           value.tag == KILN_VALUE_BUILTIN || value.tag == KILN_VALUE_METHOD ||
           value.tag == KILN_VALUE_BOUND || value.tag == KILN_VALUE_DATE ||
           value.tag == KILN_VALUE_COLLECTION || value.tag == KILN_VALUE_PROMISE ||
           value.tag == KILN_VALUE_REGEXP || value.tag == KILN_VALUE_ARRAY_BUFFER ||
           value.tag == KILN_VALUE_TYPED_ARRAY || value.tag == KILN_VALUE_DATA_VIEW ||
           value.tag == KILN_VALUE_PROXY || value.tag == KILN_VALUE_GENERATOR;
}

/* `class D extends C` links two pairs of things: the prototype objects, so an instance of `D`
 * finds a method of `C`, and the *constructors*, so `D` finds a static of `C`. The second is
 * what makes this take a function as well as an object. */
static int kiln_c99_set_prototype(kiln_value target, kiln_value prototype) {
    if (target.tag == KILN_VALUE_OBJECT) kiln_c99_prototype_touched(kiln_c99_object_of(target));
    if (target.tag == KILN_VALUE_OBJECT) {
        if (kiln_c99_object_of(target)->module_namespace) {
            KILN_C99_REFUSE("setting the prototype of a module namespace");
        }
        if (!kiln_c99_can_be_prototype(prototype)) {
            /* `{ __proto__: 42 }` is an object literal with no property and no link: the
             * specification honours the `__proto__` production only for an object or null and
             * *ignores* every other value rather than throwing. Storing the primitive left the
             * literal with a prototype nothing could read through, so `Object.getPrototypeOf`
             * answered 42 and the object had lost `Object.prototype` -- a fresh `{}` that could
             * not find `toString`. */
            return KILN_C99_OK;
        }
        kiln_c99_heap_store_value(kiln_c99_object_of(target), &kiln_c99_object_of(target)->prototype, prototype);
        return KILN_C99_OK;
    }
    if (target.tag == KILN_VALUE_FUNCTION || target.tag == KILN_VALUE_BOUND || kiln_c99_exotic_store(target) != NULL) {
        /* A function, an array and a collection all keep the link on the store they already use
         * for named properties, because none of them has a prototype field of its own -- and it
         * is the store the read walk and `instanceof` both consult. */
        kiln_value store;
        int status = kiln_c99_definition_store(target, &store);
        if (status != KILN_C99_OK) {
            return status;
        }
        kiln_c99_heap_store_value(kiln_c99_object_of(store), &kiln_c99_object_of(store)->prototype, prototype);
        return KILN_C99_OK;
    }
    if (target.tag == KILN_VALUE_BUILTIN) {
        /* Built-in callables and prototype tables both have mutable [[Prototype]] links even
         * though their intrinsic surfaces are represented by interned ids. Keep the link in the
         * same own-property store used by reads and reflection. */
        kiln_value store;
        int status = kiln_c99_callable_store(target, &store, 1);
        if (status != KILN_C99_OK) return status;
        kiln_c99_heap_store_value(kiln_c99_object_of(store), &kiln_c99_object_of(store)->prototype, prototype);
        return KILN_C99_OK;
    }
    KILN_C99_REFUSE("setting the prototype of a value with no prototype slot");
}

/* IsConstructor, as far as `class ... extends` needs it. A generated function knows what it was
 * written as; a bound function and a proxy are constructors exactly when their targets are. Every
 * other callable here is a built-in, and the built-ins that are not constructors have no
 * `prototype` either, which the class's own check on `prototype` refuses. */
static int kiln_c99_heritage_constructs(kiln_value value) {
    if (value.tag == KILN_VALUE_PROXY) {
        return kiln_c99_heritage_constructs(kiln_c99_proxy_of(value)->target);
    }
    if (value.tag == KILN_VALUE_BOUND) {
        return kiln_c99_heritage_constructs(((kiln_bound *)(uintptr_t)value.bits)->target);
    }
    if (value.tag == KILN_VALUE_FUNCTION) {
        kiln_closure *closure = kiln_c99_closure_of(value);
        return !(closure->arrow || closure->generator || closure->asynchronous ||
                 closure->is_method);
    }
    if (value.tag == KILN_VALUE_BUILTIN)
        return !kiln_c99_is_namespace(value.bits) && value.bits != KILN_C99_BUILTIN_SYMBOL &&
               value.bits != KILN_C99_BUILTIN_BIGINT &&
               value.bits != KILN_C99_BUILTIN_TYPED_ARRAY;
    if (value.tag == KILN_VALUE_METHOD) {
        for (size_t i = 0; i < sizeof kiln_c99_native_constructors / sizeof *kiln_c99_native_constructors; ++i)
            if (kiln_c99_native_constructors[i].method == value.bits) return 1;
    }
    return 0;
}

static int kiln_c99_is_constructor(kiln_value value) {
    return kiln_c99_heritage_constructs(value);
}

/* `SetPrototype` as emitted: a function target is `class D extends X` linking the two
 * constructors, and ClassDefinitionEvaluation refuses a heritage that is neither `null` nor a
 * constructor before anything is linked. Without the check `class U extends 1 {}` built a class
 * natively, where the interpreter and node throw (issue 25). */
static int kiln_c99_set_prototype_checked(kiln_value target, kiln_value prototype,
                                          kiln_value *thrown) {
    if (target.tag == KILN_VALUE_FUNCTION && prototype.tag != KILN_VALUE_NULL &&
        !kiln_c99_heritage_constructs(prototype)) {
        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                    "class heritage is not a constructor or null");
    }
    return kiln_c99_set_prototype(target, prototype);
}

/* `x instanceof C` walks the same chain a property read walks, looking for `C.prototype` rather
 * than for a name -- which is why the two must agree about the shape of the chain. */
/* The intrinsic is recognized by immutable realm-owned identity, never by function name.
 * The bootstrap installs this non-writable, non-configurable slot before user code. */
static int kiln_c99_intrinsic_has_instance(kiln_value callable) {
    if (callable.tag != KILN_VALUE_FUNCTION || !kiln_c99_closure_of(callable)->is_method) return 0;
    kiln_value store;
    kiln_property property;
    if (kiln_c99_callable_store(kiln_c99_builtin(KILN_C99_BUILTIN_FUNCTION_PROTOTYPE), &store, 0) != KILN_C99_OK ||
        store.tag != KILN_VALUE_OBJECT) return 0;
    return kiln_c99_property_read_own_value(store, &kiln_c99_symbol_key_has_instance, NULL, &property) &&
        !property.writable && !property.configurable && kiln_c99_strict_equal(property.value, callable);
}

static int kiln_c99_instance_of(kiln_host *host, kiln_value value, kiln_value constructor, int *answer, int depth, kiln_value *thrown) {
    kiln_value handler;
    int status;
    if (!kiln_c99_can_be_prototype(constructor) || constructor.tag == KILN_VALUE_NULL)
        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR, "Right-hand side of instanceof is not an object");
    status = kiln_c99_probe_symbol(host, constructor, "@@hasInstance", 13, &handler, depth, thrown);
    if (status != KILN_C99_OK) return status;
    if (handler.tag != KILN_VALUE_UNDEFINED && handler.tag != KILN_VALUE_NULL && !kiln_c99_intrinsic_has_instance(handler)) {
        kiln_value result;
        if (!kiln_c99_is_callable(handler))
            return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR, "Symbol.hasInstance is not callable");
        status = kiln_c99_call(host, &result, handler, constructor, &value, 1, depth, thrown);
        if (status == KILN_C99_OK) *answer = kiln_c99_truthy(result);
        return status;
    }
    if (!kiln_c99_is_callable(constructor))
        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR, "Right-hand side of instanceof is not callable");
    return kiln_c99_ordinary_has_instance(host, value, constructor, answer, depth, thrown);
}

static int kiln_c99_ordinary_has_instance(kiln_host *host, kiln_value value, kiln_value constructor, int *answer, int depth, kiln_value *thrown) {
    kiln_value prototype, current;
    size_t steps;
    int status;
    *answer = 0;
    if (!kiln_c99_is_callable(constructor)) return KILN_C99_OK;
    if (constructor.tag == KILN_VALUE_BOUND)
        return kiln_c99_instance_of(host, value, ((kiln_bound *)(uintptr_t)constructor.bits)->target, answer, depth, thrown);
    if (value.tag == KILN_VALUE_PROXY) value = kiln_c99_proxy_inner(value);
    if (constructor.tag == KILN_VALUE_PROXY) constructor = kiln_c99_proxy_inner(constructor);
    /* OrdinaryHasInstance returns false for every primitive before reading the constructor's
     * `prototype` property. In particular, `"x" instanceof Function.prototype` is false even
     * though Function.prototype has no object-valued `prototype`. */
    if (!kiln_c99_is_reference(value) && value.tag != KILN_VALUE_DATE)
        return KILN_C99_OK;
    /* A builtin constructor has no prototype object here; the question it answers is about the
     * tag instead, which is the same question and a shorter walk. */
    if (constructor.tag == KILN_VALUE_BUILTIN) {
        switch (constructor.bits) {
        case KILN_C99_BUILTIN_DATE: *answer = value.tag == KILN_VALUE_DATE; return KILN_C99_OK;
        case KILN_C99_BUILTIN_REGEXP: *answer = value.tag == KILN_VALUE_REGEXP; return KILN_C99_OK;
        case KILN_C99_BUILTIN_ARRAY:
            *answer = value.tag == KILN_VALUE_ARRAY ||
                      (value.tag == KILN_VALUE_SHARED && kiln_c99_shared_is_array(value));
            return KILN_C99_OK;
        case KILN_C99_BUILTIN_ARRAY_BUFFER:
            *answer = value.tag == KILN_VALUE_ARRAY_BUFFER &&
                      kiln_c99_typed_buffer_of(value)->shared == NULL;
            return KILN_C99_OK;
        case KILN_C99_BUILTIN_SHARED_ARRAY_BUFFER:
            *answer = value.tag == KILN_VALUE_ARRAY_BUFFER &&
                      kiln_c99_typed_buffer_of(value)->shared != NULL;
            return KILN_C99_OK;
        case KILN_C99_BUILTIN_DATA_VIEW:
            *answer = value.tag == KILN_VALUE_DATA_VIEW;
            return KILN_C99_OK;
        case KILN_C99_BUILTIN_TYPED_ARRAY:
            /* The abstract `%TypedArray%` constructor is the shared parent of the concrete
             * numeric constructors. Its ordinary instance check accepts every typed view. */
            *answer = value.tag == KILN_VALUE_TYPED_ARRAY;
            return KILN_C99_OK;
        case KILN_C99_BUILTIN_FUNCTION_PROTOTYPE:
            return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                        "Function has non-object prototype 'undefined' in instanceof check");
        case KILN_C99_BUILTIN_INT8_ARRAY:
        case KILN_C99_BUILTIN_UINT8_ARRAY:
        case KILN_C99_BUILTIN_UINT8_CLAMPED_ARRAY:
        case KILN_C99_BUILTIN_INT16_ARRAY:
        case KILN_C99_BUILTIN_UINT16_ARRAY:
        case KILN_C99_BUILTIN_INT32_ARRAY:
        case KILN_C99_BUILTIN_UINT32_ARRAY:
        case KILN_C99_BUILTIN_FLOAT32_ARRAY:
        case KILN_C99_BUILTIN_FLOAT64_ARRAY:
        case KILN_C99_BUILTIN_FLOAT16_ARRAY:
        case KILN_C99_BUILTIN_BIG_INT64_ARRAY:
        case KILN_C99_BUILTIN_BIG_UINT64_ARRAY: {
            kiln_typed_kind kind;
            *answer = kiln_c99_builtin_typed_kind(constructor.bits, &kind) &&
                      value.tag == KILN_VALUE_TYPED_ARRAY &&
                      kiln_c99_typed_array_of(value)->kind == kind;
            return KILN_C99_OK;
        }
        case KILN_C99_BUILTIN_ERROR:
        case KILN_C99_BUILTIN_TYPE_ERROR:
        case KILN_C99_BUILTIN_RANGE_ERROR:
        case KILN_C99_BUILTIN_REFERENCE_ERROR:
        case KILN_C99_BUILTIN_SYNTAX_ERROR:
        case KILN_C99_BUILTIN_URI_ERROR:
        case KILN_C99_BUILTIN_EVAL_ERROR:
        case KILN_C99_BUILTIN_SUPPRESSED_ERROR:
        case KILN_C99_BUILTIN_AGGREGATE_ERROR: {
            /* `Error` is the base of all of them, so it accepts any kind; the others accept only
             * their own. There is no prototype chain here to walk, and this is the same answer
             * one would give. */
            kiln_value kind;
            *answer = 0;
            if (value.tag == KILN_VALUE_OBJECT &&
                kiln_c99_get(NULL, value, "@@error", 7, &kind, 0, NULL) == KILN_C99_OK &&
                kind.tag == KILN_VALUE_NUMBER) {
                *answer = constructor.bits == KILN_C99_BUILTIN_ERROR ||
                          (uint64_t)kiln_c99_as_number(kind) == constructor.bits;
            }
            return KILN_C99_OK;
        }
        case KILN_C99_BUILTIN_PROMISE:
            *answer = value.tag == KILN_VALUE_PROMISE;
            return KILN_C99_OK;
        case KILN_C99_BUILTIN_MAP:
            *answer = value.tag == KILN_VALUE_COLLECTION &&
                      kiln_c99_collection_of(value)->is_map &&
                      !kiln_c99_collection_of(value)->is_weak;
            return KILN_C99_OK;
        case KILN_C99_BUILTIN_SET:
            *answer = value.tag == KILN_VALUE_COLLECTION &&
                      !kiln_c99_collection_of(value)->is_map &&
                      !kiln_c99_collection_of(value)->is_weak;
            return KILN_C99_OK;
        case KILN_C99_BUILTIN_WEAK_MAP:
            *answer = value.tag == KILN_VALUE_COLLECTION &&
                      kiln_c99_collection_of(value)->is_map &&
                      kiln_c99_collection_of(value)->is_weak;
            return KILN_C99_OK;
        case KILN_C99_BUILTIN_WEAK_SET:
            *answer = value.tag == KILN_VALUE_COLLECTION &&
                      !kiln_c99_collection_of(value)->is_map &&
                      kiln_c99_collection_of(value)->is_weak;
            return KILN_C99_OK;
        case KILN_C99_BUILTIN_OBJECT:
            /* `x instanceof Object` is true of every value that is not one of the seven
             * primitive types, so the test is the complement -- the list of things that are
             * *not* objects is the short and closed one, and it is the shape the interpreter's
             * own `is_an_object` uses.
             *
             * Enumerating the positive side instead is what left a function, a regexp, a
             * promise, a generator, a bound function and every `Temporal` value answering
             * `false` about a value the program is holding in its hand. That was a silent wrong
             * answer rather than a refusal, and no probe asked, so the two engines had drifted
             * apart here without the gate noticing.
             *
             * A proxy never arrives: `instanceof` refuses one at the top of this function,
             * because the answer is its handler's rather than its target's. The three internal
             * tags below are not program-visible values -- a cell lives only in a captured slot,
             * a hole only in an array's storage, and an uninitialized slot throws on read -- so
             * they are excluded rather than silently counted as objects. */
            *answer = value.tag != KILN_VALUE_UNDEFINED && value.tag != KILN_VALUE_NULL &&
                      value.tag != KILN_VALUE_BOOLEAN && value.tag != KILN_VALUE_NUMBER &&
                      value.tag != KILN_VALUE_STRING && value.tag != KILN_VALUE_SYMBOL &&
                      value.tag != KILN_VALUE_BIGINT && value.tag != KILN_VALUE_CELL &&
                      value.tag != KILN_VALUE_HOLE && value.tag != KILN_VALUE_UNINITIALIZED;
            return KILN_C99_OK;
        case KILN_C99_BUILTIN_URL: {
            kiln_value marker;
            *answer = 0;
            if (value.tag == KILN_VALUE_OBJECT &&
                kiln_c99_get(NULL, value, "@@url", 5, &marker, 0, NULL) == KILN_C99_OK &&
                marker.tag == KILN_VALUE_BOOLEAN && marker.bits != 0) {
                *answer = 1;
            }
            return KILN_C99_OK;
        }
        case KILN_C99_BUILTIN_BUFFER:
            *answer = value.tag == KILN_VALUE_TYPED_ARRAY &&
                      kiln_c99_typed_array_of(value)->is_buffer;
            return KILN_C99_OK;
        case KILN_C99_BUILTIN_FUNCTION: {
            kiln_value type = kiln_c99_typeof(value);
            size_t type_len = 0;
            const char *type_bytes = kiln_c99_string_ascii_bytes(type, &type_len);
            *answer = type_bytes != NULL && type_len == 8 && memcmp(type_bytes, "function", 8) == 0;
            return KILN_C99_OK;
        }
        case KILN_C99_BUILTIN_STRING:
        case KILN_C99_BUILTIN_NUMBER:
        case KILN_C99_BUILTIN_BOOLEAN:
        case KILN_C99_BUILTIN_SYMBOL:
        case KILN_C99_BUILTIN_BIGINT: {
            uint8_t expected = constructor.bits == KILN_C99_BUILTIN_STRING
                                   ? KILN_C99_WRAPPER_STRING
                               : constructor.bits == KILN_C99_BUILTIN_NUMBER
                                   ? KILN_C99_WRAPPER_NUMBER
                               : constructor.bits == KILN_C99_BUILTIN_BOOLEAN
                                   ? KILN_C99_WRAPPER_BOOLEAN
                               : constructor.bits == KILN_C99_BUILTIN_SYMBOL
                                   ? KILN_C99_WRAPPER_SYMBOL
                                   : KILN_C99_WRAPPER_BIGINT;
            *answer = value.tag == KILN_VALUE_OBJECT &&
                      kiln_c99_object_of(value)->primitive_wrapper_kind == expected;
            return KILN_C99_OK;
        }
        default:
            KILN_C99_REFUSE("instanceof against a built-in whose instances this backend does not "
                            "model");
        }
    }
    if (constructor.tag != KILN_VALUE_FUNCTION) {
        KILN_C99_REFUSE("instanceof against a constructor that is not a generated function");
    }
    if (value.tag == KILN_VALUE_UNDEFINED || value.tag == KILN_VALUE_NULL ||
        value.tag == KILN_VALUE_BOOLEAN || value.tag == KILN_VALUE_NUMBER ||
        value.tag == KILN_VALUE_STRING || value.tag == KILN_VALUE_SYMBOL ||
        value.tag == KILN_VALUE_BIGINT) {
        *answer = 0;
        return KILN_C99_OK;
    }
    status = kiln_c99_prototype_object(constructor, &prototype);
    if (status != KILN_C99_OK) {
        return status;
    }
    *answer = 0;
    /* A `prototype` that is not an object -- `F.prototype = undefined` now reads back as the
     * undefined it is (issue 28) -- matches nothing. Walking on would compare it with the
     * undefined an ordinary object's unrecorded link holds and answer true. OrdinaryHasInstance
     * throws a TypeError here, after rejecting primitive left-hand values. */
    if (prototype.tag == KILN_VALUE_UNDEFINED || prototype.tag == KILN_VALUE_NULL ||
        prototype.tag == KILN_VALUE_BOOLEAN || prototype.tag == KILN_VALUE_NUMBER ||
        prototype.tag == KILN_VALUE_STRING || prototype.tag == KILN_VALUE_SYMBOL ||
        prototype.tag == KILN_VALUE_BIGINT) {
        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                    "Function has non-object prototype in instanceof check");
    }
    current = value;
    {
        /* An array or a collection built by `super()` in a derived constructor carries its
         * prototype on the store rather than on itself, because the value has no prototype field
         * of its own. The walk has to begin there or `stack instanceof Stack` is false about an
         * instance the class demonstrably made. */
        kiln_value *slot = kiln_c99_exotic_store(value);
        if (slot != NULL) {
            current = *slot;
        }
    }
    for (steps = 0; steps < KILN_C99_MAX_PROTOTYPE_DEPTH; ++steps) {
        if (current.tag == KILN_VALUE_FUNCTION || current.tag == KILN_VALUE_METHOD ||
            current.tag == KILN_VALUE_BOUND) {
            kiln_value store;
            kiln_value next = kiln_c99_undefined();
            status = kiln_c99_callable_store(current, &store, 0);
            if (status != KILN_C99_OK) return status;
            if (store.tag == KILN_VALUE_OBJECT) next = kiln_c99_object_prototype_read(store);
            if (next.tag == KILN_VALUE_UNDEFINED)
                next = current.tag == KILN_VALUE_FUNCTION
                           ? kiln_c99_function_kind_prototype(kiln_c99_closure_of(current))
                           : kiln_c99_builtin(KILN_C99_BUILTIN_FUNCTION_PROTOTYPE);
            current = next;
            if (current.tag == prototype.tag && current.bits == prototype.bits) {
                *answer = 1;
                return KILN_C99_OK;
            }
            continue;
        }
        if (current.tag != KILN_VALUE_OBJECT) {
            return KILN_C99_OK;
        }
        current = kiln_c99_object_of(current)->prototype;
        if (current.tag == prototype.tag && current.bits == prototype.bits) {
            *answer = 1;
            return KILN_C99_OK;
        }
    }
    return KILN_C99_OK;
}

static kiln_value kiln_c99_typeof(kiln_value value) {
    static const char undefined_text[] = "undefined";
    static const char object_text[] = "object";
    static const char boolean_text[] = "boolean";
    static const char number_text[] = "number";
    static const char string_text[] = "string";
    static const char function_text[] = "function";
    static const char symbol_text[] = "symbol";
    static const char bigint_text[] = "bigint";
    /* A proxy is transparent to `typeof`: the answer is about what it stands in front of. */
    /* The answers are borrowed static unit strings, never raw literals: a string value
     * carries a `kiln_string` header at its pointer, and wrapping a literal reads the
     * literal's bytes as a header further down the line. */
    value = kiln_c99_proxy_inner(value);
    switch (value.tag) {
    case KILN_VALUE_BIGINT:
        return kiln_c99_static_text(bigint_text, sizeof(bigint_text) - 1);
    case KILN_VALUE_SYMBOL:
        return kiln_c99_static_text(symbol_text, sizeof(symbol_text) - 1);
    case KILN_VALUE_NULL:
        return kiln_c99_static_text(object_text, sizeof(object_text) - 1);
    case KILN_VALUE_BOOLEAN:
        return kiln_c99_static_text(boolean_text, sizeof(boolean_text) - 1);
    case KILN_VALUE_NUMBER:
        return kiln_c99_static_text(number_text, sizeof(number_text) - 1);
    case KILN_VALUE_STRING:
        return kiln_c99_static_text(string_text, sizeof(string_text) - 1);
    case KILN_VALUE_BUILTIN:
        /* A namespace is an object holding functions and cannot be called; a constructor is a
         * function. `typeof Math === "object"` is what a library feature-detects on.
         *
         * `Function.prototype` is the exception, and it is grouped with the namespaces everywhere
         * else here for a good reason: it holds `call`, `apply` and `bind`, and this backend has
         * no body to run for it. But the language says it is callable, and the interpreter says
         * "function". Answering "object" was the silent kind of wrong -- no refusal, no
         * diagnostic, just the other branch of a `typeof x === "function"` test, which is how
         * real code decides whether it may call something. Only `typeof` is special-cased;
         * calling it still refuses by name, which is the honest answer for a body that does not
         * exist. */
        if (value.bits == KILN_C99_BUILTIN_FUNCTION_PROTOTYPE) {
            return kiln_c99_static_text(function_text, sizeof(function_text) - 1);
        }
        if (value.bits == KILN_C99_BUILTIN_TYPED_ARRAY_PROTOTYPE ||
            (value.bits >= KILN_C99_BUILTIN_INT8_ARRAY_PROTOTYPE &&
             value.bits <= KILN_C99_BUILTIN_FLOAT64_ARRAY_PROTOTYPE) ||
            value.bits == KILN_C99_BUILTIN_FLOAT16_ARRAY_PROTOTYPE ||
            value.bits == KILN_C99_BUILTIN_BIG_INT64_ARRAY_PROTOTYPE ||
            value.bits == KILN_C99_BUILTIN_BIG_UINT64_ARRAY_PROTOTYPE) {
            return kiln_c99_static_text(object_text, sizeof(object_text) - 1);
        }
        if (kiln_c99_is_namespace(value.bits)) {
            return kiln_c99_static_text(object_text, sizeof(object_text) - 1);
        }
        return kiln_c99_static_text(function_text, sizeof(function_text) - 1);
    case KILN_VALUE_FUNCTION:
    case KILN_VALUE_METHOD:
    /* A bound function is a function, and so is a settler: `typeof resolve` is what an executor
     * checks before calling it. */
    case KILN_VALUE_BOUND:
    case KILN_VALUE_COMMONJS_REQUIRE:
    case KILN_VALUE_SETTLER:
        return kiln_c99_static_text(function_text, sizeof(function_text) - 1);
    case KILN_VALUE_OBJECT:
    case KILN_VALUE_SHARED:
    case KILN_VALUE_ARRAY:
    case KILN_VALUE_GENERATOR:
    case KILN_VALUE_DATE:
    case KILN_VALUE_COLLECTION:
    case KILN_VALUE_REGEXP:
    case KILN_VALUE_PROMISE:
    case KILN_VALUE_ARRAY_BUFFER:
    case KILN_VALUE_TYPED_ARRAY:
    /* A DataView, listed rather than left to fall through: the `default:` below answers
     * "undefined", and `typeof x === "undefined"` is how a bundle feature-detects. */
    case KILN_VALUE_DATA_VIEW:
    /* Every `Temporal` type is an ordinary object to `typeof`; none of them is callable. */
    case KILN_VALUE_TEMPORAL:
        return kiln_c99_static_text(object_text, sizeof(object_text) - 1);
    case KILN_VALUE_UNDEFINED:
    default:
        return kiln_c99_static_text(undefined_text, sizeof(undefined_text) - 1);
    }
}
