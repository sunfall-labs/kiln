static const char *kiln_c99_error_name(uint64_t builtin) {
    switch (builtin) {
    case KILN_C99_BUILTIN_TYPE_ERROR: return "TypeError";
    case KILN_C99_BUILTIN_RANGE_ERROR: return "RangeError";
    case KILN_C99_BUILTIN_REFERENCE_ERROR: return "ReferenceError";
    case KILN_C99_BUILTIN_SYNTAX_ERROR: return "SyntaxError";
    case KILN_C99_BUILTIN_URI_ERROR: return "URIError";
    /* `EvalError` is one of the seven the language defines, and it was the only one missing here.
     * The constant existed, the global resolved, and `kiln_c99_builtin_name` knew the name -- but
     * this function is what decides whether a built-in *is* an error, so `new EvalError("m")`
     * refused, `EvalError.prototype` had no owner, and `kiln_c99_throw_error` could not raise one.
     * That last one is the quiet part: a throw that cannot be built returns a plain error status,
     * which the caller reports as whatever it was doing at the time. */
    case KILN_C99_BUILTIN_EVAL_ERROR: return "EvalError";
    case KILN_C99_BUILTIN_ERROR: return "Error";
    case KILN_C99_BUILTIN_AGGREGATE_ERROR: return "AggregateError";
    default: return NULL;
    }
}

/* The kind an error records, or 0 for every other value. This is the question "is this an error,
 * and which one" asked once, instead of the reserved-key scan it replaces being written out at
 * each place that wanted the answer.
 *
 * Own-only, and by the reserved key rather than by `name`: a program that assigns `err.name`
 * renames the error without changing what it *is*, and an ordinary object that happens to carry a
 * `name` is not one. Own-only also keeps it cheap enough for a property read to ask -- there is no
 * chain to walk and no host to thread through. */
static uint64_t kiln_c99_error_kind(kiln_value value) {
    kiln_object *object;
    size_t slot;
    if (value.tag != KILN_VALUE_OBJECT) {
        return 0;
    }
    object = kiln_c99_object_of(value);
    for (slot = 0; slot < object->count; ++slot) {
        if (kiln_c99_same_key(&object->properties[slot], "@@error", 7)) {
            return (uint64_t)kiln_c99_as_number(object->properties[slot].value);
        }
    }
    return 0;
}

/* `Error.prototype.toString`: "name: message", and the one text form an object has that is not
 * "[object Object]".
 *
 * It reads the two properties *now* rather than reusing the text built when the error was
 * constructed, because assigning `err.name` changes what the error calls itself: `String(err)`
 * after `err.name = "Custom"` is "Custom: x", which a remembered string cannot say. Either half
 * being empty drops the separator with it, so a message-less error is "Error", not "Error: ". */
static int kiln_c99_error_to_string(kiln_host *host, kiln_value *result, kiln_value receiver,
                                    int depth, kiln_value *thrown) {
    kiln_value name;
    kiln_value message;
    kiln_text built = {NULL, 0, 0};
    int found = kiln_c99_get(host, receiver, "name", 4, &name, depth, thrown);
    if (found == KILN_C99_THROWN) {
        return found;
    }
    name = found != KILN_C99_OK || name.tag == KILN_VALUE_UNDEFINED
               ? kiln_c99_copy_text("Error", 5)
               : kiln_c99_text(name);
    found = kiln_c99_get(host, receiver, "message", 7, &message, depth, thrown);
    if (found == KILN_C99_THROWN) {
        return found;
    }
    message = found != KILN_C99_OK || message.tag == KILN_VALUE_UNDEFINED
                  ? kiln_c99_from_units(NULL, 0)
                  : kiln_c99_text(message);
    if (name.tag != KILN_VALUE_STRING || message.tag != KILN_VALUE_STRING) {
        return KILN_C99_OUT_OF_MEMORY;
    }
    if (name.len == 0 || message.len == 0) {
        *result = name.len == 0 ? message : name;
        return KILN_C99_OK;
    }
    if (kiln_c99_text_append(&built, name.ptr, name.len) != KILN_C99_OK ||
        kiln_c99_text_append_c(&built, ": ") != KILN_C99_OK ||
        kiln_c99_text_append(&built, message.ptr, message.len) != KILN_C99_OK) {
        free(built.bytes);
        return KILN_C99_OUT_OF_MEMORY;
    }
    *result = kiln_c99_text_value(&built);
    return KILN_C99_OK;
}

/* An error is an ordinary object carrying `stack` and, when one was passed, `message` -- neither
 * enumerated: `err.message` reads and `Object.keys(err)` is empty, which is why
 * `JSON.stringify(err)` is `{}`. The kind is remembered in a reserved property rather than
 * inferred from `name`, so a program that overwrites `name` does not change what it *is*.
 *
 * `name` is deliberately *not* one of them. It belongs to `Error.prototype`, and an own one here
 * would shadow whatever a subclass says about itself -- a `name = "TimeoutError"` class field, or
 * a `get name()` on the subclass prototype -- so `e.name` answered "Error" for an error the
 * program had explicitly named. It is answered from the kind when the chain misses instead,
 * beside the `toString` and `constructor` that already work that way. `message` has the same
 * shape: own only when one was passed, and "" from the kind otherwise. */
static int kiln_c99_new_error(kiln_value *out, uint64_t kind, kiln_value message) {
    const char *name = kiln_c99_error_name(kind);
    kiln_value error;
    kiln_text stack = {NULL, 0, 0};
    kiln_value text = message.tag == KILN_VALUE_UNDEFINED ? kiln_c99_from_units(NULL, 0)
                                                          : kiln_c99_text(message);
    int status;
    if (name == NULL || text.tag != KILN_VALUE_STRING) {
        KILN_C99_REFUSE("an error with no name, or a message with no text form");
    }
    status = kiln_c99_new_object(&error);
    if (status != KILN_C99_OK) {
        return status;
    }
    /* The [[ErrorData]] slot, set at the one place an error is built so that nothing a program
     * does afterwards can add or remove it. */
    kiln_c99_object_of(error)->error_brand = 1;
    if (kiln_c99_text_append_c(&stack, name) != KILN_C99_OK ||
        (text.len != 0 && (kiln_c99_text_append_c(&stack, ": ") != KILN_C99_OK ||
                           kiln_c99_text_append(&stack, text.ptr, text.len) != KILN_C99_OK))) {
        free(stack.bytes);
        return KILN_C99_OUT_OF_MEMORY;
    }
    /* `stack` first, because that is the order the own keys come out in: the header is attached
     * by the constructor before the message is. */
    status = kiln_c99_put(NULL, error, "stack", 5, kiln_c99_text_value(&stack), 0, NULL);
    if (status == KILN_C99_OK && message.tag != KILN_VALUE_UNDEFINED) {
        status = kiln_c99_put(NULL, error, "message", 7, text, 0, NULL);
    }
    if (status == KILN_C99_OK) {
        status = kiln_c99_put(NULL, error, "@@error", 7, kiln_c99_number((double)kind), 0, NULL);
    }
    if (status != KILN_C99_OK) {
        return status;
    }
    {
        /* None of them is enumerated. The reserved one is hidden by its prefix anyway; the
         * others have to be marked. */
        kiln_object *object = kiln_c99_object_of(error);
        size_t index;
        if (object->shape != NULL) {
            kiln_c99_ensure_dictionary(object);
            object->shape = NULL;
        }
        for (index = 0; index < object->count; ++index) {
            object->properties[index].enumerable = 0;
        }
    }
    *out = error;
    return KILN_C99_OK;
}

/* What `Promise.any` rejects with when nothing succeeded: the reasons, and a sentence saying so.
 *
 * An `AggregateError`, which is what node names it and what the interpreter builds. It used to be
 * a plain `Error` carrying an `errors` array -- a *plausible* wrong answer, in that everything a
 * handler reads off it was right except what it is, and `e.constructor.name` answered "Error".
 * `errors` is not enumerated, for the same reason an error's `message` is not: it is the error
 * saying what happened, not a property of an object. */
static int kiln_c99_aggregate_error(kiln_value *out, kiln_value reasons) {
    kiln_value aggregate;
    int status = kiln_c99_new_error(&aggregate, KILN_C99_BUILTIN_AGGREGATE_ERROR,
                                    kiln_c99_copy_text("All promises were rejected", 26));
    if (status != KILN_C99_OK) {
        return status;
    }
    status = kiln_c99_put(NULL, aggregate, "errors", 6, reasons, 0, NULL);
    if (status != KILN_C99_OK) {
        return status;
    }
    {
        kiln_object *object = kiln_c99_object_of(aggregate);
        if (object->shape != NULL) {
            kiln_c99_ensure_dictionary(object);
            object->shape = NULL;
        }
        if (object->count != 0) {
            object->properties[object->count - 1].enumerable = 0;
        }
    }
    *out = aggregate;
    return KILN_C99_OK;
}

static int kiln_c99_aggregate_error_from_args(kiln_host *host, kiln_value *out,
                                              const kiln_value *args, size_t argc, int depth,
                                              kiln_value *thrown) {
    kiln_value errors;
    int status = kiln_c99_new_array(&errors);
    if (status != KILN_C99_OK) return status;
    status = kiln_c99_iterate(host, errors, argc > 0 ? args[0] : kiln_c99_undefined(), depth,
                              thrown);
    if (status != KILN_C99_OK) return status;
    status = kiln_c99_new_error(out, KILN_C99_BUILTIN_AGGREGATE_ERROR,
                                argc > 1 ? args[1] : kiln_c99_undefined());
    if (status != KILN_C99_OK) return status;
    status = kiln_c99_put(NULL, *out, "errors", 6, errors, 0, NULL);
    if (status != KILN_C99_OK) return status;
    {
        kiln_object *error_obj = kiln_c99_object_of(*out);
        if (error_obj->shape != NULL) {
            kiln_c99_ensure_dictionary(error_obj);
            error_obj->shape = NULL;
        }
        if (error_obj->count > 0) {
            error_obj->properties[error_obj->count - 1].enumerable = 0;
        }
    }
    return KILN_C99_OK;
}

/* The own slot a private name occupies, or NULL.
 *
 * Own-only, and that own-only rule *is* the brand check: an object of another class does not carry
 * the name, however much of the rest of the shape it shares. Nothing walks the prototype chain
 * here for the same reason -- inheriting a private name would be inheriting somebody else's
 * identity. A static private member lives on the constructor, whose properties are in its own
 * store rather than in an object. */
static kiln_property *kiln_c99_own_private(kiln_value target, const char *key, size_t key_len) {
    kiln_object *object;
    size_t index;
    if (target.tag == KILN_VALUE_FUNCTION) {
        kiln_value store = kiln_c99_closure_of(target)->properties;
        if (store.tag != KILN_VALUE_OBJECT) {
            return NULL;
        }
        object = kiln_c99_object_of(store);
    } else if (target.tag == KILN_VALUE_OBJECT) {
        object = kiln_c99_object_of(target);
    } else {
        return NULL;
    }
    if (object->shape != NULL) {
        kiln_c99_ensure_dictionary(object);
    }
    {
        kiln_gc_heap *heap = kiln_c99_gc_heap();
        index = kiln_c99_own_index(object, kiln_c99_interned_in(heap, key, key_len), key,
                                   key_len);
    }
    return index == object->count ? NULL : &object->properties[index];
}

/* A private element refusing what was asked of it: a TypeError naming the member.
 *
 * The stored key is `@@private:<scope>:<name>` -- reserved so nothing that enumerates own string
 * properties can see it and no computed access can spell it -- and the message wants the `<name>`
 * half, spelled the way the program spells it.
 *
 * Five refusals rather than two, because a private element has a *kind*: the object may not carry
 * the name at all (0 reading, 1 writing), or carry it as an accessor with only the other half
 * (2 reading, 3 writing), or carry it as a method, which is readable and never writable (4). The
 * wording is the interpreter's, exactly: `error.message` is program-visible, so a fault the two
 * engines word differently is a divergence like any other. */
static int kiln_c99_private_error(kiln_value *thrown, const char *key, size_t key_len,
                                  int mode) {
    kiln_text text = {NULL, 0, 0};
    kiln_value error;
    size_t at = 0;
    size_t colons = 0;
    int status;
    /* A character rather than a literal, for the reason `kiln_c99_set_function_name` gives: the
     * runtime prelude lives in a raw string whose terminator is a quote followed by a hash. */
    char hash = 0x23;
    const char *before;
    const char *after;
    while (at < key_len && colons < 2) {
        if (key[at] == ':') {
            colons += 1;
        }
        at += 1;
    }
    if (colons < 2) {
        at = 0;
    }
    switch (mode) {
    case 1:
        before = "Cannot write private member ";
        after = " to an object whose class did not declare it";
        break;
    case 2:
        before = "'";
        after = "' was defined without a getter";
        break;
    case 3:
        before = "'";
        after = "' was defined without a setter";
        break;
    case 4:
        before = "Private method '";
        after = "' is not writable";
        break;
    default:
        before = "Cannot read private member ";
        after = " from an object whose class did not declare it";
        break;
    }
    if (kiln_c99_text_append_c(&text, before) != KILN_C99_OK ||
        kiln_c99_text_append(&text, &hash, 1) != KILN_C99_OK ||
        kiln_c99_text_append(&text, key + at, key_len - at) != KILN_C99_OK ||
        kiln_c99_text_append_c(&text, after) != KILN_C99_OK) {
        free(text.bytes);
        return KILN_C99_OUT_OF_MEMORY;
    }
    status = kiln_c99_new_error(&error, KILN_C99_BUILTIN_TYPE_ERROR, kiln_c99_text_value(&text));
    if (status != KILN_C99_OK) {
        return status;
    }
    *thrown = error;
    return KILN_C99_THROWN;
}

/* Raise a language-level error: build it, hand it to the caller as the thrown value, and report
 * that something was thrown rather than that something could not be done.
 *
 * The difference matters because a `catch` takes the first and must not take the second: a
 * TypeError is the program's business and an unimplemented instruction is not. */
/* How a value is named in a diagnostic: `typeof` for the tags that have a word for themselves,
 * and the kind's own word for the rest. The vocabulary is deliberately the interpreter's
 * `type_name` -- `error.message` is program-visible, so a fault the two engines word differently
 * is a divergence like any other, and this is the one place the wording is chosen. */
static const char *kiln_c99_kind_name(kiln_value value) {
    switch (value.tag) {
    case KILN_VALUE_UNDEFINED: return "undefined";
    case KILN_VALUE_NULL: return "null";
    case KILN_VALUE_BOOLEAN: return "boolean";
    case KILN_VALUE_NUMBER: return "number";
    case KILN_VALUE_STRING: return "string";
    case KILN_VALUE_BIGINT: return "bigint";
    case KILN_VALUE_TEMPORAL: return "object";
    case KILN_VALUE_SYMBOL: return "symbol";
    case KILN_VALUE_ARRAY: return "array";
    case KILN_VALUE_DATE: return "date";
    case KILN_VALUE_REGEXP: return "regexp";
    case KILN_VALUE_PROMISE: return "promise";
    case KILN_VALUE_GENERATOR: return "generator";
    case KILN_VALUE_FUNCTION:
    case KILN_VALUE_METHOD:
    case KILN_VALUE_BOUND:
    case KILN_VALUE_SETTLER: return "function";
    /* A namespace is an object holding functions rather than a callable, which is the same split
     * `typeof` makes and the same one `kiln_c99_is_namespace` already draws. */
    case KILN_VALUE_BUILTIN: return kiln_c99_is_namespace(value.bits) ? "object" : "function";
    default: return "object";
    }
}

/* "a number is not a constructor", and the same sentence for `function`.
 *
 * Built here rather than spelled at each throw site so the subject is never dropped: "is not a
 * function" on its own names nothing a program can act on, and does not say what the interpreter
 * says about the same fault. */
/* "Cannot redefine property: x" and its neighbours: a fixed sentence with the property's name on
 * the end. The name is not NUL-terminated where it is stored, which is the only reason this is a
 * function rather than a `kiln_c99_throw_error` call at each site. */
static int kiln_c99_throw_named(kiln_value *thrown, uint64_t kind, const char *sentence,
                                const char *key, size_t key_len) {
    static const char terminator[1] = {0};
    kiln_text built = {NULL, 0, 0};
    int status;
    if (kiln_c99_text_append_c(&built, sentence) != KILN_C99_OK ||
        kiln_c99_text_append(&built, key, key_len) != KILN_C99_OK ||
        kiln_c99_text_append(&built, terminator, 1) != KILN_C99_OK) {
        free(built.bytes);
        return KILN_C99_OUT_OF_MEMORY;
    }
    status = kiln_c99_throw_error(thrown, kind, built.bytes);
    free(built.bytes);
    return status;
}

static int kiln_c99_throw_not_a(kiln_value *thrown, kiln_value value, const char *role) {
    static const char terminator[1] = {0};
    kiln_text built = {NULL, 0, 0};
    int status;
    if (kiln_c99_text_append_c(&built, "a ") != KILN_C99_OK ||
        kiln_c99_text_append_c(&built, kiln_c99_kind_name(value)) != KILN_C99_OK ||
        kiln_c99_text_append_c(&built, " is not a ") != KILN_C99_OK ||
        kiln_c99_text_append_c(&built, role) != KILN_C99_OK ||
        kiln_c99_text_append(&built, terminator, 1) != KILN_C99_OK) {
        free(built.bytes);
        return KILN_C99_OUT_OF_MEMORY;
    }
    status = kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR, built.bytes);
    free(built.bytes);
    return status;
}

/* "5 is not iterable", and the same sentence for anything else a `for...of` or a spread was
 * pointed at.
 *
 * The subject is its *string* form rather than its kind, because that is the one the interpreter
 * prints and the two engines are compared byte for byte: "[object Object] is not iterable" and
 * "an object is not iterable" are the same fault reported two ways, and only one of them agrees. */
static int kiln_c99_throw_not_iterable(kiln_value *thrown, kiln_value value) {
    static const char terminator[1] = {0};
    kiln_value shown = kiln_c99_text(value);
    kiln_text built = {NULL, 0, 0};
    int status;
    if (shown.tag != KILN_VALUE_STRING) {
        /* A symbol has no string form to name it with, and inventing one would report a
         * different subject than the one the program wrote. */
        KILN_C99_REFUSE("a value with no string form to name in the message");
    }
    if (kiln_c99_text_append(&built, shown.ptr, shown.len) != KILN_C99_OK ||
        kiln_c99_text_append_c(&built, " is not iterable") != KILN_C99_OK ||
        kiln_c99_text_append(&built, terminator, 1) != KILN_C99_OK) {
        free(built.bytes);
        return KILN_C99_OUT_OF_MEMORY;
    }
    status = kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR, built.bytes);
    free(built.bytes);
    return status;
}

/* "cannot set a property on a number", and the same sentence for the other primitives.
 *
 * Only the primitives reach it. A Date or a Promise is an object the language lets a program
 * write to, so failing to store a property on one is this backend's gap rather than the
 * program's fault, and it stays a refusal even though the interpreter currently throws there --
 * agreeing with the interpreter about an answer node contradicts would be worse than saying so. */
static int kiln_c99_throw_cannot_set(kiln_value *thrown, kiln_value target) {
    static const char terminator[1] = {0};
    kiln_text built = {NULL, 0, 0};
    int status;
    if (kiln_c99_text_append_c(&built, "cannot set a property on a ") != KILN_C99_OK ||
        kiln_c99_text_append_c(&built, kiln_c99_kind_name(target)) != KILN_C99_OK ||
        kiln_c99_text_append(&built, terminator, 1) != KILN_C99_OK) {
        free(built.bytes);
        return KILN_C99_OUT_OF_MEMORY;
    }
    status = kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR, built.bytes);
    free(built.bytes);
    return status;
}

static int kiln_c99_throw_read_only(kiln_value *thrown, const char *key, size_t key_len) {
    static const char terminator[1] = {0};
    kiln_text built = {NULL, 0, 0};
    int status;
    if (kiln_c99_text_append_c(&built, "Cannot assign to read only property '") != KILN_C99_OK ||
        kiln_c99_text_append(&built, key, key_len) != KILN_C99_OK ||
        kiln_c99_text_append_c(&built, "' of object") != KILN_C99_OK ||
        kiln_c99_text_append(&built, terminator, 1) != KILN_C99_OK) {
        free(built.bytes);
        return KILN_C99_OUT_OF_MEMORY;
    }
    status = kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR, built.bytes);
    free(built.bytes);
    return status;
}

static int kiln_c99_throw_module_namespace(kiln_value *thrown, const char *prefix,
                                           const char *key, size_t key_len) {
    static const char terminator[1] = {0};
    kiln_text built = {NULL, 0, 0};
    int status;
    if (kiln_c99_text_append_c(&built, prefix) != KILN_C99_OK ||
        kiln_c99_text_append(&built, key, key_len) != KILN_C99_OK ||
        kiln_c99_text_append(&built, "'", 1) != KILN_C99_OK ||
        kiln_c99_text_append(&built, terminator, 1) != KILN_C99_OK) {
        free(built.bytes);
        return KILN_C99_OUT_OF_MEMORY;
    }
    status = kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR, built.bytes);
    free(built.bytes);
    return status;
}

static int kiln_c99_throw_module_namespace_write(kiln_value *thrown, const char *key,
                                                 size_t key_len) {
    return kiln_c99_throw_module_namespace(thrown,
                                            "Cannot assign to module namespace property '", key,
                                            key_len);
}

static int kiln_c99_throw_module_namespace_delete(kiln_value *thrown, const char *key,
                                                  size_t key_len) {
    return kiln_c99_throw_module_namespace(thrown,
                                            "Cannot delete module namespace property '", key,
                                            key_len);
}

static int kiln_c99_throw_module_namespace_define(kiln_value *thrown, const char *key,
                                                  size_t key_len) {
    return kiln_c99_throw_module_namespace(thrown,
                                            "Cannot redefine module namespace property '", key,
                                            key_len);
}

static int kiln_c99_throw_module_namespace_freeze(kiln_value *thrown) {
    return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                "Cannot freeze module namespace object");
}

static int kiln_c99_throw_cannot_delete(kiln_value *thrown, const char *key, size_t key_len) {
    static const char terminator[1] = {0};
    kiln_text built = {NULL, 0, 0};
    int status;
    if (kiln_c99_text_append_c(&built, "Cannot delete property '") != KILN_C99_OK ||
        kiln_c99_text_append(&built, key, key_len) != KILN_C99_OK ||
        kiln_c99_text_append_c(&built, "' of an object") != KILN_C99_OK ||
        kiln_c99_text_append(&built, terminator, 1) != KILN_C99_OK) {
        free(built.bytes);
        return KILN_C99_OUT_OF_MEMORY;
    }
    status = kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR, built.bytes);
    free(built.bytes);
    return status;
}

static int kiln_c99_throw_error(kiln_value *thrown, uint64_t kind, const char *message) {
    kiln_value built;
    int status;
    if (thrown == NULL) {
        KILN_C99_REFUSE("a throw with nowhere to put the thrown value");
    }
    status = kiln_c99_new_error(&built, kind, kiln_c99_copy_text(message, strlen(message)));
    if (status != KILN_C99_OK) {
        return status;
    }
    *thrown = built;
    return KILN_C99_THROWN;
}

/* A typed entry at the call-depth limit: the `RangeError` the generic call would throw, left
 * pending for the caller of the typed entry that stopped. */
static void kiln_c99_typed_overflow(void) {
    kiln_c99_typed_thrown = kiln_c99_undefined();
    kiln_c99_typed_status = kiln_c99_throw_error(&kiln_c99_typed_thrown, KILN_C99_BUILTIN_RANGE_ERROR,
                                                "Maximum call stack size exceeded");
}

static int kiln_c99_throw_node_invalid_arg_type(kiln_value *thrown, const char *message) {
    int status = kiln_c99_new_error(
        thrown, KILN_C99_BUILTIN_TYPE_ERROR, kiln_c99_copy_text(message, strlen(message)));
    if (status == KILN_C99_OK) {
        status = kiln_c99_put(NULL, *thrown, "code", 4,
                              kiln_c99_copy_text("ERR_INVALID_ARG_TYPE", 20), 0, NULL);
    }
    if (status != KILN_C99_OK) return status;
    {
        kiln_object *object = kiln_c99_object_of(*thrown);
        object->properties[object->count - 1].enumerable = 0;
    }
    return KILN_C99_THROWN;
}
