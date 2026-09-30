
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
} kiln_dyn_binding;

typedef struct kiln_dyn_scope {
    kiln_dyn_binding *bindings;
    size_t count;
    size_t capacity;
    struct kiln_dyn_scope *parent;
    kiln_value with_object;
    int has_with_object;
} kiln_dyn_scope;

static void kiln_dyn_scope_init(kiln_dyn_scope *scope, kiln_dyn_scope *parent) {
    scope->bindings = NULL;
    scope->count = 0;
    scope->capacity = 0;
    scope->parent = parent;
    scope->with_object = kiln_c99_undefined();
    scope->has_with_object = 0;
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
    char *dup = (char *)malloc(name_len + 1);
    if (dup == NULL) return 0;
    memcpy(dup, name, name_len);
    dup[name_len] = '\0';
    scope->bindings[scope->count].name = dup;
    scope->bindings[scope->count].name_len = name_len;
    scope->bindings[scope->count].value = val;
    scope->count++;
    return 1;
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
                    curr->bindings[i].value = val;
                }
                return 1;
            }
        }
        curr = curr->parent;
    }
    return 0;
}

static int kiln_dyn_scope_get(kiln_dyn_scope *scope, const char *name, size_t name_len, kiln_value *out) {
    kiln_dyn_scope *curr = scope;
    while (curr != NULL) {
        size_t i;
        for (i = 0; i < curr->count; ++i) {
            if (curr->bindings[i].name_len == name_len &&
                memcmp(curr->bindings[i].name, name, name_len) == 0) {
                *out = curr->bindings[i].value.tag == KILN_VALUE_CELL
                           ? kiln_val_to_boundary(*kiln_c99_cell_of(curr->bindings[i].value))
                           : curr->bindings[i].value;
                return 1;
            }
        }
        curr = curr->parent;
    }
    return 0;
}

static int kiln_dyn_scope_get_with(kiln_host *host, kiln_dyn_scope *scope, const char *name,
                                   size_t name_len, int depth, kiln_value *out, int *found,
                                   kiln_value *thrown) {
    kiln_dyn_scope *curr = scope;
    *found = 0;
    while (curr != NULL) {
        if (curr->has_with_object) {
            int present = 0;
            int status = kiln_c99_has_property(host, curr->with_object, name, name_len,
                                               &present, depth, thrown);
            if (status != KILN_C99_OK) return status;
            if (present) {
                status = kiln_c99_get(host, curr->with_object, name, name_len, out, depth,
                                      thrown);
                if (status == KILN_C99_OK) *found = 1;
                return status;
            }
        }
        curr = curr->parent;
    }
    return KILN_C99_OK;
}

static int kiln_dyn_scope_set_with(kiln_host *host, kiln_dyn_scope *scope, const char *name,
                                   size_t name_len, kiln_value value, int depth, int *found,
                                   kiln_value *thrown) {
    kiln_dyn_scope *curr = scope;
    *found = 0;
    while (curr != NULL) {
        if (curr->has_with_object) {
            int present = 0;
            int status = kiln_c99_has_property(host, curr->with_object, name, name_len,
                                               &present, depth, thrown);
            if (status != KILN_C99_OK) return status;
            if (present) {
                *found = 1;
                status = kiln_c99_put(host, curr->with_object, name, name_len, value, 0,
                                      thrown);
                return status;
            }
        }
        curr = curr->parent;
    }
    return KILN_C99_OK;
}

static void kiln_dyn_scope_destroy(kiln_dyn_scope *scope) {
    size_t i;
    for (i = 0; i < scope->count; ++i) {
        free(scope->bindings[i].name);
    }
    free(scope->bindings);
}

typedef enum {
    KILN_DYN_EOF,
    KILN_DYN_NUM,
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
    KILN_DYN_KW_FUNCTION,
    KILN_DYN_KW_NEW,
    KILN_DYN_KW_FOR,
    KILN_DYN_KW_IN,
    KILN_DYN_KW_OF,
    KILN_DYN_KW_BREAK,
    KILN_DYN_KW_CONTINUE,
    KILN_DYN_KW_WITH,
    KILN_DYN_KW_ASYNC,
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
} kiln_dyn_token;

typedef struct {
    const char *src;
    size_t len;
    size_t pos;
    kiln_dyn_token curr;
    kiln_dyn_token_kind prev_kind;
    const char *active_label;
    size_t active_label_len;
    const char *break_label;
    size_t break_label_len;
} kiln_dyn_lexer;

static void kiln_dyn_token_free(kiln_dyn_token *tok) {
    if ((tok->kind == KILN_DYN_STR || tok->kind == KILN_DYN_REGEX) && tok->str_val != NULL) {
        free(tok->str_val);
        tok->str_val = NULL;
    }
}

static int kiln_dyn_is_ident_name(kiln_dyn_token_kind kind) {
    return kind == KILN_DYN_IDENT || (kind >= KILN_DYN_KW_RETURN && kind <= KILN_DYN_KW_ASYNC);
}

static void kiln_dyn_lexer_next(kiln_dyn_lexer *lex) {
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

    if ((c >= '0' && c <= '9') || (c == '.' && lex->pos + 1 < lex->len && (lex->src[lex->pos + 1] >= '0' && lex->src[lex->pos + 1] <= '9'))) {
        size_t num_start = lex->pos;
        int has_dot = (c == '.');
        lex->pos++;
        while (lex->pos < lex->len) {
            char nc = lex->src[lex->pos];
            if (nc >= '0' && nc <= '9') {
                lex->pos++;
            } else if (nc == '.' && !has_dot) {
                has_dot = 1;
                lex->pos++;
            } else if ((nc == 'e' || nc == 'E') && lex->pos + 1 < lex->len) {
                lex->pos++;
                if (lex->src[lex->pos] == '+' || lex->src[lex->pos] == '-') {
                    lex->pos++;
                }
            } else {
                break;
            }
        }
        size_t num_len = lex->pos - num_start;
        double val = kiln_string_to_number(start, num_len);
        lex->curr.kind = KILN_DYN_NUM;
        lex->curr.start = start;
        lex->curr.len = num_len;
        lex->curr.num_val = val;
        return;
    }

    if (c == '\'' || c == '"' || c == '`') {
        char quote = c;
        lex->pos++;
        size_t str_start = lex->pos;
        size_t cap = 16;
        size_t out_len = 0;
        char *buf = (char *)malloc(cap);
        while (lex->pos < lex->len && lex->src[lex->pos] != quote) {
            char sc = lex->src[lex->pos];
            if (sc == '\\' && lex->pos + 1 < lex->len) {
                lex->pos++;
                sc = lex->src[lex->pos];
                if (sc == 'n') sc = '\n';
                else if (sc == 'r') sc = '\r';
                else if (sc == 't') sc = '\t';
            }
            if (out_len + 2 > cap) {
                cap *= 2;
                buf = (char *)realloc(buf, cap);
            }
            buf[out_len++] = sc;
            lex->pos++;
        }
        if (lex->pos < lex->len && lex->src[lex->pos] == quote) {
            lex->pos++;
        }
        buf[out_len] = '\0';
        lex->curr.kind = KILN_DYN_STR;
        lex->curr.start = start;
        lex->curr.len = lex->pos - (str_start - 1);
        lex->curr.str_val = buf;
        lex->curr.str_len = out_len;
        return;
    }

    if (c == '/') {
        int can_be_regex = (lex->prev_kind == KILN_DYN_EOF ||
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
                            lex->prev_kind == KILN_DYN_KW_IN ||
                            lex->prev_kind == KILN_DYN_KW_TYPEOF);
        if (can_be_regex) {
            size_t reg_start = lex->pos;
            lex->pos++;
            size_t pat_start = lex->pos;
            int in_class = 0;
            while (lex->pos < lex->len) {
                char rc = lex->src[lex->pos];
                if (rc == '\\' && lex->pos + 1 < lex->len) {
                    lex->pos += 2;
                    continue;
                }
                if (rc == '[') in_class = 1;
                else if (rc == ']') in_class = 0;
                else if (rc == '/' && !in_class) {
                    break;
                }
                lex->pos++;
            }
            size_t pat_len = lex->pos - pat_start;
            if (lex->pos < lex->len && lex->src[lex->pos] == '/') {
                lex->pos++;
            }
            size_t flags_start = lex->pos;
            while (lex->pos < lex->len && ((lex->src[lex->pos] >= 'a' && lex->src[lex->pos] <= 'z') ||
                                           (lex->src[lex->pos] >= 'A' && lex->src[lex->pos] <= 'Z'))) {
                lex->pos++;
            }
            size_t flags_len = lex->pos - flags_start;
            char *buf = (char *)malloc(pat_len + 1 + flags_len + 1);
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
        KILN_CHECK_KW("function", KILN_DYN_KW_FUNCTION);
        KILN_CHECK_KW("new", KILN_DYN_KW_NEW);
        KILN_CHECK_KW("for", KILN_DYN_KW_FOR);
        KILN_CHECK_KW("in", KILN_DYN_KW_IN);
        KILN_CHECK_KW("of", KILN_DYN_KW_OF);
        KILN_CHECK_KW("break", KILN_DYN_KW_BREAK);
        KILN_CHECK_KW("continue", KILN_DYN_KW_CONTINUE);
        KILN_CHECK_KW("with", KILN_DYN_KW_WITH);
        KILN_CHECK_KW("async", KILN_DYN_KW_ASYNC);
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
        if (lex->pos < lex->len && lex->src[lex->pos] == '=') {
            lex->pos++;
            lex->curr.len = 2;
            lex->curr.kind = KILN_DYN_LE;
            return;
        }
        lex->curr.kind = KILN_DYN_LT;
        return;
    case '>':
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
        lex->curr.kind = KILN_DYN_ERROR;
        return;
    case '|':
        if (lex->pos < lex->len && lex->src[lex->pos] == '|') {
            lex->pos++;
            lex->curr.len = 2;
            lex->curr.kind = KILN_DYN_OR;
            return;
        }
        lex->curr.kind = KILN_DYN_ERROR;
        return;
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
    case '(': lex->curr.kind = KILN_DYN_LPAREN; return;
    case ')': lex->curr.kind = KILN_DYN_RPAREN; return;
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
static int kiln_dyn_make_closure(kiln_host *host, kiln_dyn_scope *scope,
                                 const char *params, size_t params_len,
                                 const char *body, size_t body_len, kiln_value receiver,
                                 int asynchronous, int arrow, kiln_value *out,
                                 kiln_value *thrown);
static int kiln_dyn_capture_block(kiln_dyn_lexer *lex, const char **body, size_t *body_len);
static int kiln_dyn_capture_expression(kiln_dyn_lexer *lex, const char **body,
                                       size_t *body_len);
static int kiln_dyn_make_arrow_from_tail(kiln_host *host, kiln_dyn_lexer *lex,
                                         kiln_dyn_scope *scope, const char *params,
                                         size_t params_len, kiln_value receiver, int asynchronous,
                                         int depth, kiln_value *out, kiln_value *thrown);
static int kiln_dyn_parenthesized_arrow_params(kiln_dyn_lexer *lex, const char **params,
                                               size_t *params_len, size_t *after_close);

static int kiln_c99_dynamic_body_trampoline(kiln_call_context *ctx, kiln_val *result, kiln_val *upvalues,
                                            const kiln_val *receiver_ptr, const kiln_val *args, size_t argc);

static int kiln_dyn_eval_primary(kiln_host *host, kiln_dyn_lexer *lex, kiln_dyn_scope *scope,
                                kiln_value receiver, const kiln_value *args, size_t argc,
                                int depth, kiln_value *out, kiln_value *thrown) {
    if (lex->curr.kind == KILN_DYN_NUM) {
        *out = kiln_c99_number(lex->curr.num_val);
    kiln_dyn_lexer_next(lex);
        return KILN_C99_OK;
    }
    if (lex->curr.kind == KILN_DYN_STR) {
        *out = kiln_c99_copy_text(lex->curr.str_val, lex->curr.str_len);
        kiln_dyn_lexer_next(lex);
        return KILN_C99_OK;
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
        kiln_value source = kiln_c99_copy_text(pat, pat_len);
        kiln_value flag_value = kiln_c99_copy_text(flags, flags_len);
        int reg_status;
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
        if (lex->curr.kind == KILN_DYN_KW_FUNCTION) {
            asynchronous = 1;
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
                    return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_SYNTAX_ERROR,
                                                "Unterminated arrow function body");
                }
                return kiln_dyn_make_closure(host, scope, name, name_len, body, body_len,
                                             receiver, 0, 1, out, thrown);
            }
            if (!kiln_dyn_capture_expression(lex, &body, &body_len))
                return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_SYNTAX_ERROR,
                                            "Expected arrow function body");
            size_t return_len = body_len + sizeof "return ;";
            char *return_body = (char *)malloc(return_len);
            if (return_body == NULL) return KILN_C99_OUT_OF_MEMORY;
            memcpy(return_body, "return ", 7);
            memcpy(return_body + 7, body, body_len);
            return_body[7 + body_len] = ';';
            int closure_status = kiln_dyn_make_closure(host, scope, name, name_len,
                                                       return_body, body_len + 8,
                                                       receiver, 0, 1, out, thrown);
            free(return_body);
            return closure_status;
        }
        if (kiln_dyn_scope_get(scope, name, name_len, out)) {
            return KILN_C99_OK;
        }
        int with_found = 0;
        int with_status = kiln_dyn_scope_get_with(host, scope, name, name_len, depth, out,
                                                  &with_found, thrown);
        if (with_status != KILN_C99_OK) return with_status;
        if (with_found) return KILN_C99_OK;
        kiln_value globals = kiln_c99_undefined();
        int status = kiln_c99_global_object(&globals);
        if (status == KILN_C99_OK) {
            status = kiln_c99_get(host, globals, name, name_len, out, depth, thrown);
            if (status == KILN_C99_OK) {
                return KILN_C99_OK;
            }
        }
        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_REFERENCE_ERROR, "Variable is not defined");
    }
    if (lex->curr.kind == KILN_DYN_LPAREN) {
        const char *params;
        size_t params_len, after_close;
        if (kiln_dyn_parenthesized_arrow_params(lex, &params, &params_len, &after_close)) {
            lex->pos = after_close;
            kiln_dyn_lexer_next(lex);
            if (lex->curr.kind != KILN_DYN_ARROW)
                return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_SYNTAX_ERROR,
                                            "Expected arrow token");
            kiln_dyn_lexer_next(lex);
            return kiln_dyn_make_arrow_from_tail(host, lex, scope, params, params_len,
                                                 receiver, 0, depth, out, thrown);
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
        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_SYNTAX_ERROR, "Expected ')'");
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
        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_SYNTAX_ERROR, "Expected ']'");
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
            size_t key_len = 0;
            if (kiln_dyn_is_ident_name(lex->curr.kind) || lex->curr.kind == KILN_DYN_STR) {
                key_name = lex->curr.kind == KILN_DYN_STR ? lex->curr.str_val : lex->curr.start;
                key_len = lex->curr.kind == KILN_DYN_STR ? lex->curr.str_len : lex->curr.len;
                kiln_dyn_lexer_next(lex);
            } else {
                return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_SYNTAX_ERROR, "Expected property key");
            }
            kiln_value val;
            if (lex->curr.kind == KILN_DYN_COLON) {
                kiln_dyn_lexer_next(lex);
                status = kiln_dyn_eval_expr(host, lex, scope, receiver, args, argc, depth, &val, thrown);
                if (status != KILN_C99_OK) return status;
            } else {
                if (!kiln_dyn_scope_get(scope, key_name, key_len, &val)) {
                    int with_found = 0;
                    status = kiln_dyn_scope_get_with(host, scope, key_name, key_len, depth, &val,
                                                     &with_found, thrown);
                    if (status != KILN_C99_OK) return status;
                    if (!with_found) {
                        kiln_value globals = kiln_c99_undefined();
                        int st = kiln_c99_global_object(&globals);
                        if (st != KILN_C99_OK || kiln_c99_get(host, globals, key_name, key_len,
                                                            &val, depth, thrown) != KILN_C99_OK) {
                            val = kiln_c99_undefined();
                        }
                    }
                }
            }
            status = kiln_c99_put(host, *out, key_name, key_len, val, 0, thrown);
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
        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_SYNTAX_ERROR, "Expected '}'");
    }

    if (lex->curr.kind == KILN_DYN_KW_FUNCTION) {
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
            return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_SYNTAX_ERROR, "Expected '(' after function");
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
            return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_SYNTAX_ERROR, "Expected '{' for function body");
        }
        size_t bstart = lex->pos;
        int bdepth = 1;
        while (bdepth > 0 && lex->pos < lex->len) {
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
            if (c == '{') bdepth++;
            else if (c == '}') bdepth--;
            lex->pos++;
        }
        size_t blen = (lex->pos > 0 && lex->pos > bstart) ? (lex->pos - 1 - bstart) : 0;
        kiln_dyn_lexer_next(lex);

        kiln_value upvals[3];
        kiln_value name_value = kiln_c99_undefined();
        const kiln_string *closure_name = NULL;
        if (explicit_name) {
            name_value = kiln_c99_copy_text(fn_name, fn_name_len);
            if (name_value.tag != KILN_VALUE_STRING) return KILN_C99_OUT_OF_MEMORY;
            closure_name = (const kiln_string *)name_value.ptr;
        }
        upvals[0] = kiln_c99_copy_text(&lex->src[pstart], plen);
        upvals[1] = kiln_c99_copy_text(&lex->src[bstart], blen);
        kiln_value env_obj;
        int status = kiln_c99_new_object(&env_obj);
        if (status != KILN_C99_OK) return status;
        kiln_dyn_scope *s = scope;
        while (s != NULL) {
            for (size_t i = 0; i < s->count; ++i) {
                if (s->bindings[i].value.tag != KILN_VALUE_CELL) {
                    status = kiln_c99_make_cell(&s->bindings[i].value);
                    if (status != KILN_C99_OK) return status;
                }
                kiln_c99_gc_keep_alive(&s->bindings[i].value);
                status = kiln_c99_put(host, env_obj, s->bindings[i].name,
                                      strlen(s->bindings[i].name), s->bindings[i].value, 0,
                                      thrown);
                if (status != KILN_C99_OK) return status;
            }
            s = s->parent;
        }
        upvals[2] = env_obj;
        kiln_val val_upvals[3] = {
            kiln_val_from_boundary(upvals[0]),
            kiln_val_from_boundary(upvals[1]),
            kiln_val_from_boundary(upvals[2])
        };

        status = kiln_c99_make_closure(out, kiln_c99_dynamic_body_trampoline,
                                       closure_name,
                                       "", 0,
                                       "eval", 0, 0, 0, asynchronous, 0, 0, 0, 0,
                                       KILN_VAL_UNDEFINED, KILN_VAL_UNDEFINED,
                                       0, 16, 0, 0, val_upvals, 3);
        if (status != KILN_C99_OK) return status;
        kiln_c99_heap_store_value(kiln_c99_closure_of(*out), &kiln_c99_closure_of(*out)->source_value, upvals[1]);
        if (explicit_name) {
            (void)kiln_dyn_scope_set(scope, fn_name, fn_name_len, *out);
            (void)kiln_c99_put(host, env_obj, fn_name, fn_name_len, *out, 0, NULL);
        }
        return KILN_C99_OK;
    }

    if (lex->curr.kind == KILN_DYN_KW_NEW) {
        kiln_dyn_lexer_next(lex);
        return kiln_dyn_eval_postfix(host, lex, scope, receiver, args, argc, depth, out,
                                     thrown, 1);
    }

    return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_SYNTAX_ERROR, "Unexpected token in expression");
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
                return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_SYNTAX_ERROR, "Expected identifier after '.'");
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
                return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_SYNTAX_ERROR, "Expected ']'");
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
                return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_SYNTAX_ERROR, "Expected ')'");
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

static int kiln_dyn_eval_unary(kiln_host *host, kiln_dyn_lexer *lex, kiln_dyn_scope *scope,
                               kiln_value receiver, const kiln_value *args, size_t argc,
                               int depth, kiln_value *out, kiln_value *thrown) {
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
    if (lex->curr.kind == KILN_DYN_KW_TYPEOF) {
        kiln_dyn_lexer_next(lex);
        if (lex->curr.kind == KILN_DYN_IDENT) {
            const char *name = lex->curr.start;
            size_t name_len = lex->curr.len;
            size_t saved_pos = lex->pos;
            kiln_dyn_token saved_tok = lex->curr;
            kiln_dyn_lexer_next(lex);
            if (lex->curr.kind != KILN_DYN_DOT && lex->curr.kind != KILN_DYN_LBRACKET && lex->curr.kind != KILN_DYN_LPAREN) {
                if (kiln_dyn_scope_get(scope, name, name_len, out)) {
                    *out = kiln_c99_typeof(*out);
                    return KILN_C99_OK;
                }
                int with_found = 0;
                int with_status = kiln_dyn_scope_get_with(host, scope, name, name_len, depth,
                                                          out, &with_found, thrown);
                if (with_status != KILN_C99_OK) return with_status;
                if (with_found) {
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

static int kiln_dyn_eval_relational(kiln_host *host, kiln_dyn_lexer *lex, kiln_dyn_scope *scope,
                                    kiln_value receiver, const kiln_value *args, size_t argc,
                                    int depth, kiln_value *out, kiln_value *thrown) {
    int status = kiln_dyn_eval_additive(host, lex, scope, receiver, args, argc, depth, out, thrown);
    if (status != KILN_C99_OK) return status;

    while (lex->curr.kind == KILN_DYN_LT || lex->curr.kind == KILN_DYN_LE ||
           lex->curr.kind == KILN_DYN_GT || lex->curr.kind == KILN_DYN_GE ||
           lex->curr.kind == KILN_DYN_KW_IN) {
        kiln_dyn_token_kind op = lex->curr.kind;
        kiln_dyn_lexer_next(lex);
        kiln_value right;
        status = kiln_dyn_eval_additive(host, lex, scope, receiver, args, argc, depth, &right, thrown);
        if (status != KILN_C99_OK) return status;
        if (op == KILN_DYN_KW_IN) {
            int answer = 0;
            status = kiln_c99_has_computed(host, right, *out, &answer, depth, thrown);
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
    while (lex->curr.kind != KILN_DYN_EOF) {
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

static int kiln_dyn_eval_logical_and(kiln_host *host, kiln_dyn_lexer *lex, kiln_dyn_scope *scope,
                                     kiln_value receiver, const kiln_value *args, size_t argc,
                                     int depth, kiln_value *out, kiln_value *thrown) {
    int status = kiln_dyn_eval_equality(host, lex, scope, receiver, args, argc, depth, out, thrown);
    if (status != KILN_C99_OK) return status;

    while (lex->curr.kind == KILN_DYN_AND) {
        kiln_dyn_lexer_next(lex);
        if (!kiln_c99_truthy(*out)) {
            kiln_dyn_skip_expr(lex, 0);
        } else {
            status = kiln_dyn_eval_equality(host, lex, scope, receiver, args, argc, depth, out, thrown);
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
            kiln_dyn_skip_expr(lex, 0);
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
            kiln_dyn_skip_expr(lex, 1);
            if (lex->curr.kind == KILN_DYN_COLON) {
                kiln_dyn_lexer_next(lex);
                status = kiln_dyn_eval_expr(host, lex, scope, receiver, args, argc, depth, out, thrown);
                if (status != KILN_C99_OK) return status;
            }
        }
    }
    return KILN_C99_OK;
}

static int kiln_dyn_eval_assignment(kiln_host *host, kiln_dyn_lexer *lex, kiln_dyn_scope *scope,
                                    kiln_value receiver, const kiln_value *args, size_t argc,
                                    int depth, kiln_value *out, kiln_value *thrown) {
    if (lex->curr.kind == KILN_DYN_IDENT) {
        const char *name = lex->curr.start;
        size_t name_len = lex->curr.len;
        size_t saved_pos = lex->pos;
        kiln_dyn_token saved_tok = lex->curr;

        kiln_dyn_lexer_next(lex);
        if (lex->curr.kind == KILN_DYN_PLUS_PLUS) {
            kiln_dyn_lexer_next(lex);
            kiln_value cur_val = kiln_c99_number(0);
            int found = kiln_dyn_scope_get(scope, name, name_len, &cur_val);
            if (!found) {
                int status = kiln_dyn_scope_get_with(host, scope, name, name_len, depth,
                                                     &cur_val, &found, thrown);
                if (status != KILN_C99_OK) return status;
            }
            double n = kiln_c99_as_number(cur_val);
            if (!kiln_dyn_scope_update(scope, name, name_len, kiln_c99_number(n + 1.0))) {
                int status = kiln_dyn_scope_set_with(host, scope, name, name_len,
                                                     kiln_c99_number(n + 1.0), depth, &found,
                                                     thrown);
                if (status != KILN_C99_OK) return status;
                if (!found) kiln_dyn_scope_set(scope, name, name_len, kiln_c99_number(n + 1.0));
            }
            *out = kiln_c99_number(n);
            return KILN_C99_OK;
        } else if (lex->curr.kind == KILN_DYN_MINUS_MINUS) {
            kiln_dyn_lexer_next(lex);
            kiln_value cur_val = kiln_c99_number(0);
            int found = kiln_dyn_scope_get(scope, name, name_len, &cur_val);
            if (!found) {
                int status = kiln_dyn_scope_get_with(host, scope, name, name_len, depth,
                                                     &cur_val, &found, thrown);
                if (status != KILN_C99_OK) return status;
            }
            double n = kiln_c99_as_number(cur_val);
            if (!kiln_dyn_scope_update(scope, name, name_len, kiln_c99_number(n - 1.0))) {
                int status = kiln_dyn_scope_set_with(host, scope, name, name_len,
                                                     kiln_c99_number(n - 1.0), depth, &found,
                                                     thrown);
                if (status != KILN_C99_OK) return status;
                if (!found) kiln_dyn_scope_set(scope, name, name_len, kiln_c99_number(n - 1.0));
            }
            *out = kiln_c99_number(n);
            return KILN_C99_OK;
        } else if (lex->curr.kind == KILN_DYN_ASSIGN ||
                   lex->curr.kind == KILN_DYN_PLUS_ASSIGN ||
                   lex->curr.kind == KILN_DYN_MINUS_ASSIGN ||
                   lex->curr.kind == KILN_DYN_STAR_ASSIGN ||
                   lex->curr.kind == KILN_DYN_SLASH_ASSIGN ||
                   lex->curr.kind == KILN_DYN_PERCENT_ASSIGN) {
            kiln_dyn_token_kind assignment = lex->curr.kind;
            kiln_dyn_lexer_next(lex);
            kiln_value rhs;
            int status = kiln_dyn_eval_expr(host, lex, scope, receiver, args, argc, depth, &rhs, thrown);
            if (status != KILN_C99_OK) return status;
            if (assignment != KILN_DYN_ASSIGN) {
                kiln_value current;
                if (!kiln_dyn_scope_get(scope, name, name_len, &current)) {
                    int with_found = 0;
                    status = kiln_dyn_scope_get_with(host, scope, name, name_len, depth,
                                                     &current, &with_found, thrown);
                    if (status != KILN_C99_OK) return status;
                    if (!with_found) {
                        kiln_value globals = kiln_c99_undefined();
                        status = kiln_c99_global_object(&globals);
                        if (status == KILN_C99_OK) {
                            status = kiln_c99_get(host, globals, name, name_len, &current, depth,
                                                  thrown);
                        }
                    }
                    if (status != KILN_C99_OK) return status;
                }
                switch (assignment) {
                case KILN_DYN_PLUS_ASSIGN: rhs = kiln_c99_binary(1, current, rhs); break;
                case KILN_DYN_MINUS_ASSIGN: rhs = kiln_c99_binary(2, current, rhs); break;
                case KILN_DYN_STAR_ASSIGN: rhs = kiln_c99_binary(3, current, rhs); break;
                case KILN_DYN_SLASH_ASSIGN: rhs = kiln_c99_binary(4, current, rhs); break;
                case KILN_DYN_PERCENT_ASSIGN: rhs = kiln_c99_binary(5, current, rhs); break;
                default: break;
                }
            }
            if (!kiln_dyn_scope_update(scope, name, name_len, rhs)) {
                int with_found = 0;
                status = kiln_dyn_scope_set_with(host, scope, name, name_len, rhs, depth,
                                                 &with_found, thrown);
                if (status != KILN_C99_OK) return status;
                if (!with_found) {
                    kiln_value globals = kiln_c99_undefined();
                    status = kiln_c99_global_object(&globals);
                    if (status == KILN_C99_OK) {
                        status = kiln_c99_put(host, globals, name, name_len, rhs, 0, thrown);
                        if (status != KILN_C99_OK) return status;
                    }
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
                    if (!kiln_dyn_scope_get(scope, name, name_len, &target)) {
                        int with_found = 0;
                        status = kiln_dyn_scope_get_with(host, scope, name, name_len, depth,
                                                         &target, &with_found, thrown);
                        if (status != KILN_C99_OK) return status;
                        if (!with_found) {
                            kiln_value globals = kiln_c99_undefined();
                            status = kiln_c99_global_object(&globals);
                            if (status != KILN_C99_OK) return status;
                            status = kiln_c99_get(host, globals, name, name_len, &target, depth,
                                                  thrown);
                            if (status != KILN_C99_OK) return status;
                        }
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
                    if (!kiln_dyn_scope_get(scope, name, name_len, &target)) {
                        int with_found = 0;
                        status = kiln_dyn_scope_get_with(host, scope, name, name_len, depth,
                                                         &target, &with_found, thrown);
                        if (status != KILN_C99_OK) return status;
                        if (!with_found) {
                            kiln_value globals = kiln_c99_undefined();
                            status = kiln_c99_global_object(&globals);
                            if (status != KILN_C99_OK) return status;
                            status = kiln_c99_get(host, globals, name, name_len, &target, depth,
                                                  thrown);
                            if (status != KILN_C99_OK) return status;
                        }
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
    return kiln_dyn_eval_assignment(host, lex, scope, receiver, args, argc, depth, out, thrown);
}

static int kiln_dyn_capture_block(kiln_dyn_lexer *lex, const char **body, size_t *body_len) {
    if (lex->curr.kind != KILN_DYN_LBRACE) return 0;
    size_t start = lex->pos;
    size_t pos = start;
    int nesting = 1;
    while (pos < lex->len && nesting > 0) {
        char c = lex->src[pos++];
        if (c == '\"' || c == '\'' || c == '`') {
            char quote = c;
            while (pos < lex->len) {
                c = lex->src[pos++];
                if (c == '\\' && pos < lex->len) pos++;
                else if (c == quote) break;
            }
        } else if (c == '{') {
            nesting++;
        } else if (c == '}') {
            nesting--;
        }
    }
    if (nesting != 0) return 0;
    *body = &lex->src[start];
    *body_len = pos - start - 1;
    lex->pos = pos;
    kiln_dyn_lexer_next(lex);
    return 1;
}

static int kiln_dyn_capture_expression(kiln_dyn_lexer *lex, const char **body,
                                       size_t *body_len) {
    size_t start = lex->pos;
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
    lex->pos = pos;
    kiln_dyn_lexer_next(lex);
    return 1;
}

static int kiln_dyn_make_closure(kiln_host *host, kiln_dyn_scope *scope,
                                 const char *params, size_t params_len,
                                 const char *body, size_t body_len, kiln_value receiver,
                                 int asynchronous, int arrow, kiln_value *out,
                                 kiln_value *thrown) {
    kiln_value upvals[3];
    kiln_value env_obj;
    int status = kiln_c99_new_object(&env_obj);
    if (status != KILN_C99_OK) return status;
    kiln_c99_gc_keep_alive(&env_obj);
    for (kiln_dyn_scope *s = scope; s != NULL; s = s->parent) {
        for (size_t i = 0; i < s->count; ++i) {
            if (s->bindings[i].value.tag != KILN_VALUE_CELL) {
                status = kiln_c99_make_cell(&s->bindings[i].value);
                if (status != KILN_C99_OK) return status;
            }
            kiln_c99_gc_keep_alive(&s->bindings[i].value);
            status = kiln_c99_put(host, env_obj, s->bindings[i].name,
                                  strlen(s->bindings[i].name), s->bindings[i].value, 0,
                                  thrown);
            if (status != KILN_C99_OK) return status;
        }
    }
    upvals[0] = kiln_c99_copy_text(params, params_len);
    upvals[1] = kiln_c99_copy_text(body, body_len);
    upvals[2] = env_obj;
    if (upvals[0].tag != KILN_VALUE_STRING || upvals[1].tag != KILN_VALUE_STRING)
        return KILN_C99_OUT_OF_MEMORY;
    kiln_c99_gc_keep_alive(&upvals[0]);
    kiln_c99_gc_keep_alive(&upvals[1]);
    kiln_c99_gc_keep_alive(&upvals[2]);
    kiln_val val_upvals[3] = {
        kiln_val_from_boundary(upvals[0]),
        kiln_val_from_boundary(upvals[1]),
        kiln_val_from_boundary(upvals[2])
    };
    return kiln_c99_make_closure(out, kiln_c99_dynamic_body_trampoline, NULL,
                                 "", 0, "eval", 0, 0, 0, asynchronous, 0, 0, arrow, 0,
                                 arrow ? kiln_val_from_boundary(receiver) : KILN_VAL_UNDEFINED,
                                 KILN_VAL_UNDEFINED, 0, 16, 0, 0, val_upvals, 3);
}

static int kiln_dyn_make_arrow_from_tail(kiln_host *host, kiln_dyn_lexer *lex,
                                         kiln_dyn_scope *scope, const char *params,
                                         size_t params_len, kiln_value receiver, int asynchronous,
                                         int depth, kiln_value *out, kiln_value *thrown) {
    const char *body;
    size_t body_len;
    if (lex->curr.kind == KILN_DYN_LBRACE) {
        if (!kiln_dyn_capture_block(lex, &body, &body_len))
            return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_SYNTAX_ERROR,
                                        "Unterminated arrow function body");
        return kiln_dyn_make_closure(host, scope, params, params_len, body, body_len,
                                     receiver, asynchronous, 1, out, thrown);
    }
    if (!kiln_dyn_capture_expression(lex, &body, &body_len))
        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_SYNTAX_ERROR,
                                    "Expected arrow function body");
    char *return_body = (char *)malloc(body_len + sizeof "return ;");
    if (return_body == NULL) return KILN_C99_OUT_OF_MEMORY;
    memcpy(return_body, "return ", 7);
    memcpy(return_body + 7, body, body_len);
    return_body[7 + body_len] = ';';
    int status = kiln_dyn_make_closure(host, scope, params, params_len, return_body,
                                       body_len + 8, receiver, asynchronous, 1, out, thrown);
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

static int kiln_dyn_eval_stmt(kiln_host *host, kiln_dyn_lexer *lex, kiln_dyn_scope *scope,
                              kiln_value receiver, const kiln_value *args, size_t argc,
                              int depth, kiln_value *result, int *returned, kiln_value *thrown) {
    if (lex->curr.kind == KILN_DYN_KW_WITH) {
        kiln_value object;
        kiln_dyn_scope with_scope;
        int status;
        kiln_dyn_lexer_next(lex);
        if (lex->curr.kind != KILN_DYN_LPAREN) {
            return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_SYNTAX_ERROR,
                                        "Expected '(' after with");
        }
        kiln_dyn_lexer_next(lex);
        status = kiln_dyn_eval_expr(host, lex, scope, receiver, args, argc, depth, &object,
                                    thrown);
        if (status != KILN_C99_OK) return status;
        if (lex->curr.kind != KILN_DYN_RPAREN) {
            return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_SYNTAX_ERROR,
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
                return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_SYNTAX_ERROR,
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
        kiln_dyn_lexer_next(lex);
        if (!kiln_dyn_capture_block(lex, &try_body, &try_len)) {
            return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_SYNTAX_ERROR,
                                        "Expected block after try");
        }
        memset(&block, 0, sizeof(block));
        block.src = try_body;
        block.len = try_len;
        kiln_dyn_lexer_next(&block);
        int status = KILN_C99_OK;
        while (block.curr.kind != KILN_DYN_EOF) {
            status = kiln_dyn_eval_stmt(host, &block, scope, receiver, args, argc, depth,
                                        result, returned, thrown);
            if (status != KILN_C99_OK || *returned) break;
        }
        kiln_dyn_token_free(&block.curr);
        int did_throw = status == KILN_C99_THROWN;
        kiln_value exception = did_throw && thrown != NULL ? *thrown : kiln_c99_undefined();
        if (did_throw) {
            status = KILN_C99_OK;
            if (thrown != NULL) *thrown = kiln_c99_undefined();
        }
        int has_catch = lex->curr.kind == KILN_DYN_IDENT && lex->curr.len == 5 &&
                        memcmp(lex->curr.start, "catch", 5) == 0;
        if (has_catch) {
            const char *catch_name = NULL;
            size_t catch_name_len = 0;
            const char *catch_body;
            size_t catch_len;
            kiln_dyn_lexer_next(lex);
            if (lex->curr.kind != KILN_DYN_LPAREN) {
                return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_SYNTAX_ERROR,
                                            "Expected '(' after catch");
            }
            kiln_dyn_lexer_next(lex);
            if (lex->curr.kind == KILN_DYN_IDENT) {
                catch_name = lex->curr.start;
                catch_name_len = lex->curr.len;
                kiln_dyn_lexer_next(lex);
            }
            if (lex->curr.kind != KILN_DYN_RPAREN) {
                return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_SYNTAX_ERROR,
                                            "Expected ')' after catch binding");
            }
            kiln_dyn_lexer_next(lex);
            if (!kiln_dyn_capture_block(lex, &catch_body, &catch_len)) {
                return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_SYNTAX_ERROR,
                                            "Expected catch block");
            }
            if (did_throw) {
                kiln_dyn_scope catch_scope;
                kiln_dyn_scope_init(&catch_scope, scope);
                if (catch_name != NULL &&
                    !kiln_dyn_scope_set(&catch_scope, catch_name, catch_name_len, exception)) {
                    kiln_dyn_scope_destroy(&catch_scope);
                    return KILN_C99_OUT_OF_MEMORY;
                }
                memset(&block, 0, sizeof(block));
                block.src = catch_body;
                block.len = catch_len;
                kiln_dyn_lexer_next(&block);
                while (block.curr.kind != KILN_DYN_EOF) {
                    status = kiln_dyn_eval_stmt(host, &block, &catch_scope, receiver, args,
                                                argc, depth, result, returned, thrown);
                    if (status != KILN_C99_OK || *returned) break;
                }
                kiln_dyn_token_free(&block.curr);
                kiln_dyn_scope_destroy(&catch_scope);
            }
        } else if (did_throw) {
            if (thrown != NULL) *thrown = exception;
            return KILN_C99_THROWN;
        }
        return status;
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
            const char *prev_label = lex->active_label;
            size_t prev_label_len = lex->active_label_len;
            lex->active_label = saved_tok.start;
            lex->active_label_len = saved_tok.len;
            kiln_dyn_lexer_next(lex);
            int status = kiln_dyn_eval_stmt(host, lex, scope, receiver, args, argc, depth, result, returned, thrown);
            lex->active_label = prev_label;
            lex->active_label_len = prev_label_len;
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
            return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_SYNTAX_ERROR, "Expected '(' after if");
        }
        kiln_dyn_lexer_next(lex);
        kiln_value cond;
        int status = kiln_dyn_eval_expr(host, lex, scope, receiver, args, argc, depth, &cond, thrown);
        if (status != KILN_C99_OK) return status;
        if (lex->curr.kind != KILN_DYN_RPAREN) {
            return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_SYNTAX_ERROR, "Expected ')' after if condition");
        }
        kiln_dyn_lexer_next(lex);
        int is_true = kiln_c99_truthy(cond);
        if (is_true) {
            status = kiln_dyn_eval_stmt(host, lex, scope, receiver, args, argc, depth, result, returned, thrown);
            if (status != KILN_C99_OK || *returned) return status;
            if (lex->curr.kind == KILN_DYN_KW_ELSE) {
                kiln_dyn_lexer_next(lex);
                if (lex->curr.kind == KILN_DYN_LBRACE) {
                    int bdepth = 1;
                    kiln_dyn_lexer_next(lex);
                    while (bdepth > 0 && lex->curr.kind != KILN_DYN_EOF) {
                        if (lex->curr.kind == KILN_DYN_LBRACE) bdepth++;
                        else if (lex->curr.kind == KILN_DYN_RBRACE) bdepth--;
                        kiln_dyn_lexer_next(lex);
                    }
                } else {
                    while (lex->curr.kind != KILN_DYN_SEMI && lex->curr.kind != KILN_DYN_EOF) {
                        kiln_dyn_lexer_next(lex);
                    }
                    if (lex->curr.kind == KILN_DYN_SEMI) kiln_dyn_lexer_next(lex);
                }
            }
        } else {
            if (lex->curr.kind == KILN_DYN_LBRACE) {
                int bdepth = 1;
                kiln_dyn_lexer_next(lex);
                while (bdepth > 0 && lex->curr.kind != KILN_DYN_EOF) {
                    if (lex->curr.kind == KILN_DYN_LBRACE) bdepth++;
                    else if (lex->curr.kind == KILN_DYN_RBRACE) bdepth--;
                    kiln_dyn_lexer_next(lex);
                }
            } else {
                while (lex->curr.kind != KILN_DYN_SEMI && lex->curr.kind != KILN_DYN_EOF) {
                    kiln_dyn_lexer_next(lex);
                }
                if (lex->curr.kind == KILN_DYN_SEMI) kiln_dyn_lexer_next(lex);
            }
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
            kiln_dyn_lexer_next(lex);
        }
        if (lex->curr.kind == KILN_DYN_SEMI) kiln_dyn_lexer_next(lex);
        *returned = 3;
        return KILN_C99_OK;
    }
    if (lex->curr.kind == KILN_DYN_KW_FOR) {
        const char *my_label = lex->active_label;
        size_t my_label_len = lex->active_label_len;
        lex->active_label = NULL;
        lex->active_label_len = 0;

        kiln_dyn_lexer_next(lex);
        if (lex->curr.kind != KILN_DYN_LPAREN) {
            return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_SYNTAX_ERROR, "Expected '(' after for");
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
                    return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_SYNTAX_ERROR, "Expected ')' after for target");
                }
                kiln_dyn_lexer_next(lex);

                size_t body_start = lex->pos - lex->curr.len;
                if (lex->curr.kind == KILN_DYN_LBRACE) {
                    int bdepth = 1;
                    kiln_dyn_lexer_next(lex);
                    while (bdepth > 0 && lex->curr.kind != KILN_DYN_EOF) {
                        if (lex->curr.kind == KILN_DYN_LBRACE) bdepth++;
                        else if (lex->curr.kind == KILN_DYN_RBRACE) bdepth--;
                        kiln_dyn_lexer_next(lex);
                    }
                } else {
                    while (lex->curr.kind != KILN_DYN_SEMI && lex->curr.kind != KILN_DYN_EOF) {
                        kiln_dyn_lexer_next(lex);
                    }
                    if (lex->curr.kind == KILN_DYN_SEMI) kiln_dyn_lexer_next(lex);
                }
                size_t body_end = lex->pos - lex->curr.len;
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
                                        if (my_label != NULL && my_label_len == lex->break_label_len &&
                                            strncmp(my_label, lex->break_label, my_label_len) == 0) {
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
                                    kiln_dyn_token_free(&blex.curr);
                                    break;
                                }
                            }
                            kiln_dyn_token_free(&blex.curr);
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
                                        if (my_label != NULL && my_label_len == lex->break_label_len &&
                                            strncmp(my_label, lex->break_label, my_label_len) == 0) {
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
                                    kiln_dyn_token_free(&blex.curr);
                                    break;
                                }
                            }
                            kiln_dyn_token_free(&blex.curr);
                            if (break_loop) break;
                        }
                    }
                }
                if (*returned == 2 && lex->break_label != NULL && my_label != NULL &&
                    my_label_len == lex->break_label_len && strncmp(my_label, lex->break_label, my_label_len) == 0) {
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

        size_t body_start = lex->pos - lex->curr.len;
        if (lex->curr.kind == KILN_DYN_LBRACE) {
            int bdepth = 1;
            kiln_dyn_lexer_next(lex);
            while (bdepth > 0 && lex->curr.kind != KILN_DYN_EOF) {
                if (lex->curr.kind == KILN_DYN_LBRACE) bdepth++;
                else if (lex->curr.kind == KILN_DYN_RBRACE) bdepth--;
                kiln_dyn_lexer_next(lex);
            }
        } else {
            while (lex->curr.kind != KILN_DYN_SEMI && lex->curr.kind != KILN_DYN_EOF) {
                kiln_dyn_lexer_next(lex);
            }
            if (lex->curr.kind == KILN_DYN_SEMI) kiln_dyn_lexer_next(lex);
        }
        size_t body_end = lex->pos - lex->curr.len;

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
                            if (my_label != NULL && my_label_len == lex->break_label_len &&
                                strncmp(my_label, lex->break_label, my_label_len) == 0) {
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
                        kiln_dyn_token_free(&blex.curr);
                        break;
                    }
                }
                kiln_dyn_token_free(&blex.curr);
                if (break_loop) break;
            }
            if (step_len > 0) {
                kiln_dyn_lexer slex;
                memset(&slex, 0, sizeof slex);
                slex.src = step_src;
                slex.len = step_len;
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
        if (*returned == 2 && lex->break_label != NULL && my_label != NULL &&
            my_label_len == lex->break_label_len && strncmp(my_label, lex->break_label, my_label_len) == 0) {
            lex->break_label = NULL;
            lex->break_label_len = 0;
            *returned = 0;
        }
        return KILN_C99_OK;
    }
    if (lex->curr.kind == KILN_DYN_KW_VAR || lex->curr.kind == KILN_DYN_KW_LET || lex->curr.kind == KILN_DYN_KW_CONST) {
        kiln_dyn_lexer_next(lex);
        for (;;) {
            if (lex->curr.kind != KILN_DYN_IDENT) {
                return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_SYNTAX_ERROR,
                                            "Expected identifier");
            }
            const char *name = lex->curr.start;
            size_t name_len = lex->curr.len;
            kiln_dyn_lexer_next(lex);
            kiln_value init = kiln_c99_undefined();
            if (lex->curr.kind == KILN_DYN_ASSIGN) {
                kiln_dyn_lexer_next(lex);
                int status = kiln_dyn_eval_expr(host, lex, scope, receiver, args, argc, depth,
                                                &init, thrown);
                if (status != KILN_C99_OK) return status;
            }
            kiln_dyn_scope_set(scope, name, name_len, init);
            if (lex->curr.kind != KILN_DYN_COMMA) break;
            kiln_dyn_lexer_next(lex);
        }
        if (lex->curr.kind == KILN_DYN_SEMI) kiln_dyn_lexer_next(lex);
        return KILN_C99_OK;
    }
    if (lex->curr.kind == KILN_DYN_LBRACE) {
        kiln_dyn_lexer_next(lex);
        while (lex->curr.kind != KILN_DYN_RBRACE && lex->curr.kind != KILN_DYN_EOF) {
            int status = kiln_dyn_eval_stmt(host, lex, scope, receiver, args, argc, depth, result, returned, thrown);
            if (status != KILN_C99_OK || *returned) return status;
        }
        if (lex->curr.kind == KILN_DYN_RBRACE) kiln_dyn_lexer_next(lex);
        return KILN_C99_OK;
    }

    kiln_value expr_val;
    int status = kiln_dyn_eval_expr(host, lex, scope, receiver, args, argc, depth, &expr_val, thrown);
    if (status != KILN_C99_OK) return status;
    if (lex->curr.kind == KILN_DYN_SEMI) kiln_dyn_lexer_next(lex);
    return KILN_C99_OK;
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

    kiln_dyn_scope scope;
    kiln_dyn_scope_init(&scope, NULL);

    if (env_val.tag == KILN_VALUE_OBJECT) {
        kiln_object *pobj = kiln_c99_object_of(env_val);
        /* The captured environment is an ordinary object built by `kiln_c99_put`, so it is shaped
         * whenever its first binding's name can still branch from the root shape; the walk below
         * reads the property list, which only a dictionary has. */
        if (pobj->shape != NULL) kiln_c99_ensure_dictionary(pobj);
        for (size_t i = 0; i < pobj->count; ++i) {
            kiln_dyn_scope_set_key(&scope, pobj->properties[i].key, pobj->properties[i].value);
        }
    }



    size_t params_len = 0;
    uint8_t *params_owned = NULL;
    if (kiln_c99_string_to_utf8_lossy(params_val, &params_owned, &params_len) != KILN_C99_OK) {
        kiln_dyn_scope_destroy(&scope);
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
                            if (!kiln_dyn_scope_get(&scope, &p[vstart], vlen, &dval)) {
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
            while (ppos < plen && p[ppos] != ',') ppos++;
            arg_idx++;
        }
    }

    free(params_owned);
    size_t body_len = 0;
    uint8_t *body_owned = NULL;
    const char *bsrc;
    if (kiln_c99_string_to_utf8_lossy(body_val, &body_owned, &body_len) != KILN_C99_OK) {
        kiln_dyn_scope_destroy(&scope);
        return KILN_C99_OUT_OF_MEMORY;
    }
    bsrc = (const char *)body_owned;
    size_t blen = body_len;
    kiln_dyn_lexer lex;
    memset(&lex, 0, sizeof(lex));
    lex.src = bsrc;
    lex.len = blen;
    lex.pos = 0;
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
        int status = kiln_c99_to_primitive(host, args[0], KILN_C99_HINT_STRING,
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
            int status = kiln_c99_to_primitive(host, args[i], KILN_C99_HINT_STRING,
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
        int status = kiln_c99_to_primitive(host, args[argc - 1], KILN_C99_HINT_STRING,
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

    int status = kiln_c99_make_closure(result, kiln_c99_dynamic_body_trampoline,
                                       NULL, "", 0,
                                       "eval", 0, (uint32_t)(argc > 1 ? argc - 1 : 0),
                                       0, 0, 0, 0, 0, 0, KILN_VAL_UNDEFINED,
                                       KILN_VAL_UNDEFINED, 0, 16, 0, 0,
                                       val_upvalues, 3);
    kiln_c99_gc_keep_alive(val_upvalues);
    kiln_c99_gc_keep_alive(&upvalues[0]);
    kiln_c99_gc_keep_alive(&upvalues[1]);
    if (status == KILN_C99_OK) kiln_c99_heap_store_value(kiln_c99_closure_of(*result), &kiln_c99_closure_of(*result)->source_value, upvalues[1]);
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
