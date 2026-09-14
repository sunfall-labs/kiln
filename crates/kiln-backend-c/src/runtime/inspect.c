

/* The quote is whichever the string does not already contain, so the common case needs no
 * escaping at all: "it's" rather than 'it\'s'.  Single, then double, then backtick, which is
 * node's order of preference.  Bytes rather than characters is safe here: every byte this
 * inspects is ASCII, and a UTF-8 continuation byte can never equal one. */
static int kiln_c99_quote(kiln_text *text, const char *bytes, size_t len) {
    char quote = '\'';
    size_t index;
    int has_single = 0;
    int has_double = 0;
    int has_backtick = 0;
    int has_template = 0;
    for (index = 0; index < len; ++index) {
        if (bytes[index] == '\'') has_single = 1;
        if (bytes[index] == '"') has_double = 1;
        if (bytes[index] == '`') has_backtick = 1;
        if (bytes[index] == '$' && index + 1 < len && bytes[index + 1] == '{') has_template = 1;
    }
    if (has_single) {
        if (!has_double) {
            quote = '"';
        } else if (!has_backtick && !has_template) {
            quote = '`';
        }
    }
    if (kiln_c99_text_append(text, &quote, 1) != KILN_C99_OK) {
        return KILN_C99_OUT_OF_MEMORY;
    }
    for (index = 0; index < len; ++index) {
        char byte = bytes[index];
        const char *escape = NULL;
        char pair[2];
        char hex[4];
        static const char digits[] = "0123456789ABCDEF";
        if (byte == quote || byte == '\\') {
            pair[0] = '\\';
            pair[1] = byte;
            if (kiln_c99_text_append(text, pair, 2) != KILN_C99_OK) {
                return KILN_C99_OUT_OF_MEMORY;
            }
            continue;
        }
        switch (byte) {
        case '\n': escape = "\\n"; break;
        case '\r': escape = "\\r"; break;
        case '\t': escape = "\\t"; break;
        case '\b': escape = "\\b"; break;
        case '\f': escape = "\\f"; break;
        default: break;
        }
        if (escape != NULL) {
            if (kiln_c99_text_append_c(text, escape) != KILN_C99_OK) {
                return KILN_C99_OUT_OF_MEMORY;
            }
            continue;
        }
        if ((unsigned char)byte < 0x20) {
            hex[0] = '\\';
            hex[1] = 'x';
            hex[2] = digits[((unsigned char)byte >> 4) & 0xF];
            hex[3] = digits[(unsigned char)byte & 0xF];
            if (kiln_c99_text_append(text, hex, 4) != KILN_C99_OK) {
                return KILN_C99_OUT_OF_MEMORY;
            }
            continue;
        }
        if (kiln_c99_text_append(text, &byte, 1) != KILN_C99_OK) {
            return KILN_C99_OUT_OF_MEMORY;
        }
    }
    return kiln_c99_text_append(text, &quote, 1);
}

static int kiln_c99_inspect_key(kiln_text *text, const char *key, size_t len) {
    size_t index;
    int identifier = len != 0 && (isalpha((unsigned char)key[0]) || key[0] == '_');
    for (index = 1; identifier && index < len; ++index) {
        identifier = isalnum((unsigned char)key[index]) || key[index] == '_';
    }
    if (identifier) {
        return kiln_c99_text_append(text, key, len);
    }
    return kiln_c99_quote(text, key, len);
}

static int kiln_c99_inspect_into(kiln_text *text, kiln_value value, int depth);

static int kiln_c99_inspect_members(kiln_text *text, kiln_value value, int depth) {
    size_t index;
    if (value.tag == KILN_VALUE_ARRAY) {
        kiln_array *array = kiln_c99_array_of(value);
        kiln_object *extra = array->properties.tag == KILN_VALUE_OBJECT
                                 ? kiln_c99_object_of(array->properties)
                                 : NULL;
        if (extra != NULL && extra->shape != NULL) {
            kiln_c99_ensure_dictionary(extra);
        }
        if (array->count == 0 && (extra == NULL || extra->count == 0)) {
            return kiln_c99_text_append_c(text, "[]");
        }
        if (kiln_c99_text_append_c(text, "[ ") != KILN_C99_OK) {
            return KILN_C99_OUT_OF_MEMORY;
        }
        for (index = 0; index < array->count; ++index) {
            if (index != 0 && kiln_c99_text_append_c(text, ", ") != KILN_C99_OK) {
                return KILN_C99_OUT_OF_MEMORY;
            }
            if (array->elements[index] == KILN_VAL_HOLE) {
                /* A run of holes is reported as a count rather than repeated, which is what
                 * makes a sparse array readable at all. */
                size_t run = 1;
                while (index + run < array->count &&
                       array->elements[index + run] == KILN_VAL_HOLE) {
                    run += 1;
                }
                if (kiln_c99_text_append_c(text, "<") != KILN_C99_OK ||
                    kiln_c99_pad_number(text, (int64_t)run, 1) != KILN_C99_OK ||
                    kiln_c99_text_append_c(text, run == 1 ? " empty item>" : " empty items>") !=
                        KILN_C99_OK) {
                    return KILN_C99_OUT_OF_MEMORY;
                }
                index += run - 1;
                continue;
            }
            if (kiln_c99_inspect_into(text, kiln_val_to_boundary(array->elements[index]), depth + 1) != KILN_C99_OK) {
                return KILN_C99_OUT_OF_MEMORY;
            }
        }
        /* A property that is not an element is shown after the elements, which is where a console
         * puts the `index` and `input` a match result carries. */
        for (index = 0; extra != NULL && index < extra->count; ++index) {
            if (!extra->properties[index].enumerable) {
                continue;
            }
            if ((array->count != 0 || index != 0) &&
                kiln_c99_text_append_c(text, ", ") != KILN_C99_OK) {
                return KILN_C99_OUT_OF_MEMORY;
            }
            if (kiln_c99_inspect_key(text, extra->properties[index].key,
                                     extra->properties[index].key_len) != KILN_C99_OK ||
                kiln_c99_text_append_c(text, ": ") != KILN_C99_OK ||
                kiln_c99_inspect_into(text, extra->properties[index].value, depth + 1) !=
                    KILN_C99_OK) {
                return KILN_C99_OUT_OF_MEMORY;
            }
        }
        return kiln_c99_text_append_c(text, " ]");
    }
    {
        kiln_object *object = kiln_c99_object_of(value);
        size_t enumerated = 0;
        if (object->shape != NULL) {
            kiln_c99_ensure_dictionary(object);
        }
        /* A class instance shows the name of the class that made it, which is how a console
         * distinguishes two objects of the same shape. A plain object's constructor is `Object`,
         * and naming that would be noise rather than information. */
        if (object->prototype.tag == KILN_VALUE_OBJECT) {
            kiln_object *table = kiln_c99_object_of(object->prototype);
            size_t slot;
            if (table->shape != NULL) {
                kiln_c99_ensure_dictionary(table);
            }
            for (slot = 0; slot < table->count; ++slot) {
                kiln_value maker = table->properties[slot].value;
                if (!kiln_c99_same_key(&table->properties[slot], "constructor", 11) ||
                    maker.tag != KILN_VALUE_FUNCTION) {
                    continue;
                }
                {
                    kiln_closure *closure = kiln_c99_closure_of(maker);
                    if (closure->name == NULL || closure->name_len == 0 ||
                        kiln_c99_same_name(closure->name, closure->name_len, "Object")) {
                        break;
                    }
                    if (kiln_c99_text_append(text, closure->name, closure->name_len) !=
                            KILN_C99_OK ||
                        kiln_c99_text_append_c(text, " ") != KILN_C99_OK) {
                        return KILN_C99_OUT_OF_MEMORY;
                    }
                }
                break;
            }
        }
        for (index = 0; index < object->count; ++index) {
            enumerated += (size_t)(object->properties[index].enumerable != 0);
        }
        if (enumerated == 0) {
            return kiln_c99_text_append_c(text, "{}");
        }
        if (kiln_c99_text_append_c(text, "{ ") != KILN_C99_OK) {
            return KILN_C99_OUT_OF_MEMORY;
        }
        enumerated = 0;
        for (index = 0; index < object->count; ++index) {
            if (!object->properties[index].enumerable) {
                continue;
            }
            if (enumerated++ != 0 && kiln_c99_text_append_c(text, ", ") != KILN_C99_OK) {
                return KILN_C99_OUT_OF_MEMORY;
            }
            if (kiln_c99_inspect_key(text, object->properties[index].key,
                                     object->properties[index].key_len) != KILN_C99_OK) {
                return KILN_C99_OUT_OF_MEMORY;
            }
            if (kiln_c99_text_append_c(text, ": ") != KILN_C99_OK) {
                return KILN_C99_OUT_OF_MEMORY;
            }
            if (object->properties[index].accessor) {
                /* Named rather than called: showing a value here would run the program's code
                 * because something was printed. */
                const char *shown =
                    object->properties[index].getter.tag == KILN_VALUE_UNDEFINED  ? "[Setter]"
                    : object->properties[index].setter.tag == KILN_VALUE_UNDEFINED ? "[Getter]"
                                                                                   : "[Getter/Setter]";
                if (kiln_c99_text_append_c(text, shown) != KILN_C99_OK) {
                    return KILN_C99_OUT_OF_MEMORY;
                }
                continue;
            }
            if (kiln_c99_inspect_into(text, object->properties[index].value, depth + 1) !=
                KILN_C99_OK) {
                return KILN_C99_OUT_OF_MEMORY;
            }
        }
        return kiln_c99_text_append_c(text, " }");
    }
}

static int kiln_c99_inspect_into(kiln_text *text, kiln_value value, int depth) {
    /* A console shows what a proxy stands in front of rather than running its traps, which is
     * both what node does by default and the only thing this can do without a host to call. */
    value = kiln_c99_proxy_inner(value);
    switch (value.tag) {
    case KILN_VALUE_STRING: {
        uint8_t *encoded = NULL;
        size_t encoded_len = 0;
        int status = kiln_c99_string_to_utf8_lossy(value, &encoded, &encoded_len);
        if (status != KILN_C99_OK) return status;
        status = kiln_c99_quote(text, (const char *)encoded, encoded_len);
        free(encoded);
        return status;
    }
    case KILN_VALUE_NUMBER: {
        /* `-0` prints as itself here, where `String(-0)` is "0": a console shows the value
         * rather than converting it. */
        double number = kiln_c99_as_number(value);
        if (number == 0.0 && kiln_c99_signbit(number)) {
            return kiln_c99_text_append_c(text, "-0");
        }
        break;
    }
    case KILN_VALUE_BIGINT: {
        /* The `n` belongs to the console, not to the value: `String(1n)` is "1" and
         * `console.log(1n)` is `1n`, and the suffix is the only difference between them. */
        kiln_value digits = kiln_c99_bigint_text(value, 10);
        if (digits.tag != KILN_VALUE_STRING ||
            kiln_c99_text_append_string(text, digits) != KILN_C99_OK) {
            return KILN_C99_OUT_OF_MEMORY;
        }
        return kiln_c99_text_append_c(text, "n");
    }
    case KILN_VALUE_FUNCTION: {
        kiln_closure *closure = kiln_c99_closure_of(value);
        if (closure->name == NULL) {
            return kiln_c99_text_append_c(text, "[Function (anonymous)]");
        }
        if (kiln_c99_text_append_c(text, "[Function: ") != KILN_C99_OK ||
            kiln_c99_text_append(text, closure->name, closure->name_len) != KILN_C99_OK) {
            return KILN_C99_OUT_OF_MEMORY;
        }
        return kiln_c99_text_append_c(text, "]");
    }
    case KILN_VALUE_DATE: {
        /* A console shows the ISO form, which is what the interpreter shows. */
        kiln_value iso;
        if (kiln_c99_iso_string(&iso, kiln_c99_date_time(value)) != KILN_C99_OK) {
            return KILN_C99_OUT_OF_MEMORY;
        }
        if (iso.tag != KILN_VALUE_STRING) {
            return kiln_c99_text_append_c(text, "Invalid Date");
        }
        return kiln_c99_text_append_string(text, iso);
    }
    case KILN_VALUE_COLLECTION: {
        /* A console shows what a collection holds rather than the bookkeeping that makes it
         * work, with the count in front because a Map's contents do not otherwise say how many
         * there are. */
        kiln_collection *collection = kiln_c99_collection_of(value);
        size_t index;
        /* A weak collection shows neither its size nor its contents. It cannot: an entry is
         * reachable only through a key the console does not hold, and printing a count would
         * make the collector's timing observable from the program. `Map(0) {}` was the wrong
         * label twice over -- it named a type the value is not, so a log line said `Map` about
         * something `x instanceof Map` is false of. */
        if (collection->is_weak) {
            return kiln_c99_text_append_c(text, collection->is_map
                                                    ? "WeakMap { <items unknown> }"
                                                    : "WeakSet { <items unknown> }");
        }
        if (kiln_c99_text_append_c(text, collection->is_map ? "Map(" : "Set(") != KILN_C99_OK ||
            kiln_c99_pad_number(text, (int64_t)collection->live, 1) != KILN_C99_OK ||
            kiln_c99_text_append_c(text, collection->live == 0 ? ") {}" : ") { ") !=
                KILN_C99_OK) {
            return KILN_C99_OUT_OF_MEMORY;
        }
        if (collection->live == 0) {
            return KILN_C99_OK;
        }
        {
        size_t shown = 0;
        for (index = 0; index < collection->count; ++index) {
            if (collection->entries[index].dead) {
                continue;
            }
            if (shown++ != 0 && kiln_c99_text_append_c(text, ", ") != KILN_C99_OK) {
                return KILN_C99_OUT_OF_MEMORY;
            }
            if (collection->is_map) {
                if (kiln_c99_inspect_into(text, collection->entries[index].key, depth + 1) !=
                        KILN_C99_OK ||
                    kiln_c99_text_append_c(text, " => ") != KILN_C99_OK) {
                    return KILN_C99_OUT_OF_MEMORY;
                }
            }
            if (kiln_c99_inspect_into(text, collection->entries[index].value, depth + 1) !=
                KILN_C99_OK) {
                return KILN_C99_OUT_OF_MEMORY;
            }
        }
        }
        return kiln_c99_text_append_c(text, " }");
    }
    case KILN_VALUE_PROMISE: {
        kiln_promise *promise = kiln_c99_promise_of(value);
        if (promise->state == KILN_C99_PENDING) {
            return kiln_c99_text_append_c(text, "Promise { <pending> }");
        }
        if (kiln_c99_text_append_c(text, promise->state == KILN_C99_REJECTED
                                             ? "Promise { <rejected> "
                                             : "Promise { ") != KILN_C99_OK ||
            kiln_c99_inspect_into(text, promise->value, depth + 1) != KILN_C99_OK) {
            return KILN_C99_OUT_OF_MEMORY;
        }
        return kiln_c99_text_append_c(text, " }");
    }
    case KILN_VALUE_GENERATOR:
        return kiln_c99_text_append_c(text, "Object [Generator] {}");
    case KILN_VALUE_DATA_VIEW: {
        /* `DataView { byteLength: 8, byteOffset: 0 }`, which is what a console shows and what the
         * interpreter prints. Two numbers and no list: a DataView holds no elements to walk, and
         * the buffer behind it is deliberately not shown -- printing a view would then print the
         * bytes every other view of the same buffer can see. */
        kiln_data_view *view = kiln_c99_data_view_of(value);
        if (kiln_c99_text_append_c(text, "DataView { byteLength: ") != KILN_C99_OK ||
            kiln_c99_pad_number(text, (int64_t)view->byte_length, 1) != KILN_C99_OK ||
            kiln_c99_text_append_c(text, ", byteOffset: ") != KILN_C99_OK ||
            kiln_c99_pad_number(text, (int64_t)view->byte_offset, 1) != KILN_C99_OK ||
            kiln_c99_text_append_c(text, " }") != KILN_C99_OK) {
            return KILN_C99_OUT_OF_MEMORY;
        }
        return KILN_C99_OK;
    }
    case KILN_VALUE_TYPED_ARRAY: {
        /* `Uint8Array(4) [ 1, 255, 0, 255 ]`. The kind and the count go in front because a view
         * is the one container whose contents do not say how wide its elements are, and reading
         * `[ 1, 255, 0, 255 ]` alone would leave that unanswerable.
         *
         * The elements go back through `inspect` rather than through `String`: `-0` prints as
         * itself in a console and as "0" everywhere else, and a `Float64Array` can hold one. */
        kiln_typed_array *view = kiln_c99_typed_array_of(value);
        size_t at;
        if (kiln_c99_text_append_c(text, kiln_c99_typed_name(view->kind)) != KILN_C99_OK ||
            kiln_c99_text_append_c(text, "(") != KILN_C99_OK ||
            kiln_c99_pad_number(text, (int64_t)view->length, 1) != KILN_C99_OK ||
            kiln_c99_text_append_c(text, view->length == 0 ? ") []" : ") [ ") != KILN_C99_OK) {
            return KILN_C99_OUT_OF_MEMORY;
        }
        if (view->length == 0) {
            return KILN_C99_OK;
        }
        for (at = 0; at < view->length; ++at) {
            if (at != 0 && kiln_c99_text_append_c(text, ", ") != KILN_C99_OK) {
                return KILN_C99_OUT_OF_MEMORY;
            }
            if (kiln_c99_inspect_into(text, kiln_c99_number(kiln_c99_typed_get(view, at)),
                                      depth + 1) != KILN_C99_OK) {
                return KILN_C99_OUT_OF_MEMORY;
            }
        }
        return kiln_c99_text_append_c(text, " ]");
    }
    case KILN_VALUE_HOLE:
        return kiln_c99_text_append_c(text, "undefined");
    case KILN_VALUE_BUILTIN: {
        const char *name = kiln_c99_builtin_name(value.bits);
        if (kiln_c99_is_namespace(value.bits)) {
            /* `Object [Math] {}`: a namespace shows as the object it is, with no members --
             * node lists none of the built-in ones either. */
            if (kiln_c99_text_append_c(text, "Object [") != KILN_C99_OK ||
                kiln_c99_text_append_c(text, name) != KILN_C99_OK) {
                return KILN_C99_OUT_OF_MEMORY;
            }
            return kiln_c99_text_append_c(text, "] {}");
        }
        if (kiln_c99_text_append_c(text, "[Function: ") != KILN_C99_OK ||
            kiln_c99_text_append_c(text, name) != KILN_C99_OK) {
            return KILN_C99_OUT_OF_MEMORY;
        }
        return kiln_c99_text_append_c(text, "]");
    }
    case KILN_VALUE_METHOD: {
        const char *name = kiln_c99_method_name(value.bits);
        if (name == NULL) {
            return kiln_c99_text_append_c(text, "[Function (anonymous)]");
        }
        if (kiln_c99_text_append_c(text, "[Function: ") != KILN_C99_OK ||
            kiln_c99_text_append_c(text, name) != KILN_C99_OK) {
            return KILN_C99_OUT_OF_MEMORY;
        }
        return kiln_c99_text_append_c(text, "]");
    }
    case KILN_VALUE_OBJECT:
        if (depth > KILN_C99_INSPECT_DEPTH) {
            return kiln_c99_text_append_c(text, "[Object]");
        }
        return kiln_c99_inspect_members(text, value, depth);
    case KILN_VALUE_ARRAY:
        if (depth > KILN_C99_INSPECT_DEPTH) {
            return kiln_c99_text_append_c(text, "[Array]");
        }
        return kiln_c99_inspect_members(text, value, depth);
    default:
        break;
    }
    {
        kiln_value shown = kiln_c99_text(value);
        if (shown.tag != KILN_VALUE_STRING) {
            KILN_C99_REFUSE("a value with no text form to inspect");
        }
        return kiln_c99_text_append_string(text, shown);
    }
}

/* A top-level string prints as itself; only a nested one is quoted. */
static kiln_value kiln_c99_inspect(kiln_value value) {
    kiln_text text = {NULL, 0, 0};
    if (value.tag == KILN_VALUE_STRING) {
        return value;
    }
    if (kiln_c99_inspect_into(&text, value, 0) != KILN_C99_OK) {
        free(text.bytes);
        return kiln_c99_undefined();
    }
    return kiln_c99_text_value(&text);
}
