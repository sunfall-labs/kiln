
#if defined(__unix__) || defined(__APPLE__)
#include <sys/ioctl.h>
#endif
#if defined(__APPLE__)
#include <crt_externs.h>
#define kiln_c99_environ (*_NSGetEnviron())
#elif defined(_WIN32)
#define kiln_c99_environ NULL
#else
extern char **environ;
#define kiln_c99_environ environ
#endif

static int kiln_c99_realm_value_slot(uint32_t key, kiln_value **out) {
    void *slot = NULL;
    int status;
    if (out == NULL) return KILN_C99_INVALID_ARGUMENT;
    status = kiln_realm_value_slot(key, &slot);
    if (status != KILN_C99_OK || slot == NULL) return status;
    *out = (kiln_value *)slot;
    return KILN_C99_OK;
}

static int kiln_c99_private_object(kiln_value *out);

/* The same names are registered before the first realm in surface.c. A function-local table
 * escaped the prelude's literal-key registration, so each realm made its own 14-shape chain. */
static const char *const kiln_c99_atomics_names[] = {
    "add", "and", "compareExchange", "exchange", "isLockFree", "load", "notify",
    "or", "pause", "store", "sub", "wait", "waitAsync", "xor",
};

static int kiln_c99_atomics_object(kiln_value *out) {
    kiln_value *atomics;
    size_t index;
    int status = kiln_c99_realm_value_slot(KILN_C99_REALM_SLOT_ATOMICS, &atomics);
    if (status != KILN_C99_OK) return status;
    if (atomics->tag == KILN_VALUE_OBJECT) {
        *out = *atomics;
        return KILN_C99_OK;
    }
    status = kiln_c99_new_object(atomics);
    for (index = 0; status == KILN_C99_OK &&
                    index < sizeof kiln_c99_atomics_names / sizeof *kiln_c99_atomics_names;
         ++index)
        status = kiln_c99_define_method(
            *atomics, kiln_c99_atomics_names[index], strlen(kiln_c99_atomics_names[index]),
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
    kiln_value native_modules = kiln_c99_undefined();
#if KILN_RETAIN_GROUP_NODE_NET
    kiln_value net_module = kiln_c99_undefined();
#endif
    int status = kiln_c99_realm_value_slot(KILN_C99_REALM_SLOT_PROCESS, &process);
    if (status != KILN_C99_OK) return status;
    if (process->tag == KILN_VALUE_OBJECT) {
        *out = *process;
        return KILN_C99_OK;
    }
    status = kiln_c99_new_object(process);
    if (status == KILN_C99_OK)
        status = kiln_c99_define_method(*process, "__kilnFsDescriptor", 18,
                              kiln_c99_method(KILN_C99_METHOD_FS_DESCRIPTOR_RAW));
    /* Runtime capabilities live on this non-enumerable process table, never as global names.
     * The TypeScript Node facades capture and remove the bootstrap-only members before user code
     * runs, while the CommonJS decorator remains available to the graph runtime. */
    if (status == KILN_C99_OK) status = kiln_c99_new_object(&native_modules);
    if (status == KILN_C99_OK) kiln_c99_gc_keep_alive(&native_modules);
    if (status == KILN_C99_OK)
        status = kiln_c99_commit_own_data(*process, "__kilnNativeModules", sizeof("__kilnNativeModules") - 1,
                                          native_modules, 0, 1, 1);
    if (status == KILN_C99_OK)
        status = kiln_c99_commit_own_data(native_modules, "__kilnHostModuleRequireResolve", sizeof("__kilnHostModuleRequireResolve") - 1,
                                          kiln_c99_method(KILN_C99_METHOD_COMMONJS_REQUIRE_RESOLVE),
                                          0, 1, 1);
    if (status == KILN_C99_OK)
        status = kiln_c99_commit_own_data(native_modules, "__kilnHostModuleCreateRequire", sizeof("__kilnHostModuleCreateRequire") - 1,
                                          kiln_c99_method(KILN_C99_METHOD_NODE_MODULE_CREATE_REQUIRE),
                                          0, 1, 1);
    if (status == KILN_C99_OK)
        status = kiln_c99_commit_own_data(native_modules, "__kilnHostModuleLoader", sizeof("__kilnHostModuleLoader") - 1,
                                          kiln_c99_method(KILN_C99_METHOD_MODULE_LOADER_RAW),
                                          0, 1, 1);
#if KILN_RETAIN_GROUP_NODE_CRYPTO
    if (status == KILN_C99_OK)
        status = kiln_c99_commit_own_data(native_modules, "__kilnCryptoKeyImport", sizeof("__kilnCryptoKeyImport") - 1,
                                          kiln_c99_method(KILN_C99_METHOD_CRYPTO_KEY_IMPORT_RAW),
                                          0, 1, 1);
    if (status == KILN_C99_OK)
        status = kiln_c99_commit_own_data(native_modules, "__kilnCryptoKeyPublic", sizeof("__kilnCryptoKeyPublic") - 1,
                                          kiln_c99_method(KILN_C99_METHOD_CRYPTO_KEY_PUBLIC_RAW),
                                          0, 1, 1);
    if (status == KILN_C99_OK)
        status = kiln_c99_commit_own_data(native_modules, "__kilnCryptoKeyInfo", sizeof("__kilnCryptoKeyInfo") - 1,
                                          kiln_c99_method(KILN_C99_METHOD_CRYPTO_KEY_INFO_RAW),
                                          0, 1, 1);
    if (status == KILN_C99_OK)
        status = kiln_c99_commit_own_data(native_modules, "__kilnCryptoKeyExport", sizeof("__kilnCryptoKeyExport") - 1,
                                          kiln_c99_method(KILN_C99_METHOD_CRYPTO_KEY_EXPORT_RAW),
                                          0, 1, 1);
    if (status == KILN_C99_OK)
        status = kiln_c99_commit_own_data(native_modules, "__kilnCryptoKeyFree", sizeof("__kilnCryptoKeyFree") - 1,
                                          kiln_c99_method(KILN_C99_METHOD_CRYPTO_KEY_FREE_RAW),
                                          0, 1, 1);
#endif
    if (status == KILN_C99_OK)
        status = kiln_c99_commit_own_data(native_modules, "__kilnDecorateCommonJsRequire", sizeof("__kilnDecorateCommonJsRequire") - 1,
                                          kiln_c99_undefined(), 0, 1, 1);
    if (status == KILN_C99_OK)
        status = kiln_c99_commit_own_data(native_modules, "__kilnNativeImportMeta", sizeof("__kilnNativeImportMeta") - 1,
                                          kiln_c99_undefined(), 0, 1, 1);
    /* Demand-driven prelude: `process` is an emitter only in builds that
     * retain the event group (see `retained_method_groups`). Reaching this
     * initializer requires the `process` global, which retains the group,
     * so a build without it never constructs `process` this way. */
#if KILN_RETAIN_GROUP_NODE_EVENT
    /* Inherited, as node's are: `process` is an `EventEmitter` instance, so its methods live on
     * `EventEmitter.prototype` and `Object.getPrototypeOf(process).on` is a function. They
     * were installed as own methods until 2026-09-19, which read the same through `process.on`
     * and differently through the prototype. */
    if (status == KILN_C99_OK) {
        kiln_value emitter_prototype;
        status = kiln_c99_callable_prototype(kiln_c99_method(KILN_C99_METHOD_NODE_EVENT_EMITTER),
                                             &emitter_prototype);
        if (status == KILN_C99_OK) status = kiln_c99_set_prototype(*process, emitter_prototype);
    }
#endif
    if (status == KILN_C99_OK) {
        status = kiln_c99_new_object(&env);
        if (status == KILN_C99_OK)
            (void)kiln_c99_sealed_layout_attach_empty(KILN_VALUE_OBJECT,
                                                       KILN_C99_SEALED_ENV_CODE,
                                                       kiln_c99_object_of(env));
    }
    if (status == KILN_C99_OK && kiln_c99_environ != NULL) {
        for (char **entry_ptr = kiln_c99_environ; *entry_ptr != NULL; ++entry_ptr) {
            const char *entry = *entry_ptr;
            const char *eq = strchr(entry, '=');
            if (eq != NULL) {
                size_t key_len = (size_t)(eq - entry);
                const char *val = eq + 1;
                size_t val_len = 0;
                while (val[val_len] != '\0') val_len++;
                kiln_value val_obj = kiln_c99_copy_text(val, val_len);
                if (val_obj.tag != KILN_VALUE_STRING) {
                    status = KILN_C99_OUT_OF_MEMORY;
                    break;
                }
                /* This is a fresh host-built own store. Defining its final data descriptor
                 * directly avoids an inherited-property scan for every environment entry,
                 * including the wide sealed layout's otherwise quadratic own-key search. */
                status = kiln_c99_commit_own_data(env, entry, key_len, val_obj, 1, 1, 1);
                if (status != KILN_C99_OK) break;
            }
        }
    }
    if (status == KILN_C99_OK) status = kiln_c99_new_array(&argv);
    if (status == KILN_C99_OK) status = kiln_c99_new_array(&exec_argv);
    if (status == KILN_C99_OK) status = kiln_c99_new_object(&versions);
#if defined(KILN_NODE_ENV_PRODUCTION)
    if (status == KILN_C99_OK)
        status = kiln_c99_commit_own_data(env, "NODE_ENV", 8,
                                          kiln_c99_copy_text("production", 10), 1, 1, 1);
#endif
    if (status == KILN_C99_OK) {
        kiln_object *store = kiln_c99_object_of(env);
        kiln_c99_sealed_layout_publish(KILN_VALUE_OBJECT, KILN_C99_SEALED_ENV_CODE, store);
        (void)kiln_c99_sealed_layout_attach(KILN_VALUE_OBJECT,
                                            KILN_C99_SEALED_ENV_CODE, store);
    }
    if (status == KILN_C99_OK)
        status = kiln_c99_put(NULL, versions, "node", 4,
                              kiln_c99_copy_text("22.0.0", 6), 0, NULL);
    if (status == KILN_C99_OK)
        status = kiln_c99_put(NULL, *process, "env", 3, env, 0, NULL);
    if (status == KILN_C99_OK)
        status = kiln_c99_put(NULL, *process, "argv", 4, argv, 0, NULL);
    if (status == KILN_C99_OK)
        status = kiln_c99_put(NULL, *process, "execArgv", 8, exec_argv, 0, NULL);
    /* A native executable with node:ffi is running against the target OS ABI: platform and arch
     * select the library it can actually load, so reporting the compiler target is now the honest
     * capability answer. The interpreter keeps "kiln" because it exposes no dynamic loader. */
    if (status == KILN_C99_OK) {
#if defined(__linux__)
        kiln_value platform = kiln_c99_copy_text("linux", 5);
#elif defined(__APPLE__)
        kiln_value platform = kiln_c99_copy_text("darwin", 6);
#elif defined(_WIN32)
        kiln_value platform = kiln_c99_copy_text("win32", 5);
#else
        kiln_value platform = kiln_c99_copy_text("kiln", 4);
#endif
        status = kiln_c99_put(NULL, *process, "platform", 8, platform, 0, NULL);
    }
    if (status == KILN_C99_OK) {
#if defined(__x86_64__) || defined(_M_X64)
        kiln_value arch = kiln_c99_copy_text("x64", 3);
#elif defined(__aarch64__) || defined(_M_ARM64)
        kiln_value arch = kiln_c99_copy_text("arm64", 5);
#else
        kiln_value arch = kiln_c99_copy_text("unknown", 7);
#endif
        status = kiln_c99_put(NULL, *process, "arch", 4, arch, 0, NULL);
    }
    /* The node this runtime targets (ADR 0063), spelled the way node spells it: a library reads
     * `process.version.slice(1)` and `process.release.name === "node"` to decide what it may
     * use, and "kiln" here sent every one of them down its oldest path. */
    if (status == KILN_C99_OK)
        status = kiln_c99_put(NULL, *process, "version", 7,
                              kiln_c99_copy_text("v22.0.0", 7), 0, NULL);
    if (status == KILN_C99_OK)
        status = kiln_c99_put(NULL, *process, "versions", 8, versions, 0, NULL);
    if (status == KILN_C99_OK) {
        kiln_value release;
        status = kiln_c99_new_object(&release);
        if (status == KILN_C99_OK)
            status = kiln_c99_put(NULL, release, "name", 4, kiln_c99_copy_text("node", 4), 0, NULL);
        if (status == KILN_C99_OK)
            status = kiln_c99_put(NULL, *process, "release", 7, release, 0, NULL);
    }
    /* The process's own, as the interpreter's is and as a process warning's `(node:<pid>)` says
     * it (`Realm::process_warning`); 1 where there is no POSIX `getpid`. */
    if (status == KILN_C99_OK) {
#if defined(__unix__) || defined(__APPLE__)
        double pid = (double)getpid();
#else
        double pid = 1.0;
#endif
        status = kiln_c99_put(NULL, *process, "pid", 3, kiln_c99_number(pid), 0, NULL);
    }
    /* `process.features`, the same nine flags the interpreter answers (node has a longer list,
     * and these nine agree with it where kiln has the feature). */
    if (status == KILN_C99_OK) {
        static const struct { const char *name; size_t length; int value; } features[] = {
            {"cached_builtins", 15, 1}, {"debug", 5, 0}, {"inspector", 9, 0}, {"ipv6", 4, 1},
            {"tls", 3, 1}, {"tls_alpn", 8, 1}, {"tls_ocsp", 8, 1}, {"tls_sni", 7, 1}, {"uv", 2, 1},
        };
        kiln_value built;
        size_t index;
        status = kiln_c99_new_object(&built);
        for (index = 0; status == KILN_C99_OK && index < sizeof features / sizeof *features; ++index)
            status = kiln_c99_put(NULL, built, features[index].name, features[index].length,
                                  kiln_c99_boolean(features[index].value), 0, NULL);
        if (status == KILN_C99_OK) status = kiln_c99_put(NULL, *process, "features", 8, built, 0, NULL);
    }
    /* `process.getBuiltinModule` is a require over the linked builtins. The native bootstrap
     * installs the TypeScript implementation (built over `__kilnHostModuleCreateRequire`), so
     * the name is left absent here to let that installation take effect. */
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
#if KILN_RETAIN_GROUP_NODE_EVENT
        if (status == KILN_C99_OK) status = kiln_c99_install_event_emitter_methods(stream);
#endif
#if KILN_RETAIN_GROUP_NODE_STREAM
        if (status == KILN_C99_OK) status = kiln_c99_install_readable_methods(stream);
#endif
        if (status == KILN_C99_OK) {
            kiln_value queue;
            status = kiln_c99_new_array(&queue);
            if (status == KILN_C99_OK)
                status = kiln_c99_put(NULL, stream, "@@node:readable", 15, queue, 0, NULL);
        }
        if (status == KILN_C99_OK)
            status = kiln_c99_put(NULL, stream, "fd", 2, kiln_c99_number(0.0), 0, NULL);
        if (status == KILN_C99_OK)
            status = kiln_c99_put(NULL, stream, "isTTY", 5, kiln_c99_boolean(isatty(0) != 0), 0, NULL);
        if (status == KILN_C99_OK)
            status = kiln_c99_put(NULL, stream, "isRaw", 5, kiln_c99_boolean(0), 0, NULL);
        if (status == KILN_C99_OK)
            status = kiln_c99_define_method(stream, "setRawMode", 10,
                                            kiln_c99_method(KILN_C99_METHOD_NODE_READABLE_SET_RAW_MODE));
        if (status == KILN_C99_OK)
            status = kiln_c99_put(NULL, *process, "stdin", 5, stream, 0, NULL);
    }
    if (status == KILN_C99_OK) {
        status = kiln_c99_new_object(&stream);
#if KILN_RETAIN_GROUP_NODE_EVENT
        if (status == KILN_C99_OK) status = kiln_c99_install_event_emitter_methods(stream);
#endif
#if KILN_RETAIN_GROUP_NODE_STREAM
        if (status == KILN_C99_OK) status = kiln_c99_install_writable_methods(stream);
#endif
        if (status == KILN_C99_OK)
            status = kiln_c99_put(NULL, stream, "fd", 2, kiln_c99_number(1.0), 0, NULL);
        if (status == KILN_C99_OK) {
            int is_tty = isatty(1) != 0;
            status = kiln_c99_put(NULL, stream, "isTTY", 5, kiln_c99_boolean(is_tty), 0, NULL);
#if defined(__unix__) || defined(__APPLE__)
            if (status == KILN_C99_OK && is_tty) {
                struct winsize ws;
                if (ioctl(1, TIOCGWINSZ, &ws) == 0) {
                    if (ws.ws_col > 0)
                        (void)kiln_c99_put(NULL, stream, "columns", 7, kiln_c99_number((double)ws.ws_col), 0, NULL);
                    if (ws.ws_row > 0)
                        (void)kiln_c99_put(NULL, stream, "rows", 4, kiln_c99_number((double)ws.ws_row), 0, NULL);
                }
            }
#endif
        }
        if (status == KILN_C99_OK)
            status = kiln_c99_put(NULL, *process, "stdout", 6, stream, 0, NULL);
    }
    if (status == KILN_C99_OK) {
        status = kiln_c99_new_object(&stream);
#if KILN_RETAIN_GROUP_NODE_EVENT
        if (status == KILN_C99_OK) status = kiln_c99_install_event_emitter_methods(stream);
#endif
#if KILN_RETAIN_GROUP_NODE_STREAM
        if (status == KILN_C99_OK) status = kiln_c99_install_writable_methods(stream);
#endif
        if (status == KILN_C99_OK)
            status = kiln_c99_put(NULL, stream, "fd", 2, kiln_c99_number(2.0), 0, NULL);
        if (status == KILN_C99_OK)
            status = kiln_c99_put(NULL, stream, "isTTY", 5, kiln_c99_boolean(isatty(2) != 0), 0, NULL);
        if (status == KILN_C99_OK)
            status = kiln_c99_put(NULL, *process, "stderr", 6, stream, 0, NULL);
    }
    if (status == KILN_C99_OK)
        /* Node's process functions are enumerable own properties (`Object.keys(process)` lists
         * them); the private hooks above and below are not. */
        status = kiln_c99_put(NULL, *process, "nextTick", 8,
                                        kiln_c99_method(KILN_C99_METHOD_PROCESS_NEXT_TICK), 0, NULL);
    if (status == KILN_C99_OK)
        status = kiln_c99_put(NULL, *process, "hrtime", 6,
                                        kiln_c99_method(KILN_C99_METHOD_PROCESS_HRTIME), 0, NULL);
    if (status == KILN_C99_OK)
        status = kiln_c99_put(NULL, *process, "cwd", 3,
                                        kiln_c99_method(KILN_C99_METHOD_PROCESS_CWD), 0, NULL);
    if (status == KILN_C99_OK)
        status = kiln_c99_put(NULL, *process, "kill", 4,
                                        kiln_c99_method(KILN_C99_METHOD_PROCESS_KILL), 0, NULL);
    if (status == KILN_C99_OK)
        status = kiln_c99_put(NULL, *process, "exit", 4,
                                        kiln_c99_method(KILN_C99_METHOD_PROCESS_EXIT), 0, NULL);
#if KILN_RETAIN_GROUP_NODE_NET
    /* The public node:net module is a TypeScript facade. Its per-socket pause/resume/read and
     * cork methods reach these private kernels through process.__kilnNativeModules.net; keeping
     * that adapter separate from the public export object lets the facade own the public API. */
    if (status == KILN_C99_OK) {
        status = kiln_c99_new_object(&net_module);
        if (status == KILN_C99_OK) {
            kiln_c99_gc_keep_alive(&native_modules);
            kiln_c99_gc_keep_alive(&net_module);
        }
    }
    if (status == KILN_C99_OK)
        status = kiln_c99_define_method(net_module, "createServer", 12,
                                        kiln_c99_method(KILN_C99_METHOD_NODE_NET_CREATE_SERVER));
    if (status == KILN_C99_OK)
        status = kiln_c99_define_method(net_module, "connect", 7,
                                        kiln_c99_method(KILN_C99_METHOD_NODE_NET_CONNECT));
    if (status == KILN_C99_OK)
        status = kiln_c99_define_method(net_module, "createConnection", 16,
                                        kiln_c99_method(KILN_C99_METHOD_NODE_NET_CONNECT));
    if (status == KILN_C99_OK)
        status = kiln_c99_define_method(net_module, "__corkSocket", 12,
                                        kiln_c99_method(KILN_C99_METHOD_NODE_SOCKET_CORK_KERNEL));
    if (status == KILN_C99_OK)
        status = kiln_c99_define_method(net_module, "__uncorkSocket", 14,
                                        kiln_c99_method(KILN_C99_METHOD_NODE_SOCKET_UNCORK_KERNEL));
    if (status == KILN_C99_OK)
        status = kiln_c99_define_method(net_module, "__pauseSocket", 13,
                                        kiln_c99_method(KILN_C99_METHOD_NODE_SOCKET_PAUSE_KERNEL));
    if (status == KILN_C99_OK)
        status = kiln_c99_define_method(net_module, "__resumeSocket", 14,
                                        kiln_c99_method(KILN_C99_METHOD_NODE_SOCKET_RESUME_KERNEL));
    if (status == KILN_C99_OK)
        status = kiln_c99_define_method(net_module, "__readSocket", 12,
                                        kiln_c99_method(KILN_C99_METHOD_NODE_SOCKET_READ_KERNEL));
    if (status == KILN_C99_OK)
        status = kiln_c99_put(NULL, native_modules, "net", 3, net_module, 0, NULL);
    kiln_c99_gc_keep_alive(&native_modules);
    kiln_c99_gc_keep_alive(&net_module);
#endif
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
#if KILN_RETAIN_GROUP_TEMPORAL
                        {"Temporal", KILN_C99_BUILTIN_TEMPORAL},
#endif
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
                     {"clearImmediate", KILN_C99_METHOD_CLEAR_IMMEDIATE},
                     /* Captured and deleted by the compiled builtin bootstrap before user code. */
                     {"__kilnIntlSegment", KILN_C99_METHOD_INTL_SEGMENT_RAW},
                     {"__kilnHostStringLocaleCase", KILN_C99_METHOD_STRING_LOCALE_CASE_RAW},
                     {"__kilnIntlDateTimeFormat", KILN_C99_METHOD_INTL_DATETIME_FORMAT_RAW},
                     {"__kilnUint8ToHex", KILN_C99_METHOD_UINT8_TO_HEX_RAW},
                     {"__kilnUint8ToBase64", KILN_C99_METHOD_UINT8_TO_BASE64_RAW},
                     {"__kilnUrlParse", KILN_C99_METHOD_URL_PARSE_RAW},
                     {"__kilnUrlSet", KILN_C99_METHOD_URL_SET_RAW},
                     {"__kilnUrlFast", KILN_C99_METHOD_URL_FAST_RAW},
                     {"__kilnUrlAdopt", KILN_C99_METHOD_URL_ADOPT_RAW},
                     /* The transfer primitive `kiln-builtins/src/arraybuffer.ts` captures. */
                     {"__kilnArrayBufferDetach", KILN_C99_METHOD_ARRAY_BUFFER_DETACH_RAW},
                     {"__kilnArrayBufferByteLength", KILN_C99_METHOD_ARRAY_BUFFER_BYTE_LENGTH_RAW},
                     /* The structured clone's plain-data fast path, captured the same way. */
                     {"__kilnCloneData", KILN_C99_METHOD_CLONE_DATA_RAW},
                     {"__kilnSharedWeak", KILN_C99_METHOD_SHARED_WEAK_RAW},
                     {"__kilnCollectionSize", KILN_C99_METHOD_COLLECTION_SIZE_RAW},
                     {"__kilnJsonSerialize", KILN_C99_METHOD_JSON_STRINGIFY},
                     {"__kilnJsonParse", KILN_C99_METHOD_JSON_PARSE},
                     {"__kilnJsonUnbox", KILN_C99_METHOD_JSON_UNBOX_RAW},
                     {"__kilnRegExpLegacyCapture", KILN_C99_METHOD_REGEXP_LEGACY_CAPTURE_RAW},
                     {"__kilnFunctionKindPrototype", KILN_C99_METHOD_FUNCTION_KIND_PROTOTYPE},
                     {"__kilnFunctionKindConstructor", KILN_C99_METHOD_FUNCTION_KIND_CONSTRUCTOR},
                     /* `%TypedArray%.prototype`'s element-storage primitives, captured the same
                      * way by `kiln-builtins/src/typedarray.ts`. */
                     {"__kilnTypedArrayCopy", KILN_C99_METHOD_TYPED_ARRAY_COPY_RAW},
                     {"__kilnTypedArraySort", KILN_C99_METHOD_TYPED_ARRAY_SORT_RAW},
                     {"__kilnDataViewState", KILN_C99_METHOD_DATA_VIEW_STATE_RAW},
                     {"__kilnIsConstructor", KILN_C99_METHOD_IS_CONSTRUCTOR_RAW},
                     /* Message delivery's checkpoint question, captured the same way. */
                     {"__kilnJobsPending", KILN_C99_METHOD_JOBS_PENDING_RAW},
                     {"__kilnQueueIo", KILN_C99_METHOD_QUEUE_IO_RAW},
                     {"__kilnTimerRef", KILN_C99_METHOD_TIMER_REF_RAW},
                     /* The runtime's cross-realm ports and workers, captured the same way. */
                     {"__kilnWorkerOp", KILN_C99_METHOD_WORKER_OP_RAW},
                     {"__kilnPortWait", KILN_C99_METHOD_PORT_WAIT_RAW},
                     /* `SharedArrayBuffer` stores and `Atomics`, captured the same way. */
                     {"__kilnSharedBuffer", KILN_C99_METHOD_SHARED_BUFFER_RAW},
                     {"__kilnAtomics", KILN_C99_METHOD_ATOMICS_RAW},
                     /* Node's unhandled-rejection tracking registers itself through this. */
                     {"__kilnPromiseRejections", KILN_C99_METHOD_PROMISE_REJECTIONS_RAW},
                     /* `Buffer.prototype.toString`'s decoder, captured by the shared Buffer
                      * bootstrap (kiln-builtins/src/buffer.ts) without a call: the bootstrap
                      * runs in every Node-mode program, and a call into the Buffer group would
                      * refuse in one that never reaches Buffer. */
                     {"__kilnBufferToString", KILN_C99_METHOD_BUFFER_TO_STRING_RAW}};
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
        status = kiln_c99_commit_own_data(*globals, constructors[index].name,
                                          strlen(constructors[index].name),
                                          kiln_c99_builtin(constructors[index].builtin),
                                          0, 1, 1);
    }
    for (index = 0; index < sizeof functions / sizeof *functions && status == KILN_C99_OK;
         ++index) {
        status = kiln_c99_commit_own_data(*globals, functions[index].name,
                                          strlen(functions[index].name),
                                          kiln_c99_method(functions[index].code), 0, 1, 1);
    }
    /* The two the realm builds rather than names: each is a real object with its own slot, so
     * `globalThis.console === console` and a second read finds the same one. */
    if (status == KILN_C99_OK) {
        kiln_value built;
        status = kiln_c99_atomics_object(&built);
        if (status == KILN_C99_OK) {
            status = kiln_c99_commit_own_data(*globals, "Atomics", 7, built, 0, 1, 1);
        }
    }
    if (status == KILN_C99_OK) {
        kiln_value built;
        status = kiln_c99_console_object(&built);
        if (status == KILN_C99_OK) {
            status = kiln_c99_commit_own_data(*globals, "console", 7, built, 0, 1, 1);
        }
    }
    /* Whether this realm installs kiln's own members (ADR 0068), for the shared bootstrap's
     * `%TypedArray%.prototype`, which installs its extension itself. Captured and deleted by
     * `kiln-builtins/src/extensions.ts` before any program runs, as the interpreter does. */
    if (status == KILN_C99_OK) {
        status = kiln_c99_commit_own_data(*globals, "__kilnExtensions", 16,
                                          kiln_c99_boolean(kiln_c99_kiln_extensions_on()),
                                          0, 1, 1);
    }
    /* The realm's engine-private object, captured and deleted by `kiln-builtins/src/extensions.ts`
     * before any program runs. Later it is readable only through the read overlay below. */
    if (status == KILN_C99_OK) {
        kiln_value private_object;
        status = kiln_c99_private_object(&private_object);
        if (status == KILN_C99_OK) {
            status = kiln_c99_commit_own_data(*globals, "__kilnPrivate", 13, private_object,
                                              0, 1, 1);
        }
    }
    /* `process` too: the bare name loaded it by demand while `globalThis.process` answered
     * `undefined` (2026-09-19). One object, the realm slot's, so the two spellings agree. */
    if (status == KILN_C99_OK) {
        kiln_value built;
        status = kiln_c99_process_object(&built);
        if (status == KILN_C99_OK) {
            status = kiln_c99_commit_own_data(*globals, "process", 7, built, 0, 1, 1);
        }
    }
    /* `crypto` is a namespace built-in with no rows, so a member read refuses by name rather than
     * reading `undefined` off an empty object, which is what stood here. `node:crypto`, when it
     * is linked, replaces this binding with its own object (see the tail of `node/crypto.ts`);
     * it replaces rather than patches because a write into a built-in's store bumps
     * `kiln_c99_builtin_store_epoch` and would switch off built-in speculation for the program. */
    if (status == KILN_C99_OK) {
        status = kiln_c99_commit_own_data(*globals, "crypto", 6,
                                          kiln_c99_builtin(KILN_C99_BUILTIN_CRYPTO), 0, 1, 1);
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
            status = kiln_c99_commit_own_data(
                *globals, locked[index].name, locked[index].length,
                locked[index].is_undefined ? kiln_c99_undefined()
                                           : kiln_c99_number(values[index]),
                0, 0, 0);
        }
    }
    /* It holds itself under both of its names, which is how a program reaches it without a
     * keyword: `global.Object === Object` is the standard test for "am I in a realm that has
     * one". */
    if (status == KILN_C99_OK) {
        status = kiln_c99_commit_own_data(*globals, "globalThis", 10, *globals, 0, 1, 1);
    }
    if (status == KILN_C99_OK) {
        status = kiln_c99_commit_own_data(*globals, "global", 6, *globals, 0, 1, 1);
    }
    if (status != KILN_C99_OK) {
        return status;
    }
    *out = *globals;
    return KILN_C99_OK;
}

/* The realm's engine-private object (`__kilnPrivate`): the one channel between the engine, the
 * realm bootstrap and the builtin modules. It holds the engine's own operations -- present only
 * when kiln's extensions are installed (ADR 0068) -- and whatever one builtin hands another
 * (the message-port internals, the structured clone's marks). It is a realm slot, so it is a GC
 * root of its realm like the other slots, and per realm like them (ADR 0071). A program can
 * never read it: the bootstrap captures it from the global object at realm creation and deletes
 * the name, and a builtin module reads it only through the overlay below. */
#define KILN_C99_REALM_SLOT_PRIVATE UINT32_C(13)

static int kiln_c99_private_object(kiln_value *out) {
    kiln_value *slot;
    int status = kiln_c99_realm_value_slot(KILN_C99_REALM_SLOT_PRIVATE, &slot);
    if (status != KILN_C99_OK) return status;
    if (slot->tag == KILN_VALUE_OBJECT) {
        *out = *slot;
        return KILN_C99_OK;
    }
    status = kiln_c99_new_object(slot);
    if (status != KILN_C99_OK) return status;
    status = kiln_c99_commit_own_data(*slot, "extensions", 10,
                                      kiln_c99_boolean(kiln_c99_kiln_extensions_on()), 0, 1, 1);
    /* A developer switch for the conformance probes that drive the actor protocol directly: the
     * bootstrap then leaves the global name in place. Off unless the process is started with it. */
    if (status == KILN_C99_OK) {
        const char *exposed = getenv("KILN_ENGINE_PRIVATE_FOR_TESTS");
        status = kiln_c99_commit_own_data(*slot, "exposed", 7,
                                          kiln_c99_boolean(exposed != NULL && exposed[0] == '1' &&
                                                           exposed[1] == '\0'), 0, 1, 1);
    }
    if (status == KILN_C99_OK && kiln_c99_kiln_extensions_on()) {
        status = kiln_c99_commit_own_data(*slot, "share", 5,
                                          kiln_c99_method(KILN_C99_METHOD_SHARE_RAW), 0, 1, 1);
        if (status == KILN_C99_OK)
            status = kiln_c99_commit_own_data(*slot, "sharedIs", 8,
                                              kiln_c99_method(KILN_C99_METHOD_SHARED_IS_RAW),
                                              0, 1, 1);
        if (status == KILN_C99_OK)
            status = kiln_c99_commit_own_data(*slot, "hostProcessStart", 16,
                                              kiln_c99_method(KILN_C99_METHOD_PROCESS_START_RAW),
                                              0, 1, 1);
        if (status == KILN_C99_OK)
            status = kiln_c99_commit_own_data(*slot, "hostProcessCancel", 17,
                                              kiln_c99_method(KILN_C99_METHOD_PROCESS_CANCEL_RAW),
                                              0, 1, 1);
    }
    if (status == KILN_C99_OK)
        status = kiln_c99_commit_own_data(*slot, "processExitCodeGet", 18,
                                          kiln_c99_method(KILN_C99_METHOD_PROCESS_EXIT_CODE_GET),
                                          0, 1, 1);
    if (status == KILN_C99_OK)
        status = kiln_c99_commit_own_data(*slot, "processExitCodeSet", 18,
                                          kiln_c99_method(KILN_C99_METHOD_PROCESS_EXIT_CODE_SET),
                                          0, 1, 1);
    if (status == KILN_C99_OK)
        status = kiln_c99_commit_own_data(*slot, "gcRequest", 9,
                                          kiln_c99_method(KILN_C99_METHOD_GC_REQUEST), 0, 1, 1);
    if (status != KILN_C99_OK) return status;
    *out = *slot;
    return KILN_C99_OK;
}

/* How many engine builtin modules are initializing on this thread right now: the generated graph
 * wrapper brackets each module's synchronous top-level code with `kiln_realm_private_window`. A
 * realm never changes lanes in the middle of a turn, and a module body has no await at the
 * level the overlay answers, so no continuation can inherit the count. */
static _Thread_local int kiln_c99_private_hooks_active;

/* The read overlay never writes the global. A program's own property of this name therefore
 * keeps its descriptor and slot order across a later dynamic import, and nothing a builtin
 * defines on the global object while the window is open is discarded when it closes. */
static int kiln_c99_private_hook_read(kiln_value target, const kiln_string *key,
                                      kiln_value *out) {
    kiln_value *global;
    if (kiln_c99_private_hooks_active <= 0 || target.tag != KILN_VALUE_OBJECT ||
        key == NULL || kiln_c99_key_is_reserved(key) || out == NULL) return 0;
    if (!kiln_c99_same_name_key(key, "__kilnPrivate")) return 0;
    if (kiln_c99_realm_value_slot(KILN_C99_REALM_SLOT_GLOBALS, &global) != KILN_C99_OK ||
        global == NULL || global->tag != KILN_VALUE_OBJECT || global->bits != target.bits) return 0;
    return kiln_c99_private_object(out) == KILN_C99_OK;
}

/* The generated graph wrapper calls this immediately around each engine builtin module's
 * initializer, after the realm bootstrap. It changes only the read overlay above. */
int kiln_realm_private_window(int begin) {
    if (begin) {
        kiln_c99_private_hooks_active += 1;
    } else if (kiln_c99_private_hooks_active > 0) {
        kiln_c99_private_hooks_active -= 1;
    }
    return KILN_C99_OK;
}

/* Whether this realm installs kiln's own members (ADR 0068). A kiln module refuses itself without
 * them, in TypeScript, from the `extensions` entry of the engine-private object. */
int kiln_realm_kiln_extensions_on(void) {
    return kiln_c99_kiln_extensions_on();
}

static int kiln_c99_global_object_val(kiln_val *out) {
    kiln_value g;
    int status = kiln_c99_global_object(&g);
    if (status != KILN_C99_OK) return status;
    *out = kiln_val_from_boundary(g);
    return KILN_C99_OK;
}

/* One binding of the realm's global object, for a name the compiler could not place.
 *
 * `found` stays 0 for a name that is genuinely absent, which is the caller's cue to throw. Own
 * global bindings take priority, followed by inherited Object.prototype methods from the global
 * object's object record. */
/* A global the runtime bootstrap defines in TypeScript -- `structuredClone` -- read the way a
 * program's own global read goes. The bootstrap runs before any module body, so the only way to
 * miss is a program compiled without it, which is refused by name rather than read as undefined. */
static int kiln_c99_object_prototype_binding(kiln_host *host, kiln_value *out,
                                             const kiln_string *view, int *found, int depth,
                                             kiln_value *thrown) {
    kiln_value object_prototype = kiln_c99_builtin(KILN_C99_BUILTIN_OBJECT_PROTOTYPE);
    kiln_value store = kiln_c99_undefined();
    kiln_property property;
    char scratch[256];
    const char *name;
    size_t name_len;
    int status;
    *found = 0;
    if (!kiln_c99_intrinsic_ascii_view(view, scratch, &name, &name_len)) return KILN_C99_OK;
    status = kiln_c99_callable_store(object_prototype, &store, 0);
    if (status != KILN_C99_OK) return status;
    if (store.tag == KILN_VALUE_OBJECT &&
        kiln_c99_property_read_own(kiln_c99_object_of(store), view, NULL, &property)) {
        /* The property may be an accessor or a program's replacement for an intrinsic.
         * Resolve it through [[Get]] so the live descriptor and correct receiver decide. */
        status = kiln_c99_get_key(host, object_prototype, view, NULL, out, depth, thrown);
        if (status != KILN_C99_OK) return status;
        *found = 1;
        return KILN_C99_OK;
    }
    {
        uint64_t method = kiln_c99_namespace_method(KILN_C99_BUILTIN_OBJECT_PROTOTYPE,
                                                    name, name_len);
        if (method != 0) {
            *out = kiln_c99_method(method);
            *found = 1;
        }
    }
    return KILN_C99_OK;
}

int kiln_c99_bootstrap_global(kiln_host *host, kiln_value *out, const char *name,
                              size_t name_len, int depth, kiln_value *thrown) {
    int found = 0;
    int status = kiln_c99_global_binding(host, out, name, name_len, &found, depth, thrown);
    if (status != KILN_C99_OK) return status;
    if (!found) {
        KILN_C99_REFUSE("a bootstrap-defined global read before the runtime bootstrap defined it");
    }
    return KILN_C99_OK;
}

int kiln_c99_global_binding(kiln_host *host, kiln_value *out, const char *name,
                            size_t name_len, int *found, int depth, kiln_value *thrown) {
    kiln_value globals;
    kiln_property property;
    kiln_string view;
    int status = kiln_c99_global_object(&globals);
    *found = 0;
    if (status != KILN_C99_OK) {
        return status;
    }
    if (name_len > UINT32_MAX) return KILN_C99_OUT_OF_MEMORY;
    view.unit_len = (uint32_t)name_len;
    view.flags = kiln_c99_fixed_key_flags(name, name_len);
    view.data = name;
    if (kiln_c99_property_read_own(kiln_c99_object_of(globals), &view, NULL, &property)) {
        if (property.accessor) {
            status = kiln_c99_get_key(host, globals, &view, NULL, out, depth, thrown);
            if (status != KILN_C99_OK) return status;
        } else {
            *out = property.value;
        }
        *found = 1;
        return KILN_C99_OK;
    }
    return kiln_c99_object_prototype_binding(host, out, &view, found, depth, thrown);
}

/* The headed form: global names compare as units, so a non-ASCII binding resolves the
 * way the interpreter resolves it rather than missing every name the ASCII-only
 * comparison cannot spell. */
int kiln_c99_global_binding_key(kiln_host *host, kiln_value *out, const kiln_string *name,
                                int *found, int depth, kiln_value *thrown) {
    kiln_value globals;
    kiln_property property;
    int status = kiln_c99_global_object(&globals);
    *found = 0;
    if (status != KILN_C99_OK) {
        return status;
    }
    if (kiln_c99_property_read_own(kiln_c99_object_of(globals), name, NULL, &property)) {
        if (property.accessor) {
            status = kiln_c99_get_key(host, globals, name, NULL, out, depth, thrown);
            if (status != KILN_C99_OK) return status;
        } else {
            *out = property.value;
        }
        *found = 1;
        return KILN_C99_OK;
    }
    return kiln_c99_object_prototype_binding(host, out, name, found, depth, thrown);
}

static int kiln_c99_global_own_binding(kiln_value *out, const char *name, size_t name_len,
                                      int *found) {
    kiln_value globals;
    kiln_property property;
    kiln_string view;
    int status = kiln_c99_global_object(&globals);
    *found = 0;
    if (status != KILN_C99_OK) return status;
    if (name_len > UINT32_MAX) return KILN_C99_OUT_OF_MEMORY;
    view.unit_len = (uint32_t)name_len;
    view.flags = kiln_c99_fixed_key_flags(name, name_len);
    view.data = name;
    if (kiln_c99_property_read_own(kiln_c99_object_of(globals), &view, NULL, &property)) {
        *out = property.value;
        *found = 1;
    }
    return KILN_C99_OK;
}

static int kiln_c99_global_own_binding_key(kiln_value *out, const kiln_string *name,
                                          int *found) {
    kiln_value globals;
    kiln_property property;
    int status = kiln_c99_global_object(&globals);
    *found = 0;
    if (status != KILN_C99_OK) return status;
    if (kiln_c99_property_read_own(kiln_c99_object_of(globals), name, NULL, &property)) {
        *out = property.value;
        *found = 1;
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
    int status = kiln_c99_global_own_binding(&existing, name, name_len, &found);
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

/* The headed form, owning the same create-if-absent contract through the keyed lookup
 * and slot primitives. */
static int kiln_c99_declare_global_key(const kiln_string *name, int configurable) {
    kiln_value globals;
    kiln_value existing;
    kiln_property *slot;
    int found = 0;
    int status = kiln_c99_global_own_binding_key(&existing, name, &found);
    if (status != KILN_C99_OK || found) {
        return status;
    }
    status = kiln_c99_global_object(&globals);
    if (status != KILN_C99_OK) {
        return status;
    }
    status = kiln_c99_own_slot_key(globals, name, NULL, &slot);
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
