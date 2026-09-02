
/* This layout is the complete boundary value.  bits stores booleans and IEEE-754 numbers;
 * ptr/len stores UTF-8 string bytes and is not required to be NUL terminated. */
typedef struct kiln_value {
    uint32_t tag;
    uint32_t len;
    union { uint64_t bits; const char *ptr; };
} kiln_value;

typedef enum kiln_typed_kind {
    KILN_TYPED_INT8,
    KILN_TYPED_UINT8,
    KILN_TYPED_UINT8_CLAMPED,
    KILN_TYPED_INT16,
    KILN_TYPED_UINT16,
    KILN_TYPED_INT32,
    KILN_TYPED_UINT32,
    KILN_TYPED_FLOAT32,
    KILN_TYPED_FLOAT64
} kiln_typed_kind;

typedef struct kiln_typed_buffer {
    uint8_t *bytes;
    size_t byte_length;
} kiln_typed_buffer;

typedef struct kiln_typed_array {
    kiln_typed_buffer *buffer;
    size_t byte_offset;
    size_t length;
    kiln_typed_kind kind;
    /* Node's Buffer is a Uint8Array with a different prototype surface, not another byte
     * container. Keep that distinction on the view so slices can share storage and every
     * numeric typed-array operation continues to use the battle-tested path above it. */
    int is_buffer;
} kiln_typed_array;


/* A nonzero callback return is propagated as KILN_C99_HOST_ERROR.  The callback owns any
 * interpretation of operation numbers and may replace *result. */
typedef int (*kiln_host_call_fn)(void *context, uint16_t operation, uint16_t argc,
                                 const kiln_value *arguments, kiln_value *result);
typedef struct kiln_host {
    kiln_host_call_fn call;
    void *context;
} kiln_host;

typedef void (*kiln_drop_opaque_fn)(void *pointer);
extern int kiln_realm_callable_store(uint32_t tag, uint64_t code, void *candidate,
                                     kiln_drop_opaque_fn drop_candidate, void **out_store);
extern int kiln_realm_value_slot(uint32_t key, void **out_slot);
extern int kiln_realm_entropy_word(uint64_t seed, uint64_t *out_word);
/* The collector's seam with the runtime archive (ADR 0041). The prelude owns the heap and the
 * tracer; the archive owns the turn boundary and the roots it holds. */
typedef void (*kiln_gc_roots_fn)(void *heap, void *context, int phase);
typedef struct kiln_gc_hooks {
    uint32_t version;
    uint32_t size;
    void (*collect)(void *heap, int force);
    void (*teardown_begin)(void *heap);
    void (*teardown_end)(void *heap);
    void (*mark_value)(void *heap, const void *value);
    void (*mark_block)(void *heap, void *pointer);
} kiln_gc_hooks;
extern void *kiln_realm_gc_current_heap(void);
extern int kiln_realm_gc_attach(const kiln_gc_hooks *hooks, void *candidate, void **out_heap);
extern int kiln_realm_gc_register_roots(kiln_gc_roots_fn roots, void *context, uint32_t flags);
extern int kiln_realm_gc_mark_roots(void *heap);

/* A closure is its body plus the boxes it captured.
 *
 * The body is a function pointer rather than an index into the emitting program's table,
 * because a function does not have to be called by the unit that made it: a module exports one,
 * another module imports and calls it, and an index would then be resolved against the wrong
 * table.  `proto` is kept only to name the function in text -- it is the same index the
 * interpreter uses, so both engines print `[Function 3]` for the same function. */
struct kiln_closure;
struct kiln_generator;
typedef int (*kiln_body_fn)(kiln_host *host, kiln_value *result, kiln_value *upvalues,
                            kiln_value receiver, const kiln_value *args, size_t argc, int depth,
                            kiln_value *thrown, struct kiln_generator *frame);
typedef struct kiln_closure {
    kiln_body_fn body;
    /* An arrow captures `new.target` where it was written, for the same reason it captures
     * `this`: it has none of its own. */
    int arrow;
    /* Whether this body is strict code, which is a property of the text it was written in and
     * not of the call. It decides one thing here: `OrdinaryCallBindThis`. A sloppy body called
     * with no receiver runs with the global object as `this`, which is how `var root = this ||
     * global` finds the realm; a strict one runs with the `undefined` it was passed. */
    int strict;
    int publishes_frame;
    kiln_value captured_this;
    kiln_value captured_new_target;
    /* ...and its `arguments`, which is the enclosing function's list rather than its own. */
    kiln_value captured_arguments;
    /* Calling this builds a generator object instead of running the body. */
    int generator;
    /* ...and, when this is set, drives it to completion against a promise rather than handing
     * the object to the program. */
    int asynchronous;
    /* ...unless it is *both*, in which case the object is handed over after all and each `next`
     * gets a promise of its own. */
    int async_generator;
    /* What a generator's frame has to be big enough for, known when the body is emitted. */
    uint32_t local_count;
    uint32_t stack_size;
    uint32_t handler_depth;
    /* The object `new f()` links instances to, and the store `f.x = 1` writes into. Both are
     * created on first use: an ordinary function that is never used as a constructor and never
     * carries a property allocates neither. */
    kiln_value prototype;
    kiln_value properties;
    /* For the console, which prints `[Function: name]`.  NULL for an anonymous function, which
     * prints `[Function (anonymous)]` -- a distinct answer, not a missing one. */
    const char *name;
    size_t name_len;
    /* Immutable source ownership and what `Function.prototype.toString` answers: this function's
     * own text, pointing into the
     * emitting unit's one copy of its source rather than owning a copy.  A closure outlives the
     * frame that made it and crosses unit boundaries, but the unit's static text outlives both.
     * NULL for a closure the runtime made for itself, which has no source and says so. Keeping
     * this beside the body pointer means diagnostics and future stack traces need no map,
     * allocation, or ambient current-module state. */
    const char *source;
    size_t source_len;
    /* Immutable module ownership for diagnostics. This is deliberately distinct from `source`:
     * the latter is the function's exact source slice, not the module identifier. */
    const char *diagnostic_source;
    /* Declared parameters, which is what `f.length` answers and what `bind` subtracts from.
     * Counted where the body was compiled, because that is the only place it is known. */
    uint32_t arity;
    /* The `resume` position meaning "the parameters are bound and the body has not started".
     *
     * Binding them is part of the *call*, not of the first resumption: the language runs
     * FunctionDeclarationInstantiation and only then hands back the generator object, so
     * `function* g([{x}]) {}` called as `g([null])` throws where it is written and a parameter
     * default's side effect happens whether or not anyone ever steps the generator.
     *
     * Zero when there is nothing to run at the call: an ordinary function, a generator with no
     * pattern, default or rest to bind, and a plain async function -- whose instantiation
     * failure *rejects* rather than throwing, which is already what running the prologue inside
     * the body gives. */
    uint32_t prologue_resume;
    uint32_t proto;
    uint32_t upvalue_count;
    kiln_value *upvalues;
} kiln_closure;

/* Properties attached to runtime-provided callables. Method identity is its append-only numeric
 * code, so one realm-owned store per code gives `Stream.Readable`, `Buffer.constants`, and future
 * native constructor statics ordinary property behavior without changing the scalar value ABI or
 * pretending a method id is a closure pointer. */
typedef struct kiln_method_store {
    uint64_t code;
    kiln_value properties;
    struct kiln_method_store *next;
} kiln_method_store;

typedef enum kiln_native_prototype_surface {
    KILN_C99_NATIVE_EVENT_EMITTER,
    KILN_C99_NATIVE_READABLE,
    KILN_C99_NATIVE_WRITABLE,
    KILN_C99_NATIVE_SERVER_RESPONSE
} kiln_native_prototype_surface;

/* Generated from the checked Rust constructor graph. The prototype value is the only mutable
 * field: constructor identity, parentage, and surface cannot drift from the compiler registry. */
typedef struct kiln_native_constructor {
    uint64_t method;
    uint64_t parent;
    kiln_native_prototype_surface surface;
    kiln_value prototype;
} kiln_native_constructor;

/* KILN_NATIVE_CONSTRUCTOR_TABLE */

/* A plain data object: own string-keyed properties, scanned linearly.
 *
 * There is deliberately no shape here.  A shape identity has to be globally stable and never
 * recycled to be sound (see ADR 0021), and inventing one before there is anything to guard is
 * how that goes wrong; a list has no identity to get wrong.  It is also uniformly slow rather
 * than fast-until-megamorphic, which is the honest starting point.
 *
 * Keys point at the generated constant table or at heap bytes from a computed key, and are
 * compared by content, never by pointer -- two spellings of one name must be one property. */
/* A property is a value, or a pair of functions.  Reading an accessor *calls* one, which is why
 * every program-visible read has to go through the runtime rather than being a struct field
 * access -- and why `kiln_c99_get` takes a host and a depth. */
typedef struct kiln_property {
    const char *key;
    size_t key_len;
    /* Whether `Object.keys`, `for...in`, spread and JSON see it. Every own property of an error
     * is readable by name and not enumerated, which is why `JSON.stringify(err)` is `{}`. */
    int enumerable;
    /* Whether an assignment may replace the value, and whether `delete` may remove the property
     * or `defineProperty` redefine it. Both are true for everything an assignment or a literal
     * creates, which is why they went unmodelled for so long: only `Object.defineProperty` and
     * `Object.freeze` can produce a property where they are not, and only
     * `getOwnPropertyDescriptor` reports them back. Absent, a defined non-writable property took
     * writes and a non-configurable one deleted -- both silently. */
    int writable;
    int configurable;
    int accessor;
    kiln_value value;
    kiln_value getter;
    kiln_value setter;
} kiln_property;

typedef struct kiln_object {
    kiln_property *properties;
    size_t count;
    size_t capacity;
    /* The next object a property read falls through to, or undefined for none.  Undefined and
     * null are different answers here, exactly as they are to `Object.getPrototypeOf`: one means
     * nothing was recorded and the other means nothing was wanted. */
    kiln_value prototype;
    /* `Object.freeze`. A frozen object ignores every write rather than refusing one: modules are
     * strict and a real engine would throw, but the interpreter is silent here and the two have
     * to agree before either can change. */
    int frozen;
    /* `Object.preventExtensions`. Separate from `frozen`, because it is strictly weaker: a
     * non-extensible object still takes writes to the properties it already has, and only
     * refuses to grow. `Object.isFrozen` on one is false, which is exactly the distinction a
     * library checks before deciding an object is safe to mutate in place. */
    int not_extensible;
    /* Module namespace objects are non-extensible and reject [[Set]] even though their exported
     * data properties report writable=true. This is deliberately separate from `frozen`: a
     * namespace is not an ordinary frozen object, and reflection must preserve that distinction. */
    int module_namespace;
    /* The `arguments` object of a call. An internal slot rather than a `Symbol.toStringTag`
     * property, because the tag is not a property: neither `getOwnPropertyNames` nor
     * `getOwnPropertySymbols` may report it, and a program must not be able to delete it. */
    int arguments_brand;
    /* How many of `properties` currently have `accessor` set. `kiln_c99_put_mode`'s chain walk
     * exists to find an inherited setter; when a prototype level's count is zero, it has none,
     * and the level costs one pointer follow instead of a scan of its whole property list.
     *
     * A level with `accessor_count == 0` can still hold a non-writable *data* property under the
     * same key, which must refuse the shadowing write; `read_only_seen` below covers that, so
     * the two together make the skip exact rather than a bet on what the Node surface defines.
     *
     * Kept exact by every site that flips a property's `accessor` bit, so it is never a scan in
     * disguise. */
    size_t accessor_count;
    /* Set once a data property of this object has been made non-writable, and never cleared:
     * with `accessor_count`, it is what lets `kiln_c99_put_mode` skip a prototype level's scan
     * exactly. A level that never held an accessor or a read-only data property has nothing
     * that could intercept a write from a descendant. */
    int read_only_seen;
} kiln_object;

/* A prototype chain is built by the program, so its length is the program's business -- but the
 * walk is on the C stack, where running out aborts rather than reports. Bounded, like the
 * interpreter's. */
#define KILN_C99_MAX_PROTOTYPE_DEPTH 10000

/* A compiled named-access site's memory of where its key was last found (ADR 0047): a slot on
 * the receiver, or a slot on the receiver's immediate prototype. Objects have no shape, so a
 * hit is validated by the key itself: keys are unique within an object and canonical across
 * the realm, so `properties[own].key == key` proves the slot *is* the property, on any object
 * whatsoever -- and objects built by the same code put their keys at the same indices, which is
 * what makes one guess right for every instance of a class that does not exist. One of these is
 * a block-scope static at each GetProperty, SetProperty, SetPropertyStrict and CallMethod. */
typedef struct kiln_c99_site {
    uint32_t own;
    uint32_t proto;
    /* Slow-path answers that taught the site nothing: a name answered from a builtin table or a
     * deeper prototype. After `KILN_C99_SITE_GIVE_UP` of them the site stops scanning to learn. */
    uint32_t misses;
} kiln_c99_site;
#define KILN_C99_SITE_NONE 0xFFFFFFFFu
#define KILN_C99_SITE_GIVE_UP 8u

/* A proxy's two halves.
 *
 * Both are held as ordinary values because both are: a handler is a plain object whose
 * properties are the traps, and a target is whatever was proxied -- including another proxy,
 * which is why neither is narrowed to a kind here. */
typedef struct kiln_proxy {
    kiln_value target;
    kiln_value handler;
    /* `Proxy.revocable`'s second half was called. A revoked proxy is not an empty one: every
     * operation on it is a TypeError, which is the entire point -- the capability it stood for
     * has been withdrawn, and a program holding a stale reference has to find out rather than
     * see a plausible answer. */
    int revoked;
} kiln_proxy;

/* An array's elements are dense and its `length` is derived, not stored: writing past the end
 * grows it and the gap is filled with holes, which read as undefined and are not the same thing --
 * `1 in [1, , 3]` is false and `[1, , 3].map(f)` does not call `f` for the middle. */
typedef struct kiln_array {
    kiln_value *elements;
    size_t count;
    size_t capacity;
    /* Properties that are not elements: `a.total = 1`, and the `index`, `input` and `groups` a
     * match result carries. Undefined until the first one is written, so an ordinary array is one
     * pointer smaller than it looks and costs nothing. */
    kiln_value properties;
    /* `Object.freeze`. An element write is ignored, like a property write on a frozen object;
     * a method that would change the length throws, because there is no way to do half of it. */
    int frozen;
    /* `Object.preventExtensions`. An existing index still takes a write; a new one does not. */
    int not_extensible;
} kiln_array;

/* A suspended body.
 *
 * A generator does not run when it is called: it hands back this, and each `next` resumes the
 * body where it left off. An automatic C frame cannot be suspended -- it is gone the moment the
 * function returns -- so a generator's locals and operand stack live here instead, and the body
 * is emitted to read them from here rather than from its own frame.
 *
 * Re-entry is a `switch` on `resume` at the top of the body, jumping to a label emitted directly
 * after the `Yield` that produced it. That works because the emitted body is already a flat
 * sequence of labels with `goto` between them: the shape a bytecode interpreter has, and the
 * shape that makes resuming possible at all. */
typedef struct kiln_generator {
    kiln_value function;
    kiln_value receiver;
    /* An async function is compiled as a generator -- `await` suspends exactly as `yield` does --
     * and this is the promise its eventual return value settles. Undefined for a plain one. */
    kiln_value promise;
    int is_async;
    kiln_value *locals;
    kiln_value *stack;
    size_t sp;
    /* Which `Yield` to come back to. Zero before the first `next`, when the body starts at its
     * beginning rather than resuming. */
    size_t resume;
    /* The handler stack, which is part of the state a suspension has to survive: a `try` that
     * stopped catching across a `yield` would surface three frames away. Owned here rather than
     * being automatic, for exactly the reason the operand stack is. */
    size_t *handlers;
    size_t *handler_stack;
    size_t handler_top;
    /* What `next(v)` passed in, which becomes the result of the `yield` that suspended. */
    kiln_value sent;
    /* Whether the suspension that stopped this frame was an `await` rather than a `yield`.
     *
     * They emit identically -- both save the frame and return `YIELDED` -- and for an ordinary
     * generator or an async function nothing has to tell them apart. An async generator does:
     * an `await` between two `yield`s is invisible to the consumer, because the step is not
     * finished. */
    int awaiting;
    int async_generator;
    /* Whether `sent` is a value to resume with or a rejection to raise at the suspension point.
     * A rejected `await` throws *into* the body, so a `try` around it catches -- which is the
     * whole reason the handler stack above has to survive a suspension. */
    int raising;
    /* Whether this entry is the parameter binding rather than the body. Set only for the call
     * that builds the generator, and read at the seam between the two halves: the emitted body
     * is one flat instruction sequence, so the only thing that can stop it there is a test. */
    int binding;
    int started;
    int done;
    /* Whether the body is on the C stack right now.
     *
     * A generator has one frame, and `next` resumes *that* frame -- so a `next` from inside the
     * body would resume a frame that is already running, overwriting the locals and operand stack
     * the running body is standing on. The language answers "Generator is already running"
     * instead, and this is what lets the answer be given: before it, the re-entrant call clobbered
     * the frame and the process ended with an exit code and no words. */
    int running;
} kiln_generator;

/* The parts a civil date is made of. The realm has one zone and it is UTC (ADR 0015), so there
 * is no local variant of any of these to keep in step. */
typedef struct kiln_date_parts {
    int64_t year;
    int month;
    int day;
    int weekday;
    int hour;
    int minute;
    int second;
    int millisecond;
    int valid;
} kiln_date_parts;

/* A Map or a Set.
 *
 * Insertion-ordered and scanned linearly, for the same reason an object's properties are: a hash
 * needs a hash of a *value*, and the identity half of that is a pointer the collector will one
 * day move. Order is observable -- iteration is in insertion order and the specification says so
 * -- so the list is the honest structure and the index is what a hash would accelerate. */
typedef struct kiln_entry {
    kiln_value key;
    kiln_value value;
    /* A deleted entry is a hole, not a removal: compacting here would shift every later entry
     * past a live iterator's cursor and skip one. */
    int dead;
} kiln_entry;

typedef struct kiln_collection {
    kiln_entry *entries;
    /* How many slots have ever been used, dead ones included -- what iteration walks. */
    size_t count;
    /* How many are still there, which is what `size` reports. */
    size_t live;
    size_t capacity;
    int is_map;
    /* Weak collections share the identity-keyed storage and methods, but deliberately expose
     * neither size nor iteration. The native runtime has no tracing collector yet, so entries
     * remain strongly retained for this process lifetime; the observable WeakMap/WeakSet API is
     * nevertheless correct and this flag prevents accidentally advertising Map semantics. */
    int is_weak;
    /* Where `class Registry extends Map {}` puts the prototype its instances inherit from, and
     * where a write that is not an entry goes. Undefined until something needs one, so an
     * ordinary Map costs nothing for it. */
    kiln_value properties;
} kiln_collection;

/* A promise: a value that arrives once, later, and everything waiting for it.
 *
 * `state` is pending, fulfilled or rejected, and only the first settlement counts -- a promise is
 * handed to the program as two functions and the program may call either twice. */
typedef struct kiln_reaction {
    kiln_value on_fulfilled;
    kiln_value on_rejected;
    /* The promise `then` returned, settled with whatever the handler produces. */
    kiln_value derived;
} kiln_reaction;

typedef struct kiln_promise {
    int state;
    kiln_value value;
    kiln_reaction *reactions;
    size_t count;
    size_t capacity;
} kiln_promise;

#define KILN_C99_PENDING 0
#define KILN_C99_FULFILLED 1
#define KILN_C99_REJECTED 2

typedef struct kiln_settler {
    kiln_value promise;
    int rejects;
} kiln_settler;

/* A compiled pattern, its source text, and the cursor `g` and `y` advance.
 *
 * The engine is behind the provider ABI below rather than written here (ADR 0025): it is the same
 * `regress` the interpreter runs, so the two cannot disagree about a backreference. `last_index`
 * is mutable and program-visible, which is why two evaluations of one literal must not share a
 * `kiln_regexp`. */
typedef struct kiln_regexp {
    void *handle;
    const char *source;
    size_t source_len;
    const char *flags;
    size_t flags_len;
    int global;
    int sticky;
    double last_index;
} kiln_regexp;

/* A symbol: a property key with an identity of its own.
 *
 * `key` is the reserved string the symbol names a property under, so the object model stays
 * string-keyed and nothing else has to learn about symbols. A well-known symbol gets a fixed key,
 * so two mentions of `Symbol.iterator` reach the same property; a unique one gets a serial, so two
 * symbols with the same description do not collide and a program cannot spell either by writing a
 * string. The description is for display and is never part of the key. */
typedef struct kiln_symbol {
    const char *key;
    size_t key_len;
    const char *description;
    size_t description_len;
    /* Non-zero only for a symbol from the cross-realm registry. It lived in the value's payload
     * word until that word was shared with the pointer; a registered symbol is the one value that
     * wanted both, and the id belongs with the symbol rather than beside it. */
    uint64_t registry_id;
} kiln_symbol;

typedef struct kiln_bound {
    kiln_value target;
    kiln_value receiver;
    /* `f.bind(o, 1)` fixes the 1 as well as the `o`: the bound arguments come first and the
     * call's own follow them. Owned here, because the call that supplies the rest happens
     * arbitrarily later. */
    kiln_value *arguments;
    size_t count;
    /* Bound functions are ordinary callable objects: libraries attach metadata to wrappers
     * returned by `bind`. Keep that own store on the value, not on its target, so two bindings
     * of one function do not share statics. */
    kiln_value properties;
} kiln_bound;

typedef int (*kiln_commonjs_require_fn)(void *context, kiln_host *host, kiln_value specifier,
                                        kiln_value *result, kiln_value *thrown);
typedef struct kiln_commonjs_require {
    kiln_commonjs_require_fn call;
    void *context;
} kiln_commonjs_require;

/* One entry of the microtask queue: run `handler` against `argument` and settle `derived` with
 * what it produced.  A handler that is not callable passes the value through unchanged, which is
 * what `p.then(undefined, f)` and `p.catch(f)` rely on. */
typedef struct kiln_microtask {
    kiln_value handler;
    kiln_value argument;
    kiln_value derived;
    int rejected;
} kiln_microtask;

/* One process.nextTick callback. The Realm Runtime owns lane ordering; this payload contains
 * only the language values its opaque work callback understands. */
typedef struct kiln_next_tick {
    kiln_value callback;
    kiln_value *args;
    size_t argc;
} kiln_next_tick;

/* One generated callback payload. The Realm Runtime owns its timer identity, deadline, sequence,
 * repeat policy and cancellation; this allocation contains only the language state its opaque
 * work callback understands. */
typedef struct kiln_timer {
    kiln_value callback;
    kiln_value *args;
    size_t argc;
} kiln_timer;
typedef struct kiln_node_server kiln_node_server;
typedef struct kiln_node_socket kiln_node_socket;

struct kiln_node_server {
    kiln_host *host;
    kiln_value object;
    kiln_value connection_listener;
    uint64_t listener;
    uint64_t listen_request;
    uint64_t accept_request;
    char *address;
    size_t address_len;
    uint32_t port;
    uint64_t timeout_milliseconds;
    int protocol_http;
    int closing;
    int closed;
    kiln_node_socket *sockets;
};

struct kiln_node_socket {
    kiln_host *host;
    kiln_value object;
    kiln_node_socket *next;
    uint64_t resource;
    uint64_t connect_request;
    uint64_t read_request;
    uint64_t write_request;
    uint64_t shutdown_request;
    uint64_t close_request;
    kiln_node_server *server;
    struct kiln_node_client_request *client_requests;
    uint64_t next_client_request_id;
    struct kiln_node_response *responses;
    int close_after_write;
    int close_after_responses;
    int shutdown_after_write;
    int utf8;
    int closing;
    int closed;
};

typedef struct kiln_node_client_request {
    kiln_host *host;
    kiln_value object;
    kiln_value callback;
    kiln_value response;
    kiln_node_socket *socket;
    char *host_name;
    size_t host_name_len;
    char *path;
    size_t path_len;
    char *method;
    size_t method_len;
    char *headers;
    size_t headers_len;
    char *pending_body;
    size_t pending_body_len;
    size_t pending_body_capacity;
    uint16_t port;
    uint64_t connect_request;
    uint64_t request_id;
    int ended;
    int sent;
    int response_started;
    int response_ended;
    int utf8;
    struct kiln_node_client_request *next;
} kiln_node_client_request;

static int kiln_c99_node_client_try_start(kiln_node_client_request *request);
static int kiln_c99_node_socket_object(kiln_node_socket *socket, kiln_value *thrown);

static kiln_node_client_request *kiln_c99_node_client_for_id(kiln_node_socket *socket,
                                                              uint64_t id) {
    kiln_node_client_request *request;
    if (socket == NULL || id == 0) return NULL;
    for (request = socket->client_requests; request != NULL; request = request->next) {
        if (request->request_id == id) return request;
    }
    return NULL;
}

typedef struct kiln_node_response {
    kiln_node_socket *socket;
    uint64_t request_id;
    kiln_value request;
    kiln_value object;
    kiln_value headers;
    /* The request's headers as the runtime flattened them: NUL-separated name/value pairs,
     * copied because the event that carried them is the runtime's and does not outlive the
     * callback.  A malloc'd inner buffer of this block, released by its finaliser (ADR 0041) or
     * as soon as `req.headers` is read and the object it describes exists instead. */
    char *request_headers;
    size_t request_headers_len;
    uint16_t status;
    int started;
    int ended;
    struct kiln_node_response *next;
} kiln_node_response;

static void kiln_c99_node_unlink_response(kiln_node_response *response) {
    kiln_node_response **cursor;
    if (response == NULL || response->socket == NULL) return;
    cursor = &response->socket->responses;
    while (*cursor != NULL) {
        if (*cursor == response) {
            *cursor = response->next;
            response->next = NULL;
            return;
        }
        cursor = &(*cursor)->next;
    }
}

/* A server keeps its accepted sockets in a list so that `server.close()` can drain them and so
 * that the last one to close is what emits the server's own `close`. A socket that has closed
 * answers neither question, and leaving it linked would make the server retain every connection
 * it ever accepted -- one live block per request, which is exactly the growth ADR 0041 exists to
 * remove. Unlinking is what "all remaining sockets are open" means, so the walks above read the
 * same either way. */
static void kiln_c99_node_unlink_socket(kiln_node_socket *socket) {
    kiln_node_socket **cursor;
    if (socket == NULL || socket->server == NULL) return;
    cursor = &socket->server->sockets;
    while (*cursor != NULL) {
        if (*cursor == socket) {
            *cursor = socket->next;
            socket->next = NULL;
            return;
        }
        cursor = &(*cursor)->next;
    }
}

static int kiln_c99_node_has_unended_response(const kiln_node_socket *socket) {
    const kiln_node_response *response;
    if (socket == NULL) return 0;
    for (response = socket->responses; response != NULL; response = response->next) {
        if (!response->ended) return 1;
    }
    return 0;
}

/* A response head the script already framed. Node leaves a script-supplied Content-Length or
 * Transfer-Encoding alone; header names arrive from setHeader lowercased and from writeHead as
 * written, so the comparison is case-insensitive. */
static int kiln_c99_node_response_is_framed(const kiln_object *headers) {
    static const char *const framing[2] = {"content-length", "transfer-encoding"};
    size_t index;
    size_t which;
    if (headers == NULL) return 0;
    for (index = 0; index < headers->count; ++index) {
        const kiln_property *property = &headers->properties[index];
        for (which = 0; which < 2; ++which) {
            size_t len = strlen(framing[which]);
            size_t byte;
            if (property->key_len != len) continue;
            for (byte = 0; byte < len; ++byte) {
                if ((char)tolower((unsigned char)property->key[byte]) != framing[which][byte])
                    break;
            }
            if (byte == len) return 1;
        }
    }
    return 0;
}

typedef struct kiln_node_completion {
    uint32_t kind;
    kiln_node_server *server;
    kiln_node_socket *socket;
    kiln_node_client_request *client_request;
} kiln_node_completion;

/* One Node fs/promises request. The Promise and conversion mode stay in generated C; the Realm
 * owns this allocation from submission through completion/cancellation and invokes the adapter
 * only on the realm task. */
typedef struct kiln_node_fs_completion {
    kiln_value promise;
    uint32_t operation;
    int utf8;
} kiln_node_fs_completion;

/* A cursor over JSON source, moved forward as text is consumed. */
typedef struct kiln_json_reader {
    const char *bytes;
    size_t len;
    size_t at;
    int depth;
} kiln_json_reader;

/* A growable byte buffer, for the operations that build a string rather than slice one. */
typedef struct kiln_text {
    char *bytes;
    size_t len;
    size_t capacity;
} kiln_text;

/* Internal link-time runtime contract. The final executable/shared library supplies these symbols
 * from the Rust provider archive; they are not part of the public embedding ABI.
 *
 * The offsets `kiln_regexp_search` reports are UTF-16 code units, because that is what the
 * language counts in (ADR 0022) and matching over UTF-16 makes them so without a conversion. */
int kiln_number_to_js_string(double value, char *buffer, size_t capacity);
double kiln_string_to_number(const char *bytes, size_t len);
typedef int (*kiln_realm_work_fn)(void *work, void *host, void *thrown);
typedef void (*kiln_realm_drop_fn)(void *work);
typedef int (*kiln_realm_entry_fn)(void *context);
#define KILN_REALM_ASYNC_ABI_VERSION 5
#define KILN_REALM_ASYNC_OK 0
#define KILN_REALM_ASYNC_ERROR 1
#define KILN_REALM_ASYNC_NOT_FOUND 1
#define KILN_REALM_ASYNC_UNDEFINED 0
#define KILN_REALM_ASYNC_BYTES 1
#define KILN_REALM_ASYNC_FILE_STAT 2
#define KILN_REALM_ASYNC_RESOURCE 3
#define KILN_REALM_ASYNC_RESOURCE_STATE 4
#define KILN_REALM_ASYNC_ACCEPTED_STREAM 5
#define KILN_REALM_OP_READ_FILE 1
#define KILN_REALM_OP_WRITE_FILE 2
#define KILN_REALM_OP_STAT 3
#define KILN_REALM_OP_CLOSE_RESOURCE 4
#define KILN_REALM_OP_TCP_LISTEN 7
#define KILN_REALM_OP_TCP_ACCEPT 8
#define KILN_REALM_OP_TCP_CONNECT 9
#define KILN_REALM_OP_TCP_READ 10
#define KILN_REALM_OP_TCP_WRITE 11
#define KILN_REALM_OP_TCP_SHUTDOWN 12
#define KILN_REALM_HTTP_CLOSE_GRACEFUL 0
#define KILN_REALM_HTTP_CLOSE_ABORT 1
typedef struct kiln_async_host_op {
    uint32_t operation;
    uint32_t flags;
    uint64_t resource;
    uint64_t argument;
    const uint8_t *text;
    uint64_t text_len;
    const uint8_t *bytes;
    uint64_t bytes_len;
} kiln_async_host_op;
typedef struct kiln_async_completion {
    uint64_t request;
    uint64_t sequence;
    int32_t status;
    uint32_t value_kind;
    const uint8_t *bytes;
    uint64_t bytes_len;
    uint64_t value_u64;
    uint64_t resource;
    uint32_t resource_kind;
    uint32_t resource_port;
    uint32_t resource_flags;
    const uint8_t *address;
    uint64_t address_len;
    const uint8_t *peer_address;
    uint64_t peer_address_len;
    uint32_t peer_port;
    const uint8_t *error_code;
    uint64_t error_code_len;
    const uint8_t *error_syscall;
    uint64_t error_syscall_len;
    const uint8_t *error_path;
    uint64_t error_path_len;
    const uint8_t *error_message;
    uint64_t error_message_len;
} kiln_async_completion;
typedef int (*kiln_realm_completion_fn)(void *work, const kiln_async_completion *completion,
                                       void *host, void *thrown);
int kiln_realm_create(uint64_t *out_realm);
int kiln_realm_enter(uint64_t realm, kiln_realm_entry_fn entry, void *context);
int kiln_realm_drive(uint64_t realm, void *host, void *thrown);
int kiln_realm_destroy(uint64_t realm);
int kiln_realm_is_active(void);
int kiln_realm_diagnostic_reset(void);
int kiln_realm_diagnostic_note(const uint8_t *site, uint64_t site_len, uint64_t instruction,
                               const uint8_t *operation, uint64_t operation_len, uint64_t depth,
                               const uint8_t *detail, uint64_t detail_len,
                               const uint8_t *function, uint64_t function_len,
                               uint64_t method, uint64_t receiver_tag, int has_method);
int kiln_realm_diagnostic_render(uint8_t *out, uint64_t capacity, uint64_t *out_len);
int kiln_realm_symbol_for(const uint8_t *key, uint64_t key_len, uint64_t *out_id);
int kiln_realm_symbol_key_for(uint64_t id, const uint8_t **out_key, uint64_t *out_key_len);
int kiln_realm_value_slot(uint32_t key, void **out_slot);
/* The command line the launcher's `main` was handed, recorded on the realm before it was
 * entered. A realm nobody launched -- an embedded module, a unit test -- reports zero, which is
 * how `process.argv` stays empty instead of naming an executable that does not exist. */
int kiln_realm_argv_count(uint64_t *out_count);
int kiln_realm_argv_at(uint64_t index, const uint8_t **out_bytes, uint64_t *out_len);
int kiln_realm_entropy_word(uint64_t seed, uint64_t *out_word);
int kiln_realm_now(double *out_milliseconds);
/* The same virtual clock, in whole epoch nanoseconds split across two halves because C99 has no
 * portable 128-bit integer. `Temporal` measures instants in nanoseconds, and rounding the realm's
 * clock through the millisecond accessor above would make the two engines disagree below the
 * millisecond rather than refuse. */
int kiln_realm_now_nanoseconds(int64_t *out_high, uint64_t *out_low);
/* ADR 0031 civil time. Deterministic realms use the virtual accessor above; host realms call
 * HostOps::ClockNow through the explicit host argument. */
static int kiln_c99_wall_clock_milliseconds(kiln_host *host, double *out_milliseconds);
static int kiln_c99_wall_clock_nanoseconds(kiln_host *host, int64_t *out_high, uint64_t *out_low);
int kiln_realm_queue_microtask(kiln_realm_work_fn run, kiln_realm_drop_fn drop, void *work);
int kiln_realm_queue_next_tick(kiln_realm_work_fn run, kiln_realm_drop_fn drop, void *work);
int kiln_realm_set_timeout(double delay_ms, kiln_realm_work_fn run, kiln_realm_drop_fn drop,
                           void *work, uint64_t *out_id);
int kiln_realm_set_interval(double delay_ms, kiln_realm_work_fn run, kiln_realm_drop_fn drop,
                            void *work, uint64_t *out_id);
int kiln_realm_clear_timer(uint64_t id);
int kiln_realm_read_file(uint64_t realm, const uint8_t *path, uint64_t path_len,
                         kiln_realm_completion_fn run, kiln_realm_drop_fn drop, void *work,
                         uint64_t *out_request);
int kiln_realm_start_host_op(uint64_t realm, const kiln_async_host_op *operation,
                             kiln_realm_completion_fn run, kiln_realm_drop_fn drop, void *work,
                             uint64_t *out_request);
int kiln_realm_start_active_host_op(const kiln_async_host_op *operation,
                                    kiln_realm_completion_fn run, kiln_realm_drop_fn drop,
                                    void *work, uint64_t *out_request);
int kiln_realm_cancel_request(uint64_t realm, uint64_t request);
int kiln_realm_cancel_active_request(uint64_t request);
typedef struct kiln_realm_http_event {
    uint32_t kind;
    uint32_t flags;
    uint64_t id;
    uint16_t status;
    uint8_t version_minor;
    uint8_t reserved;
    const uint8_t *method;
    uint64_t method_len;
    const uint8_t *target;
    uint64_t target_len;
    const uint8_t *reason;
    uint64_t reason_len;
    const uint8_t *headers;
    uint64_t headers_len;
    const uint8_t *data;
    uint64_t data_len;
} kiln_realm_http_event;
typedef struct kiln_realm_http_request {
    const uint8_t *method;
    uint64_t method_len;
    const uint8_t *target;
    uint64_t target_len;
    const uint8_t *headers;
    uint64_t headers_len;
    const uint8_t *body;
    uint64_t body_len;
} kiln_realm_http_request;
typedef struct kiln_realm_http_response {
    uint16_t status;
    uint16_t reserved;
    const uint8_t *headers;
    uint64_t headers_len;
} kiln_realm_http_response;
typedef int (*kiln_realm_http_event_fn)(void *work, const kiln_realm_http_event *event,
                                       void *host, void *thrown);
int kiln_realm_http_server_attach_active(uint64_t resource, kiln_realm_http_event_fn run,
                                         kiln_realm_drop_fn drop, void *work);
int kiln_realm_http_server_attach_with_options_active(
    uint64_t resource, int allow_half_close, kiln_realm_http_event_fn run,
    kiln_realm_drop_fn drop, void *work);
