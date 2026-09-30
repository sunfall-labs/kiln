use super::*;
use kiln_module_resolver::NodeBuiltin;

pub(super) fn graph_shared_header(wrapper_symbol: &str) -> Result<String, NativeError> {
    // The graph wrapper has the same public two-argument signature as a scalar embedded module,
    // so reuse the runtime's canonical symbol validation and include-guarded header generator.
    embedding_module_header(wrapper_symbol).map_err(NativeError::from)
}

/// Append the v2 owned-result ABI adapter to a shared-library translation unit.
///
/// The adapter deliberately calls the already-generated v1 entry point.  That keeps the two
/// entry points semantically identical while making ownership explicit at the DSO boundary.
/// `prefix` is only used to make the private helper names easy to identify in generated C; the
/// public accessor always follows the ABI-mandated `{base}_api_v2` spelling.
pub(super) fn owned_result_wrapper_source(
    base: &str,
    prefix: &str,
    invocation: &str,
    post_invoke_cleanup: &str,
) -> String {
    let stem = format!("{}_{}", prefix, base);
    let arena = format!("kiln_v2_{stem}_arena");
    let proxy = format!("kiln_v2_{stem}_proxy");
    let run = format!("kiln_v2_{stem}_run");
    let read = format!("kiln_v2_{stem}_read");
    let dispose = format!("kiln_v2_{stem}_dispose");
    let descriptor = format!("kiln_v2_{stem}_descriptor");
    let api_symbol = format!("{base}_api_v2");
    format!(
        r#"
/* Kiln native owned-result API v2 adapter for {base}. */
#ifndef KILN_MODULE_API_VERSION
#define KILN_MODULE_API_VERSION {api_version}
typedef struct kiln_result_v2 kiln_result_v2;
typedef struct kiln_module_api_v2 {{
    uint32_t abi_version;
    uint32_t struct_size;
    int (*run)(kiln_host *host, kiln_result_v2 **result);
    int (*read)(const kiln_result_v2 *result, kiln_value *value);
    void (*dispose)(kiln_result_v2 **result);
}} kiln_module_api_v2;
#endif

struct kiln_result_v2 {{
    kiln_value value;
}};

typedef struct {arena} {{
    void **allocations;
    size_t count;
    size_t capacity;
}} {arena};

typedef struct {proxy} {{
    kiln_host *upstream;
    {arena} *arena;
    int failure_status;
}} {proxy};

static void {arena}_clear({arena} *arena) {{
    size_t index;
    if (arena == NULL) return;
    for (index = 0; index < arena->count; ++index) free(arena->allocations[index]);
    free(arena->allocations);
    arena->allocations = NULL;
    arena->count = 0;
    arena->capacity = 0;
}}

static int {arena}_track({arena} *arena, void *allocation) {{
    size_t capacity;
    void **grown;
    if (arena == NULL || allocation == NULL) return KILN_C99_INVALID_ARGUMENT;
    if (arena->count == arena->capacity) {{
        capacity = arena->capacity == 0 ? 8 : arena->capacity;
        if (capacity > (size_t)-1 / 2) return KILN_C99_HOST_ERROR;
        capacity *= 2;
        if (capacity > (size_t)-1 / sizeof(*grown)) return KILN_C99_HOST_ERROR;
        grown = (void **)realloc(arena->allocations, capacity * sizeof(*grown));
        if (grown == NULL) return KILN_C99_HOST_ERROR;
        arena->allocations = grown;
        arena->capacity = capacity;
    }}
    arena->allocations[arena->count++] = allocation;
    return KILN_C99_OK;
}}

static int {stem}_proxy_call(void *context, uint16_t operation, uint16_t argc,
                             const kiln_value *arguments, kiln_value *result) {{
    {proxy} *proxy = ({proxy} *)context;
    kiln_host *upstream;
    kiln_string *copy;
    kiln_value normalized_small[16];
    kiln_value *normalized = NULL;
    int status;
    if (proxy == NULL || proxy->upstream == NULL || proxy->upstream->call == NULL ||
        result == NULL) return KILN_C99_HOST_ERROR;
    upstream = proxy->upstream;
    if (argc != 0) {{
        if (arguments == NULL) return KILN_C99_INVALID_ARGUMENT;
        normalized = argc <= 16 ? normalized_small :
            (kiln_value *)malloc((size_t)argc * sizeof(*normalized));
        if (normalized == NULL) return KILN_C99_HOST_ERROR;
        memcpy(normalized, arguments, (size_t)argc * sizeof(*normalized));
        for (uint16_t index = 0; index < argc; ++index) normalized[index].flags = 0;
    }}
    status = upstream->call(upstream->context, operation, argc, normalized, result);
    if (normalized != normalized_small) free(normalized);
    result->flags = 0;
    if (status != KILN_C99_OK) return status;
    if (result->tag > KILN_VALUE_STRING) {{
        proxy->failure_status = KILN_C99_INVALID_ARGUMENT;
        return KILN_C99_INVALID_ARGUMENT;
    }}
    if (result->tag != KILN_VALUE_STRING) return KILN_C99_OK;
    if (result->len == (uint32_t)-1 || (result->len != 0 && result->ptr == NULL)) {{
        proxy->failure_status = KILN_C99_INVALID_ARGUMENT;
        return KILN_C99_INVALID_ARGUMENT;
    }}
    {{
        const kiln_string *src_str = (const kiln_string *)result->ptr;
        size_t storage_size = 0;
        uint32_t flags = src_str != NULL ? src_str->flags : KILN_STRING_LATIN1;
        uint32_t unit_len = (uint32_t)result->len;
        if (!kiln_string_storage_size(unit_len, flags, &storage_size)) {{
            proxy->failure_status = KILN_C99_INVALID_ARGUMENT;
            return KILN_C99_INVALID_ARGUMENT;
        }}
        copy = (kiln_string *)malloc(storage_size);
        if (copy == NULL) {{
            proxy->failure_status = KILN_C99_HOST_ERROR;
            return KILN_C99_HOST_ERROR;
        }}
        copy->unit_len = unit_len;
        copy->flags = flags;
        copy->data = (const void *)(copy + 1);
        size_t unit_bytes = (size_t)unit_len * (flags == KILN_STRING_UTF16 ? sizeof(uint16_t) : sizeof(uint8_t));
        if (unit_bytes != 0 && src_str != NULL && src_str->data != NULL) {{
            memcpy((void *)copy->data, src_str->data, unit_bytes);
        }}
        status = {arena}_track(proxy->arena, copy);
        if (status != KILN_C99_OK) {{
            free(copy);
            proxy->failure_status = KILN_C99_HOST_ERROR;
            return KILN_C99_HOST_ERROR;
        }}
        result->ptr = (const char *)copy;
        result->flags = 0;
        result->len = unit_len;
        return KILN_C99_OK;
    }}
}}

static int {run}(kiln_host *host, kiln_result_v2 **out) {{
    {arena} arena = {{ NULL, 0, 0 }};
    {proxy} proxy = {{ host, &arena, KILN_C99_OK }};
    kiln_host proxy_host = {{ {stem}_proxy_call, &proxy }};
    kiln_value value = {{ .tag = KILN_VALUE_UNDEFINED, .flags = 0, .len = 0, .bits = 0 }};
    kiln_result_v2 *owned = NULL;
    kiln_string *copy = NULL;
    int status;
    if (out == NULL) return KILN_C99_INVALID_ARGUMENT;
    *out = NULL;
    status = {invocation};
    if (proxy.failure_status != KILN_C99_OK) status = proxy.failure_status;
    if (status != KILN_C99_OK && status != KILN_C99_THROWN) goto cleanup;
    if (value.tag > KILN_VALUE_STRING) {{
        status = KILN_C99_INVALID_ARGUMENT;
        goto cleanup;
    }}
    if (value.tag == KILN_VALUE_STRING &&
        (value.len == (uint32_t)-1 || (value.len != 0 && value.ptr == NULL))) {{
        status = KILN_C99_INVALID_ARGUMENT;
        goto cleanup;
    }}
    owned = (kiln_result_v2 *)malloc(sizeof(*owned));
    if (owned == NULL) {{
        status = KILN_C99_HOST_ERROR;
        goto cleanup;
    }}
    owned->value = value;
    if (value.tag == KILN_VALUE_STRING) {{
        const kiln_string *src_str = (const kiln_string *)value.ptr;
        size_t storage_size = 0;
        uint32_t flags = src_str != NULL ? src_str->flags : KILN_STRING_LATIN1;
        uint32_t unit_len = (uint32_t)value.len;
        if (!kiln_string_storage_size(unit_len, flags, &storage_size)) {{
            status = KILN_C99_INVALID_ARGUMENT;
            goto cleanup;
        }}
        copy = (kiln_string *)malloc(storage_size);
        if (copy == NULL) {{
            status = KILN_C99_HOST_ERROR;
            goto cleanup;
        }}
        copy->unit_len = unit_len;
        copy->flags = flags;
        copy->data = (const void *)(copy + 1);
        size_t unit_bytes = (size_t)unit_len * (flags == KILN_STRING_UTF16 ? sizeof(uint16_t) : sizeof(uint8_t));
        if (unit_bytes != 0 && src_str != NULL && src_str->data != NULL) {{
            memcpy((void *)copy->data, src_str->data, unit_bytes);
        }}
        owned->value.ptr = (const char *)copy;
        owned->value.flags = 0;
        owned->value.len = unit_len;
        copy = NULL;
    }} else {{
        /* `ptr` shares the payload word with `bits`, so clearing it here would erase the number
         * or boolean this value carries. A non-string has no pointer that could dangle; only the
         * length needs normalising. */
        owned->value.len = 0;
    }}
    *out = owned;
    owned = NULL;
cleanup:
    {post_invoke_cleanup}
    free(copy);
    free(owned);
    {arena}_clear(&arena);
    return status;
}}

static int {read}(const kiln_result_v2 *result, kiln_value *value) {{
    if (result == NULL || value == NULL) return KILN_C99_INVALID_ARGUMENT;
    *value = result->value;
    return KILN_C99_OK;
}}

static void {dispose}(kiln_result_v2 **result) {{
    if (result == NULL || *result == NULL) return;
    /* Only a string owns heap bytes. `ptr` shares the payload word with `bits`, so freeing it
     * unconditionally would pass a number's bit pattern to `free`, and clearing it afterwards
     * would erase the value. */
    if ((*result)->value.tag == KILN_VALUE_STRING) {{
        free((void *)(*result)->value.ptr);
        (*result)->value.ptr = NULL;
    }}
    (*result)->value.len = 0;
    free(*result);
    *result = NULL;
}}

static const kiln_module_api_v2 {descriptor} = {{
    KILN_MODULE_API_VERSION,
    (uint32_t)sizeof(kiln_module_api_v2),
    {run},
    {read},
    {dispose}
}};

const kiln_module_api_v2 *{api_symbol}(void) {{
    return &{descriptor};
}}
"#,
        api_version = EMBEDDING_MODULE_API_VERSION,
        api_symbol = api_symbol,
    )
}

/// The embedding wrapper's root function (ADR 0041): the link arrays its body allocates, for as
/// long as they exist, and the module result slot the body writes into.
pub(super) fn render_shared_wrapper_roots(
    output: &mut String,
    modules: &[(&kiln_module_graph::ModuleNode, kiln_backend_c::CModule)],
    wrapper_symbol: &str,
) {
    writeln!(
        output,
        "\ntypedef struct {wrapper_symbol}_gc_context {{\n    kiln_value **imports;\n    kiln_value **exports;\n    kiln_value *module_namespaces;\n    kiln_value *result;\n}} {wrapper_symbol}_gc_context;"
    )
    .unwrap();
    writeln!(
        output,
        "static const size_t {wrapper_symbol}_gc_import_counts[{}] = {{{}}};",
        modules.len().max(1),
        modules
            .iter()
            .map(|(node, _)| node.program.imports.len().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    )
    .unwrap();
    writeln!(
        output,
        "static const size_t {wrapper_symbol}_gc_export_counts[{}] = {{{}}};",
        modules.len().max(1),
        modules
            .iter()
            .map(|(node, _)| native_graph_export_count(node).to_string())
            .collect::<Vec<_>>()
            .join(", ")
    )
    .unwrap();
    let opentui_visit = if modules
        .iter()
        .any(|(node, _)| node.program.program.has_dynamic_import())
    {
        "\n    if (visit(visit_context, &kiln_shared_opentui_namespace, KILN_GC_VISIT_VALUE) != 0 ||\n        visit(visit_context, &kiln_shared_node_ffi_namespace, KILN_GC_VISIT_VALUE) != 0) return KILN_C99_HOST_ERROR;"
    } else {
        ""
    };
    writeln!(output, "static int {wrapper_symbol}_gc_visit_roots(void *context, kiln_gc_visit_root_fn visit, void *visit_context) {{\n    {wrapper_symbol}_gc_context *roots = ({wrapper_symbol}_gc_context *)context;\n    size_t module;\n    size_t index;\n    if (roots == NULL) return KILN_C99_OK;\n    for (module = 0; module < (size_t){count}; ++module) {{\n        if (roots->imports != NULL && roots->imports[module] != NULL)\n            for (index = 0; index < {wrapper_symbol}_gc_import_counts[module]; ++index)\n                if (visit(visit_context, &roots->imports[module][index], KILN_GC_VISIT_VALUE) != 0) return KILN_C99_HOST_ERROR;\n        if (roots->exports != NULL && roots->exports[module] != NULL)\n            for (index = 0; index < {wrapper_symbol}_gc_export_counts[module]; ++index)\n                if (visit(visit_context, &roots->exports[module][index], KILN_GC_VISIT_VALUE) != 0) return KILN_C99_HOST_ERROR;\n        if (roots->module_namespaces != NULL && visit(visit_context, &roots->module_namespaces[module], KILN_GC_VISIT_VALUE) != 0) return KILN_C99_HOST_ERROR;\n    }}\n    if (roots->result != NULL && visit(visit_context, roots->result, KILN_GC_VISIT_VALUE) != 0) return KILN_C99_HOST_ERROR;{opentui_visit}\n    return KILN_C99_OK;\n}}", count = modules.len()).unwrap();
    // The visitor owns the inventory; marking supplies only the operation on each slot.
    writeln!(output, "static int {wrapper_symbol}_gc_mark_root(void *heap, const void *pointer, uint32_t kind) {{\n    if (kind != KILN_GC_VISIT_VALUE) return KILN_C99_INVALID_ARGUMENT;\n    (void)kiln_realm_gc_mark_value(heap, pointer);\n    return KILN_C99_OK;\n}}\nstatic void {wrapper_symbol}_gc_roots(void *heap, void *context, int phase) {{\n    if (phase == KILN_GC_PHASE_RESET || phase == KILN_GC_PHASE_LANE_RESET || context == NULL) return;\n    (void){wrapper_symbol}_gc_visit_roots(context, {wrapper_symbol}_gc_mark_root, heap);\n}}").unwrap();
}

#[cfg(test)]
pub(super) fn graph_shared_wrapper_source(
    graph: &ModuleGraph,
    modules: &[(&kiln_module_graph::ModuleNode, kiln_backend_c::CModule)],
    wrapper_symbol: &str,
) -> Result<String, NativeError> {
    graph_shared_wrapper_source_with_failures(
        graph,
        modules,
        wrapper_symbol,
        &BTreeMap::new(),
        &BTreeMap::new(),
    )
}

pub(super) fn graph_shared_wrapper_source_with_failures(
    graph: &ModuleGraph,
    modules: &[(&kiln_module_graph::ModuleNode, kiln_backend_c::CModule)],
    wrapper_symbol: &str,
    native_failures: &BTreeMap<(PathBuf, String), String>,
    native_runtime_failures: &BTreeMap<(PathBuf, PathBuf, String), String>,
) -> Result<String, NativeError> {
    if modules.is_empty() {
        return Err(NativeError::Graph(
            ModuleGraphError::InternalMissingModule {
                path: graph.entry().to_path_buf(),
            },
        ));
    }
    let impl_symbol = format!("{}_impl", wrapper_symbol);
    let mut indices = std::collections::HashMap::with_capacity(modules.len());
    for (index, (node, _)) in modules.iter().enumerate() {
        indices.insert(node.path.clone(), index);
    }
    let namespace_plans = graph_namespace_plans(graph, modules, &indices, None, None)?;
    let entry_index = modules
        .iter()
        .position(|(node, _)| node.path == graph.entry())
        .ok_or_else(|| {
            NativeError::Graph(ModuleGraphError::InternalMissingModule {
                path: graph.entry().to_path_buf(),
            })
        })?;
    let logical_paths = modules
        .iter()
        .map(|(node, _)| logical_module_path(graph.project_root(), &node.path))
        .collect::<Result<Vec<_>, _>>()?;
    let mut runtime_importers = BTreeMap::<PathBuf, usize>::new();
    for (node, _) in modules {
        for dependency in &node.runtime_dependencies {
            let next = runtime_importers.len();
            runtime_importers
                .entry(dependency.importer.clone())
                .or_insert(next);
        }
        for ((importer, _), _) in graph.runtime_failures(&node.path) {
            let next = runtime_importers.len();
            runtime_importers.entry(importer.clone()).or_insert(next);
        }
        for ((owner, importer, _), _) in native_runtime_failures {
            if owner == &node.path {
                let next = runtime_importers.len();
                runtime_importers.entry(importer.clone()).or_insert(next);
            }
        }
    }
    let mut output = String::with_capacity(embedding_header().len() + modules.len() * 768);
    output.push_str("/* Kiln native shared graph wrapper. */\n");
    output.push_str(
        "#include <stddef.h>\n#include <stdint.h>\n#include <stdlib.h>\n#include <string.h>\n#ifndef _WIN32\n#include <unistd.h>\n#endif\n\n",
    );
    output.push_str(embedding_header());
    output.push('\n');
    output.push_str(
        "static inline int kiln_c99_string_equals_ascii(kiln_value value, const char *ascii, size_t length) { if (value.tag != KILN_VALUE_STRING || value.ptr == NULL) return 0; return kiln_string_equals_ascii((const kiln_string *)value.ptr, ascii, length); }\n\
         static inline int kiln_c99_string_starts_ascii(kiln_value value, const char *ascii, size_t length) { if (value.tag != KILN_VALUE_STRING || value.ptr == NULL) return 0; return kiln_string_starts_ascii((const kiln_string *)value.ptr, ascii, length); }\n\
         static int kiln_c99_filename_matches(kiln_value value, const char *path, size_t path_len) { if (path == NULL || value.tag != KILN_VALUE_STRING || (value.len != 0 && value.ptr == NULL)) return 0; const kiln_string *s = (const kiln_string *)value.ptr; if (s == NULL) return 0; if (s->unit_len == path_len && (path_len == 0 || kiln_string_equals_ascii(s, path, path_len))) return 1; if (s->unit_len >= 7 && kiln_string_starts_ascii(s, \"file://\", 7) && s->unit_len - 7 == path_len) { if (path_len == 0) return 1; for (size_t i = 0; i < path_len; ++i) { if (kiln_string_unit_at(s, i + 7) != (uint8_t)path[i]) return 0; } return 1; } return 0; }\n",
    );
    output.push_str(
        r#"typedef int (*kiln_realm_entry_fn)(void *context);
extern int kiln_realm_create(uint64_t *out_realm);
extern int kiln_realm_enter(uint64_t realm, kiln_realm_entry_fn entry, void *context);
extern int kiln_realm_drive(uint64_t realm, void *host, void *thrown);
extern int kiln_realm_destroy(uint64_t realm);
typedef void (*kiln_gc_roots_fn)(void *heap, void *context, int phase);
extern int kiln_realm_gc_register_roots(kiln_gc_roots_fn roots, void *context, uint32_t flags);
typedef int (*kiln_gc_visit_root_fn)(void *context, const void *pointer, uint32_t kind);
typedef int (*kiln_gc_visit_roots_fn)(void *context, kiln_gc_visit_root_fn visit, void *visit_context);
extern int kiln_realm_gc_register_roots_with_visitor(kiln_gc_roots_fn roots, kiln_gc_visit_roots_fn visit_roots, void *context, uint32_t flags);
#define KILN_GC_VISIT_VALUE 2u
extern int kiln_realm_gc_unregister_roots(kiln_gc_roots_fn roots, void *context);
extern int kiln_realm_gc_mark_value(void *heap, const void *value);
#define KILN_GC_ROOT_SURVIVES_TEARDOWN 1u
#define KILN_GC_PHASE_RESET 2
#define KILN_GC_PHASE_LANE_RESET 3
#define KILN_C99_HINT_STRING 1
#define KILN_C99_HOST_MODULE_CREATE_REQUIRE UINT16_C(0x7FF0)
extern int kiln_realm_is_active(void);
extern int kiln_realm_kiln_shared_hooks(int begin);

typedef int (*kiln_graph_module_fn)(kiln_host *host, kiln_value *result,
                                    const kiln_value *imports, size_t import_count,
                                    kiln_value *exports, size_t export_count);
typedef int (*kiln_commonjs_require_fn)(void *context, kiln_host *host, kiln_value specifier,
                                        kiln_value *result, kiln_value *thrown);
typedef struct kiln_commonjs_context kiln_commonjs_context;
extern int kiln_c99_commonjs_require_value(kiln_value *out, kiln_commonjs_require_fn call, void *context);
extern int kiln_c99_commonjs_require_value_with_roots(kiln_value *out, kiln_commonjs_require_fn call, void *context, kiln_gc_visit_roots_fn visit_context, void *root_context);
extern int kiln_c99_default_require(void *context, kiln_host *host, kiln_value specifier,
                                    kiln_value *result, kiln_value *thrown);
extern int kiln_c99_get(kiln_host *host, kiln_value target, const char *key, size_t key_len,
                        kiln_value *out, int depth, kiln_value *thrown);
extern int kiln_c99_new_object(kiln_value *result);
extern int kiln_c99_put(kiln_host *host, kiln_value target, const char *key, size_t key_len,
                        kiln_value value, int depth, kiln_value *thrown);
extern kiln_value kiln_c99_copy_text(const char *text, size_t len);
extern kiln_value kiln_c99_undefined(void);
extern int kiln_c99_to_primitive(kiln_host *host, kiln_value value, int hint, kiln_value *out, int depth, kiln_value *thrown);
extern kiln_value kiln_c99_text(kiln_value value);
extern int kiln_c99_throw_error(kiln_value *out, uint64_t kind, const char *message);
extern int kiln_c99_new_type_error(kiln_value *out, const char *message);
extern int kiln_c99_invoke_method(kiln_host *host, kiln_value *result, uint64_t code,
                                  kiln_value receiver, const kiln_value *args, size_t argc,
                                  int depth, kiln_value *thrown);
extern int kiln_c99_module_completion(kiln_host *, kiln_value, uint32_t, kiln_value *, int, kiln_value *);
extern int kiln_c99_finish_module_checkpoint(kiln_host *, kiln_value *);
#define KILN_C99_OK 0
#define KILN_C99_METHOD_PROMISE_RESOLVE 110
#define KILN_C99_METHOD_PROMISE_REJECT 111
#define KILN_C99_METHOD_PROMISE_WITH_RESOLVERS 411
static __thread kiln_value *kiln_shared_namespaces = NULL;
static __thread kiln_commonjs_context *kiln_shared_commonjs = NULL;
"#,
    );
    if modules
        .iter()
        .any(|(node, _)| node.program.program.has_dynamic_import())
    {
        output.push_str("static __thread kiln_value kiln_shared_opentui_namespace;\n");
        output.push_str("static __thread kiln_value kiln_shared_node_ffi_namespace;\n");
    }
    for (_, module) in modules {
        writeln!(
            output,
            "extern int {}(kiln_host *host, kiln_value *result, const kiln_value *imports, size_t import_count, kiln_value *exports, size_t export_count);",
            module.symbol
        )
        .unwrap();
        writeln!(
            output,
            "extern int {}_namespace_wrapper(kiln_value *result, const char *const *names, const size_t *name_lengths, const kiln_value *values, size_t count);",
            module.symbol
        )
        .unwrap();
    }
    output.push_str(
        r#"
typedef struct kiln_commonjs_context {
    kiln_graph_module_fn module;
    kiln_host *host;
    kiln_value *imports;
    size_t import_count;
    kiln_value *exports;
    int state;
} kiln_commonjs_context;

typedef struct kiln_commonjs_graph_context kiln_commonjs_graph_context;
typedef struct kiln_shared_require_context {
    kiln_commonjs_graph_context *graph;
    size_t base_index;
} kiln_shared_require_context;
struct kiln_commonjs_graph_context {
    kiln_commonjs_context *modules;
    kiln_value **exports;
    size_t module_count;
    kiln_shared_require_context *require_contexts;
    kiln_gc_visit_roots_fn visit_roots;
    void *root_context;
};

static int kiln_graph_load_commonjs(kiln_commonjs_context *context, kiln_value *result,
                                    kiln_value *thrown);

static int kiln_shared_ensure_target(kiln_commonjs_graph_context *graph, size_t index,
                                     kiln_value *result, kiln_value *thrown) {
    int status;
    if (graph == NULL || result == NULL || index >= graph->module_count ||
        graph->modules == NULL || graph->exports == NULL) return KILN_C99_INVALID_ARGUMENT;
    if (graph->modules[index].module != NULL) {
        status = kiln_graph_load_commonjs(&graph->modules[index], result, thrown);
        return status;
    }
    return KILN_C99_HOST_ERROR;
}

static int kiln_graph_load_commonjs(kiln_commonjs_context *context, kiln_value *result,
                                    kiln_value *thrown) {
    int status;
    if (context == NULL || result == NULL || context->exports == NULL) {
        return KILN_C99_INVALID_ARGUMENT;
    }
    if (context->state != 0) {
        *result = context->exports[0];
        return KILN_C99_OK;
    }
    context->state = 1;
    status = context->module(context->host, result, context->imports, context->import_count,
                             context->exports, (size_t)1);
    if (status != KILN_C99_OK) {
        context->state = 0;
        if (thrown != NULL) *thrown = *result;
        return status;
    }
    context->state = 2;
    *result = context->exports[0];
    return KILN_C99_OK;
}
"#,
    );
    for (node, module) in modules {
        if node.source == ModuleSource::CommonJs {
            writeln!(
                output,
                "extern int {}_commonjs_wrapper(kiln_value *imports, size_t import_count, const char *filename, size_t filename_len, const char *dirname, size_t dirname_len, kiln_commonjs_require_fn require_call, void *require_context, kiln_gc_visit_roots_fn require_visit_roots, void *require_root_context);",
                module.symbol
            )
            .unwrap();
        }
    }
    output.push_str(
        r#"
typedef struct kiln_proxy_arena {
    void **allocations;
    size_t count;
    size_t capacity;
} kiln_proxy_arena;

typedef struct kiln_proxy_host {
    kiln_host *upstream;
    /* The graph context is owned by the shared wrapper's realm frame.  Keeping this pointer
     * beside the forwarding host gives later graph callbacks a stable route back to the graph
     * without replacing the embedding host's call/context pair or using process-global state. */
    kiln_commonjs_graph_context *graph;
    kiln_proxy_arena *arena;
    int failure_status;
} kiln_proxy_host;

static int kiln_shared_create_require(kiln_proxy_host *proxy, uint16_t argc,
                                      const kiln_value *arguments, kiln_value *result);

static void kiln_proxy_arena_clear(kiln_proxy_arena *arena) {
    size_t index;
    if (arena == NULL) return;
    for (index = 0; index < arena->count; ++index) free(arena->allocations[index]);
    free(arena->allocations);
    arena->allocations = NULL;
    arena->count = 0;
    arena->capacity = 0;
}

static int kiln_proxy_arena_track(kiln_proxy_arena *arena, void *allocation) {
    size_t capacity;
    void **grown;
    if (arena == NULL || allocation == NULL) return KILN_C99_INVALID_ARGUMENT;
    if (arena->count == arena->capacity) {
        capacity = arena->capacity == 0 ? 8 : arena->capacity;
        if (capacity > (size_t)-1 / 2) return KILN_C99_HOST_ERROR;
        capacity *= 2;
        if (capacity > (size_t)-1 / sizeof(*grown)) return KILN_C99_HOST_ERROR;
        grown = (void **)realloc(arena->allocations, capacity * sizeof(*grown));
        if (grown == NULL) return KILN_C99_HOST_ERROR;
        arena->allocations = grown;
        arena->capacity = capacity;
    }
    arena->allocations[arena->count++] = allocation;
    return KILN_C99_OK;
}

static int kiln_proxy_call(void *context, uint16_t operation, uint16_t argc,
                           const kiln_value *arguments, kiln_value *result) {
    kiln_proxy_host *proxy = (kiln_proxy_host *)context;
    kiln_host *upstream;
    kiln_proxy_arena *arena;
    kiln_string *copy;
    kiln_value normalized_small[16];
    kiln_value *normalized_arguments = NULL;
    int status;
    if (proxy == NULL || proxy->upstream == NULL || proxy->upstream->call == NULL ||
        result == NULL) return KILN_C99_INVALID_ARGUMENT;
    upstream = proxy->upstream;
    arena = proxy->arena;
    if (operation == KILN_C99_HOST_MODULE_CREATE_REQUIRE)
        return kiln_shared_create_require(proxy, argc, arguments, result);
    if (argc != 0) {
        if (arguments == NULL || (size_t)argc > (size_t)-1 / sizeof(*normalized_arguments)) {
            proxy->failure_status = KILN_C99_INVALID_ARGUMENT;
            return KILN_C99_INVALID_ARGUMENT;
        }
        normalized_arguments = argc <= 16 ? normalized_small :
            (kiln_value *)malloc((size_t)argc * sizeof(*normalized_arguments));
        if (normalized_arguments == NULL) {
            proxy->failure_status = KILN_C99_HOST_ERROR;
            return KILN_C99_HOST_ERROR;
        }
        memcpy(normalized_arguments, arguments, (size_t)argc * sizeof(*normalized_arguments));
        for (uint16_t index = 0; index < argc; ++index) normalized_arguments[index].flags = 0;
    }
    status = upstream->call(upstream->context, operation, argc, normalized_arguments, result);
    if (normalized_arguments != normalized_small) free(normalized_arguments);
    result->flags = 0;
    if (status != KILN_C99_OK) return status;
    if (result->tag != KILN_VALUE_STRING) return KILN_C99_OK;
    if (result->len == (uint32_t)-1 || (result->len != 0 && result->ptr == NULL)) {
        proxy->failure_status = KILN_C99_INVALID_ARGUMENT;
        return KILN_C99_INVALID_ARGUMENT;
    }
    {
        const kiln_string *src_str = (const kiln_string *)result->ptr;
        size_t storage_size = 0;
        uint32_t flags = src_str != NULL ? src_str->flags : KILN_STRING_LATIN1;
        uint32_t unit_len = (uint32_t)result->len;
        if (!kiln_string_storage_size(unit_len, flags, &storage_size)) {
            proxy->failure_status = KILN_C99_INVALID_ARGUMENT;
            return KILN_C99_INVALID_ARGUMENT;
        }
        copy = (kiln_string *)malloc(storage_size);
        if (copy == NULL) {
            proxy->failure_status = KILN_C99_HOST_ERROR;
            return KILN_C99_HOST_ERROR;
        }
        copy->unit_len = unit_len;
        copy->flags = flags;
        copy->data = (const void *)(copy + 1);
        size_t unit_bytes = (size_t)unit_len * (flags == KILN_STRING_UTF16 ? sizeof(uint16_t) : sizeof(uint8_t));
        if (unit_bytes != 0 && src_str != NULL && src_str->data != NULL) {
            memcpy((void *)copy->data, src_str->data, unit_bytes);
        }
        status = kiln_proxy_arena_track(arena, copy);
        if (status != KILN_C99_OK) {
            free(copy);
            proxy->failure_status = KILN_C99_HOST_ERROR;
            return KILN_C99_HOST_ERROR;
        }
        result->ptr = (const char *)copy;
        result->flags = 0;
        result->len = unit_len;
        return KILN_C99_OK;
    }
}

/* Shared graph callbacks receive the wrapper host.  These accessors are deliberately tiny and
 * private to the generated wrapper: the original embedding host remains the sole owner of host
 * operations, while the graph context lifetime follows the realm frame that registered roots. */
static kiln_commonjs_graph_context *kiln_graph_context_from_host(kiln_host *host) {
    kiln_proxy_host *proxy;
    if (host == NULL || host->call != kiln_proxy_call || host->context == NULL) return NULL;
    proxy = (kiln_proxy_host *)host->context;
    return proxy->graph;
}

"#,
    );
    for (index, (node, _)) in modules.iter().enumerate() {
        writeln!(
            output,
            "\nstatic int kiln_commonjs_require_{index}(void *opaque, kiln_host *host, kiln_value specifier, kiln_value *result, kiln_value *thrown) {{\n    kiln_commonjs_graph_context *graph = (kiln_commonjs_graph_context *)opaque;\n    (void)host;\n    if (graph == NULL || result == NULL || specifier.tag != KILN_VALUE_STRING || (specifier.len != 0 && specifier.ptr == NULL)) return KILN_C99_INVALID_ARGUMENT;"
        )
        .unwrap();
        for dependency in &node.dependencies {
            let provider_index = *indices.get(&dependency.target).ok_or_else(|| {
                NativeError::Graph(ModuleGraphError::InternalMissingModule {
                    path: dependency.target.clone(),
                })
            })?;
            let specifier = native_c_string(&dependency.specifier);
            let specifier_len = dependency.specifier.len();
            if modules[provider_index].0.source == ModuleSource::CommonJs {
                writeln!(
                    output,
                    "    if (kiln_c99_string_equals_ascii(specifier, {specifier}, (size_t){specifier_len})) return kiln_graph_load_commonjs(&graph->modules[{provider_index}], result, thrown);"
                )
                .unwrap();
            } else {
                let Some(commonjs_export) =
                    native_commonjs_projection_index(modules[provider_index].0)
                else {
                    continue;
                };
                writeln!(
                    output,
                    "    if (kiln_c99_string_equals_ascii(specifier, {specifier}, (size_t){specifier_len})) {{ *result = graph->exports[{provider_index}][{commonjs_export}]; return KILN_C99_OK; }}"
                )
                .unwrap();
            }
        }
        output.push_str(
            "    if (thrown != NULL) *thrown = specifier;\n    return KILN_C99_HOST_ERROR;\n}\n",
        );
    }
    output.push_str("\nstatic int kiln_shared_require_builtin(void *opaque, kiln_host *host, kiln_value specifier, kiln_value *result, kiln_value *thrown) {\n    kiln_commonjs_graph_context *graph = (kiln_commonjs_graph_context *)opaque;\n    (void)host;\n    if (graph == NULL || result == NULL || specifier.tag != KILN_VALUE_STRING || (specifier.len != 0 && specifier.ptr == NULL)) return KILN_C99_INVALID_ARGUMENT;\n");
    for (index, (node, _)) in modules.iter().enumerate() {
        let ModuleSource::NodeBuiltin(builtin) = node.source else {
            continue;
        };
        let Some(projection) = native_commonjs_projection_index(node) else {
            continue;
        };
        let specifier = builtin.specifier();
        writeln!(output, "    if (kiln_c99_string_equals_ascii(specifier, {}, (size_t){})) {{ *result = graph->exports[{}][{}]; return KILN_C99_OK; }}", native_c_string(specifier), specifier.len(), index, projection).unwrap();
    }
    output.push_str("    return kiln_c99_default_require(NULL, host, specifier, result, thrown);\n}\n\nstatic int kiln_shared_runtime_require(void *opaque, kiln_host *host, kiln_value specifier, kiln_value *result, kiln_value *thrown) {\n    kiln_shared_require_context *context = (kiln_shared_require_context *)opaque;\n    kiln_commonjs_graph_context *graph;\n    if (context == NULL || (graph = context->graph) == NULL || result == NULL || specifier.tag != KILN_VALUE_STRING || (specifier.len != 0 && specifier.ptr == NULL)) return KILN_C99_INVALID_ARGUMENT;\n    (void)host;\n    switch (context->base_index) {\n");
    for (base_index, importer) in runtime_importers.keys().enumerate() {
        writeln!(output, "        case {base_index}:").unwrap();
        for node in modules.iter().map(|(node, _)| node) {
            for dependency in &node.runtime_dependencies {
                if dependency.importer != *importer {
                    continue;
                }
                let Some(&provider_index) = indices.get(&dependency.target) else {
                    continue;
                };
                let specifier = native_c_string(&dependency.specifier);
                let specifier_len = dependency.specifier.len();
                if modules[provider_index].0.source == ModuleSource::CommonJs {
                    writeln!(output, "            if (kiln_c99_string_equals_ascii(specifier, {specifier}, (size_t){specifier_len})) return kiln_shared_ensure_target(graph, (size_t){provider_index}, result, thrown);").unwrap();
                } else if let Some(projection) =
                    native_commonjs_projection_index(modules[provider_index].0)
                {
                    writeln!(output, "            if (kiln_c99_string_equals_ascii(specifier, {specifier}, (size_t){specifier_len})) {{ *result = graph->exports[{provider_index}][{projection}]; return KILN_C99_OK; }}").unwrap();
                }
            }
        }
        for node in modules.iter().map(|(node, _)| node) {
            for ((failure_importer, failure_specifier), message) in
                graph.runtime_failures(&node.path)
            {
                if failure_importer != importer {
                    continue;
                }
                writeln!(output, "            if (kiln_c99_string_equals_ascii(specifier, {}, (size_t){})) {{ kiln_value error = kiln_c99_undefined(); (void)kiln_c99_new_type_error(&error, {}); if (thrown != NULL) *thrown = error; return KILN_C99_THROWN; }}", native_c_string(failure_specifier), failure_specifier.len(), native_c_string(message)).unwrap();
            }
        }
        for ((owner, failure_importer, failure_specifier), message) in native_runtime_failures {
            if failure_importer != importer {
                continue;
            }
            if !modules.iter().any(|(node, _)| node.path == *owner) {
                continue;
            }
            writeln!(output, "            if (kiln_c99_string_equals_ascii(specifier, {}, (size_t){})) {{ kiln_value error = kiln_c99_undefined(); (void)kiln_c99_new_type_error(&error, {}); if (thrown != NULL) *thrown = error; return KILN_C99_THROWN; }}", native_c_string(failure_specifier), failure_specifier.len(), native_c_string(message)).unwrap();
        }
        output.push_str("            break;\n");
    }
    output.push_str("        default: break;\n    }\n    return kiln_c99_default_require(NULL, host, specifier, result, thrown);\n}\n\nstatic int kiln_shared_create_require(kiln_proxy_host *proxy, uint16_t argc, const kiln_value *arguments, kiln_value *result) {\n    kiln_commonjs_graph_context *graph;\n    if (proxy == NULL || result == NULL || argc != 1 || arguments == NULL || arguments[0].tag != KILN_VALUE_STRING || (arguments[0].len != 0 && arguments[0].ptr == NULL)) return KILN_C99_INVALID_ARGUMENT;\n    graph = proxy->graph;\n    if (graph == NULL || graph->require_contexts == NULL) return KILN_C99_HOST_ERROR;\n");
    for (base_index, importer) in runtime_importers.keys().enumerate() {
        let importer = importer.to_string_lossy();
        let file_url = Url::from_file_path(importer.as_ref())
            .ok()
            .map(|url| url.to_string());
        let logical_alias = modules
            .iter()
            .find(|(node, _)| node.path == *importer)
            .map(|_| logical_module_path(graph.project_root(), Path::new(importer.as_ref())))
            .transpose()?;
        let aliases = format!(
            "{}{}",
            file_url
                .as_ref()
                .map(|url| format!(
                    " || kiln_c99_filename_matches(arguments[0], {}, (size_t){})",
                    native_c_string(url),
                    url.len()
                ))
                .unwrap_or_default(),
            logical_alias
                .as_ref()
                .map(|path| format!(
                    " || kiln_c99_filename_matches(arguments[0], {}, (size_t){})",
                    native_c_string(path.as_str()),
                    path.as_str().len()
                ))
                .unwrap_or_default()
        );
        writeln!(output, "    if (kiln_c99_filename_matches(arguments[0], {}, (size_t){}){} ) {{ graph->require_contexts[{base_index}].graph = graph; graph->require_contexts[{base_index}].base_index = (size_t){base_index}; return kiln_c99_commonjs_require_value_with_roots(result, kiln_shared_runtime_require, &graph->require_contexts[{base_index}], graph->visit_roots, graph->root_context); }}", native_c_string(importer.as_ref()), importer.len(), aliases).unwrap();
    }
    output.push_str("    return kiln_c99_commonjs_require_value_with_roots(result, kiln_shared_require_builtin, graph, graph->visit_roots, graph->root_context);\n}\n");
    let has_any_dynamic_import = modules
        .iter()
        .any(|(node, _)| node.program.program.has_dynamic_import());
    if has_any_dynamic_import {
        write_shared_wrapper_dynamic_import(
            &mut output,
            graph,
            modules,
            &logical_paths,
            native_failures,
        );
    }
    render_shared_wrapper_roots(&mut output, modules, wrapper_symbol);
    let opentui_init = if has_any_dynamic_import {
        "\n    kiln_shared_opentui_namespace = (kiln_value){ .tag = KILN_VALUE_UNDEFINED, .flags = 0, .len = 0, .bits = 0 };\n    kiln_shared_node_ffi_namespace = (kiln_value){ .tag = KILN_VALUE_UNDEFINED, .flags = 0, .len = 0, .bits = 0 };"
    } else {
        ""
    };
    writeln!(
        output,
        r#"
typedef struct {wrapper_symbol}_realm_context {{
    kiln_value *result;
    kiln_proxy_arena arena;
    kiln_proxy_host proxy;
    kiln_host proxy_host;
    kiln_value **shared_import_arrays;
    kiln_value **shared_export_arrays;
    kiln_value *shared_namespaces;
    kiln_commonjs_context *shared_commonjs_modules;
    kiln_commonjs_graph_context shared_commonjs_graph;
    kiln_shared_require_context require_contexts[{runtime_context_count}];
    {wrapper_symbol}_gc_context shared_gc_context;
    kiln_value shared_module_result;
    int roots_registered;
}} {wrapper_symbol}_realm_context;
"#,
        wrapper_symbol = wrapper_symbol,
        runtime_context_count = runtime_importers.len().max(1),
    )
    .unwrap();
    writeln!(
        output,
        "\nstatic int {wrapper_symbol}_body({wrapper_symbol}_realm_context *context) {{\n    kiln_host *proxy_host;\n    kiln_value *result;\n    kiln_value **import_arrays = NULL;\n    kiln_value **export_arrays = NULL;\n    kiln_value *namespaces = NULL;\n    kiln_commonjs_context *commonjs_modules = NULL;\n    kiln_commonjs_graph_context *commonjs_graph;\n    {wrapper_symbol}_gc_context *gc_context;\n    /* All graph storage belongs to the realm context and survives this body until drive and\n     * realm teardown have completed. */\n    #define module_result (context->shared_module_result)\n    int status = KILN_C99_OK;\n    size_t index;\n    if (context == NULL) return KILN_C99_INVALID_ARGUMENT;\n    proxy_host = &context->proxy_host;\n    result = context->result;\n    import_arrays = context->shared_import_arrays;\n    export_arrays = context->shared_export_arrays;\n    namespaces = context->shared_namespaces;\n    commonjs_modules = context->shared_commonjs_modules;\n    commonjs_graph = &context->shared_commonjs_graph;\n    gc_context = &context->shared_gc_context;\n    if (result == NULL || proxy_host == NULL) return KILN_C99_INVALID_ARGUMENT;\n    *commonjs_graph = (kiln_commonjs_graph_context){{ NULL, NULL, (size_t){count} }};\n    *result = (kiln_value){{ .tag = KILN_VALUE_UNDEFINED, .flags = 0, .len = 0, .bits = 0 }};\n    if ((size_t){count} > (size_t)-1 / sizeof(*import_arrays) ||\n        (size_t){count} > (size_t)-1 / sizeof(*export_arrays) ||\n        (size_t){count} > (size_t)-1 / sizeof(*namespaces) ||\n        (size_t){count} > (size_t)-1 / sizeof(*commonjs_modules)) {{\n        status = KILN_C99_HOST_ERROR;\n        goto cleanup;\n    }}\n    import_arrays = (kiln_value **)calloc((size_t){count}, sizeof(*import_arrays));\n    context->shared_import_arrays = import_arrays;\n    export_arrays = (kiln_value **)calloc((size_t){count}, sizeof(*export_arrays));\n    context->shared_export_arrays = export_arrays;\n    namespaces = (kiln_value *)calloc((size_t){count}, sizeof(*namespaces));\n    context->shared_namespaces = namespaces;\n    commonjs_modules = (kiln_commonjs_context *)calloc((size_t){count}, sizeof(*commonjs_modules));\n    context->shared_commonjs_modules = commonjs_modules;\n    if (import_arrays == NULL || export_arrays == NULL || namespaces == NULL || commonjs_modules == NULL) {{ status = KILN_C99_HOST_ERROR; goto cleanup; }}\n    commonjs_graph->modules = commonjs_modules;\n    commonjs_graph->exports = export_arrays;\n    /* The wrapper host carries the graph context for callbacks, but preserves the embedding\n     * host's function and context verbatim for every host operation. */\n    if (proxy_host->context != NULL && proxy_host->call == kiln_proxy_call)\n        ((kiln_proxy_host *)proxy_host->context)->graph = commonjs_graph;\n    if (kiln_graph_context_from_host(proxy_host) != commonjs_graph) {{ status = KILN_C99_INVALID_ARGUMENT; goto cleanup; }}\n    gc_context->imports = import_arrays;\n    gc_context->exports = export_arrays;\n    gc_context->module_namespaces = namespaces;\n    gc_context->result = &module_result;\n    module_result = (kiln_value){{ .tag = KILN_VALUE_UNDEFINED, .flags = 0, .len = 0, .bits = 0 }};\n    status = kiln_realm_gc_register_roots_with_visitor({wrapper_symbol}_gc_roots, {wrapper_symbol}_gc_visit_roots, gc_context, KILN_GC_ROOT_SURVIVES_TEARDOWN);\n    if (status != KILN_C99_OK) goto cleanup;\n    context->roots_registered = 1;\n    commonjs_graph->visit_roots = {wrapper_symbol}_gc_visit_roots;\n    commonjs_graph->root_context = gc_context;\n    kiln_shared_namespaces = namespaces;\n    kiln_shared_commonjs = commonjs_modules;{opentui_init}",
        wrapper_symbol = wrapper_symbol,
        count = modules.len(),
    )
    .unwrap();
    output.push_str("    commonjs_graph->require_contexts = context->require_contexts;\n");
    for (index, (node, _)) in modules.iter().enumerate() {
        if !node.program.imports.is_empty() {
            writeln!(
                output,
                "    if ((size_t){} > (size_t)-1 / sizeof(**import_arrays)) {{ status = KILN_C99_HOST_ERROR; goto cleanup; }}\n    import_arrays[{index}] = (kiln_value *)calloc((size_t){}, sizeof(**import_arrays));\n    if (import_arrays[{index}] == NULL) {{ status = KILN_C99_HOST_ERROR; goto cleanup; }}",
                node.program.imports.len(),
                node.program.imports.len()
            )
            .unwrap();
        }
        let export_count = native_graph_export_count(node);
        if export_count != 0 {
            writeln!(
                output,
                "    if ((size_t){export_count} > (size_t)-1 / sizeof(**export_arrays)) {{ status = KILN_C99_HOST_ERROR; goto cleanup; }}\n    export_arrays[{index}] = (kiln_value *)malloc((size_t){export_count} * sizeof(**export_arrays));\n    if (export_arrays[{index}] == NULL) {{ status = KILN_C99_HOST_ERROR; goto cleanup; }}",
            )
            .unwrap();
        }
    }
    for (index, (node, module)) in modules.iter().enumerate() {
        if node.source != ModuleSource::CommonJs {
            continue;
        }
        let paths = CommonJsModulePaths::new(graph.project_root(), &node.path)?;
        write_graph_commonjs_shared_setup(
            &mut output,
            index,
            &module.symbol,
            &paths.filename,
            &paths.dirname,
        );
    }
    for (index, (node, _)) in modules.iter().enumerate() {
        for (import_index, import) in node.program.imports.iter().enumerate() {
            match graph_import_value(graph, modules, &indices, node, import)? {
                NativeGraphImport::Internal => {}
                NativeGraphImport::Binding {
                    provider_index,
                    export_index,
                } => {
                    writeln!(
                        output,
                        "    import_arrays[{index}][{import_index}] = export_arrays[{provider_index}][{export_index}];"
                    )
                    .unwrap();
                }
                NativeGraphImport::Namespace { provider_index } => {
                    if modules[provider_index].0.source == ModuleSource::CommonJs {
                        writeln!(
                            output,
                            "    status = kiln_graph_load_commonjs(&commonjs_modules[{provider_index}], &module_result, &module_result);\n    if (status != KILN_C99_OK) {{ *result = module_result; goto cleanup; }}\n    namespaces[{provider_index}] = export_arrays[{provider_index}][0];"
                        )
                        .unwrap();
                    }
                    writeln!(
                        output,
                        "    import_arrays[{index}][{import_index}] = namespaces[{provider_index}];"
                    )
                    .unwrap();
                }
                NativeGraphImport::CommonJsNamed {
                    provider_index,
                    property,
                } => {
                    writeln!(
                        output,
                        "    status = kiln_graph_load_commonjs(&commonjs_modules[{provider_index}], &module_result, &module_result);\n    if (status != KILN_C99_OK) {{ *result = module_result; goto cleanup; }}"
                    )
                    .unwrap();
                    let prop_c = native_c_string(&property);
                    let prop_len = property.len();
                    writeln!(
                        output,
                        "    status = kiln_c99_get(proxy_host, export_arrays[{provider_index}][0], {prop_c}, (size_t){prop_len}, &import_arrays[{index}][{import_index}], 0, &module_result);\n    if (status != KILN_C99_OK) {{ *result = module_result; goto cleanup; }}"
                    )
                    .unwrap();
                }
            }
        }
        let import_count = node.program.imports.len();
        let export_count = native_graph_export_count(node);
        if node.source == ModuleSource::CommonJs {
            writeln!(
                output,
                "    status = kiln_graph_load_commonjs(&commonjs_modules[{index}], &module_result, &module_result);\n    if (status != KILN_C99_OK) {{ *result = module_result; goto cleanup; }}"
            )
            .unwrap();
            if index == entry_index {
                output.push_str("    *result = module_result;\n");
            }
            writeln!(
                output,
                "    namespaces[{index}] = export_arrays[{index}][0];"
            )
            .unwrap();
            if let Some(plan) = &namespace_plans[index] {
                write_graph_namespace_build(
                    &mut output,
                    index,
                    &modules[index].1.symbol,
                    plan,
                    true,
                );
                output.push_str("    if (status != KILN_C99_OK) goto cleanup;\n");
            }
            continue;
        }
        let shared_builtin = node.source == ModuleSource::NodeBuiltin(NodeBuiltin::Shared);
        if shared_builtin {
            output.push_str("    status = kiln_realm_kiln_shared_hooks(1);\n    if (status != KILN_C99_OK) goto cleanup;\n");
        }
        writeln!(
            output,
            "    module_result = (kiln_value){{ .tag = KILN_VALUE_UNDEFINED, .flags = 0, .len = 0, .bits = 0 }};\n    status = {}(proxy_host, &module_result, import_arrays[{index}], (size_t){import_count}, export_arrays[{index}], (size_t){export_count});",
            modules[index].1.symbol
        )
        .unwrap();
        if shared_builtin {
            output.push_str("    { int hook_status = kiln_realm_kiln_shared_hooks(0); if (status == KILN_C99_OK) status = hook_status; }\n");
        }
        output.push_str(
            "    if (status != KILN_C99_OK) { *result = module_result; goto cleanup; }\n",
        );
        if index == entry_index {
            output.push_str("    *result = module_result;\n");
        }
        if let Some(plan) = &namespace_plans[index] {
            write_graph_namespace_build(&mut output, index, &modules[index].1.symbol, plan, true);
            output.push_str("    if (status != KILN_C99_OK) goto cleanup;\n");
        }
    }
    let opentui_cleanup = if has_any_dynamic_import {
        "\n    kiln_shared_opentui_namespace = (kiln_value){ .tag = KILN_VALUE_UNDEFINED, .flags = 0, .len = 0, .bits = 0 };\n    kiln_shared_node_ffi_namespace = (kiln_value){ .tag = KILN_VALUE_UNDEFINED, .flags = 0, .len = 0, .bits = 0 };"
    } else {
        ""
    };
    output.push_str("cleanup:\n    #undef module_result\n    return status;\n}\n");
    writeln!(
        output,
        r#"
static void {wrapper_symbol}_gc_result_roots(void *heap, void *context, int phase) {{
    if (phase == KILN_GC_PHASE_RESET || phase == KILN_GC_PHASE_LANE_RESET || context == NULL) return;
    (void)kiln_realm_gc_mark_value(heap, context);
}}

static int {wrapper_symbol}_gc_visit_result(void *context, kiln_gc_visit_root_fn visit, void *visit_context) {{
    if (context == NULL) return KILN_C99_OK;
    return visit(visit_context, context, KILN_GC_VISIT_VALUE);
}}

static int {wrapper_symbol}_realm_entry(void *opaque) {{
    {wrapper_symbol}_realm_context *context = ({wrapper_symbol}_realm_context *)opaque;
    int status;
    if (context == NULL) return KILN_C99_INVALID_ARGUMENT;
    status = kiln_realm_gc_register_roots_with_visitor({wrapper_symbol}_gc_result_roots,
                                          {wrapper_symbol}_gc_visit_result, context->result,
                                          KILN_GC_ROOT_SURVIVES_TEARDOWN);
    if (status != KILN_C99_OK) return status;
    return {wrapper_symbol}_body(context);
}}

static int {impl_symbol}(kiln_host *host, kiln_value *result, int preserve_result) {{
    {wrapper_symbol}_realm_context context;
    uint64_t realm = 0;
    kiln_string *preserved = NULL;
    int status;
    int destroy_status;
    if (result == NULL) return KILN_C99_INVALID_ARGUMENT;
    memset(&context, 0, sizeof(context));
    context.result = result;
    context.arena = (kiln_proxy_arena){{ NULL, 0, 0 }};
    context.proxy = (kiln_proxy_host){{ host, NULL, &context.arena, KILN_C99_OK }};
    context.proxy_host = (kiln_host){{ kiln_proxy_call, &context.proxy }};
    status = kiln_realm_create(&realm);
    if (status == KILN_C99_OK)
        status = kiln_realm_enter(realm, {wrapper_symbol}_realm_entry, &context);
    if (status == KILN_C99_OK)
        status = kiln_realm_drive(realm, &context.proxy_host, result);
    /* Public v1 results remain borrowed; private v2 calls preserve strings across cleanup. */
    if (preserve_result != 0 && result->tag == KILN_VALUE_STRING) {{
        if (result->len == (uint32_t)-1 || (result->len != 0 && result->ptr == NULL)) {{
            *result = (kiln_value){{ .tag = KILN_VALUE_UNDEFINED, .flags = 0, .len = 0, .bits = 0 }};
            status = KILN_C99_INVALID_ARGUMENT;
        }} else {{
            const kiln_string *src_str = (const kiln_string *)result->ptr;
            size_t storage_size = 0;
            uint32_t flags = src_str != NULL ? src_str->flags : KILN_STRING_LATIN1;
            uint32_t unit_len = (uint32_t)result->len;
            if (!kiln_string_storage_size(unit_len, flags, &storage_size)) {{
                *result = (kiln_value){{ .tag = KILN_VALUE_UNDEFINED, .flags = 0, .len = 0, .bits = 0 }};
                status = KILN_C99_INVALID_ARGUMENT;
            }} else {{
                preserved = (kiln_string *)malloc(storage_size);
                if (preserved == NULL) {{
                    *result = (kiln_value){{ .tag = KILN_VALUE_UNDEFINED, .flags = 0, .len = 0, .bits = 0 }};
                    status = KILN_C99_HOST_ERROR;
                }} else {{
                    preserved->unit_len = unit_len;
                    preserved->flags = flags;
                    preserved->data = (const void *)(preserved + 1);
                    size_t unit_bytes = (size_t)unit_len * (flags == KILN_STRING_UTF16 ? sizeof(uint16_t) : sizeof(uint8_t));
                    if (unit_bytes != 0 && src_str != NULL && src_str->data != NULL) {{
                        memcpy((void *)preserved->data, src_str->data, unit_bytes);
                    }}
                }}
            }}
        }}
    }}
    if (realm != 0) {{
        destroy_status = kiln_realm_destroy(realm);
        if (status == KILN_C99_OK) status = destroy_status;
    }}
    kiln_shared_namespaces = NULL;
    kiln_shared_commonjs = NULL;{opentui_cleanup}
    for (size_t cleanup_index = 0; cleanup_index < (size_t){count}; ++cleanup_index) {{
        free(context.shared_import_arrays == NULL ? NULL : context.shared_import_arrays[cleanup_index]);
        free(context.shared_export_arrays == NULL ? NULL : context.shared_export_arrays[cleanup_index]);
    }}
    free(context.shared_import_arrays);
    free(context.shared_export_arrays);
    free(context.shared_namespaces);
    free(context.shared_commonjs_modules);
    kiln_proxy_arena_clear(&context.arena);
    if (preserved != NULL) result->ptr = (const char *)preserved;
    result->flags = 0;
    return status;
}}

int {wrapper_symbol}(kiln_host *host, kiln_value *result) {{
    return {impl_symbol}(host, result, 0);
}}
"#,
        count = modules.len(),
    )
    .unwrap();
    let invocation = format!("{impl_symbol}(&proxy_host, &value, 1)");
    output.push_str(&owned_result_wrapper_source(
        wrapper_symbol,
        "graph",
        &invocation,
        "if (value.tag == KILN_VALUE_STRING) { free((void *)value.ptr); value.ptr = NULL; value.len = 0; }",
    ));
    Ok(output.replace(
        "UINT16_C(0x7FF0)",
        &format!("UINT16_C(0x{:04X})", C99_HOST_MODULE_CREATE_REQUIRE),
    ))
}
