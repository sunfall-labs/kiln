use super::*;
#[derive(Debug, Default)]
pub(super) struct NativeGraphPlan {
    pub(super) excluded: BTreeSet<PathBuf>,
    pub(super) failures: BTreeMap<(PathBuf, String), String>,
    pub(super) runtime_failures: BTreeMap<(PathBuf, PathBuf, String), String>,
}

pub(super) fn native_graph_plan(graph: &ModuleGraph) -> Result<NativeGraphPlan, NativeError> {
    native_graph_plan_with_validator(graph, &validate_native_builtin_node)
}

pub(super) fn native_graph_plan_with_validator(
    graph: &ModuleGraph,
    validate: &dyn Fn(&ModuleGraph, &kiln_module_graph::ModuleNode) -> Result<(), NativeError>,
) -> Result<NativeGraphPlan, NativeError> {
    #[derive(Clone)]
    struct LazyEdge {
        owner: PathBuf,
        specifier: String,
        captured_importer: PathBuf,
        target: PathBuf,
        runtime: bool,
    }

    let static_reachable = graph.statically_reachable();
    validate_native_builtins_in_paths(graph, &static_reachable, validate)?;
    let mut plan = NativeGraphPlan::default();
    let mut retained = static_reachable.clone();
    let mut pending = static_reachable
        .iter()
        .flat_map(|path| {
            let dynamic = graph
                .dynamic_dependencies(path)
                .iter()
                .map(move |dependency| LazyEdge {
                    owner: path.clone(),
                    specifier: dependency.specifier.clone(),
                    captured_importer: path.clone(),
                    target: dependency.target.clone(),
                    runtime: false,
                });
            let runtime = graph.module(path).into_iter().flat_map(move |node| {
                node.runtime_dependencies
                    .iter()
                    .map(move |dependency| LazyEdge {
                        owner: path.clone(),
                        specifier: dependency.specifier.clone(),
                        captured_importer: dependency.importer.clone(),
                        target: dependency.target.clone(),
                        runtime: true,
                    })
            });
            dynamic.chain(runtime)
        })
        .collect::<Vec<_>>();
    let mut processed = BTreeSet::new();
    while let Some(edge) = pending.pop() {
        if !processed.insert((
            edge.owner.clone(),
            edge.captured_importer.clone(),
            edge.specifier.clone(),
            edge.runtime,
        )) {
            continue;
        }
        let mut closure = BTreeSet::new();
        let mut walk = vec![edge.target.clone()];
        let mut unsupported = None;
        while let Some(path) = walk.pop() {
            if !closure.insert(path.clone()) {
                continue;
            }
            let Some(node) = graph.module(&path) else {
                continue;
            };
            if let Err(error) = validate(graph, node) {
                unsupported.get_or_insert(error.to_string());
            }
            walk.extend(node.dependencies.iter().map(|edge| edge.target.clone()));
        }
        if let Some(message) = unsupported {
            if edge.runtime {
                plan.runtime_failures.insert(
                    (edge.owner, edge.captured_importer, edge.specifier),
                    format!("runtime request failed: {message}"),
                );
            } else {
                plan.failures.insert(
                    (edge.owner, edge.specifier),
                    format!("dynamic import failed: {message}"),
                );
            }
            continue;
        }
        for path in &closure {
            retained.insert(path.clone());
            for nested in graph.dynamic_dependencies(path) {
                pending.push(LazyEdge {
                    owner: path.clone(),
                    specifier: nested.specifier.clone(),
                    captured_importer: path.clone(),
                    target: nested.target.clone(),
                    runtime: false,
                });
            }
            if let Some(node) = graph.module(path) {
                for nested in &node.runtime_dependencies {
                    pending.push(LazyEdge {
                        owner: path.clone(),
                        specifier: nested.specifier.clone(),
                        captured_importer: nested.importer.clone(),
                        target: nested.target.clone(),
                        runtime: true,
                    });
                }
            }
        }
    }
    plan.excluded = graph
        .modules()
        .map(|node| node.path.clone())
        .filter(|path| !retained.contains(path))
        .collect();
    Ok(plan)
}

pub(super) fn validate_native_builtins_in_paths(
    graph: &ModuleGraph,
    paths: &BTreeSet<PathBuf>,
    validate: &dyn Fn(&ModuleGraph, &kiln_module_graph::ModuleNode) -> Result<(), NativeError>,
) -> Result<(), NativeError> {
    for path in paths {
        if let Some(node) = graph.module(path) {
            validate(graph, node)?;
        }
    }
    Ok(())
}

pub(super) fn validate_native_builtin_node(
    graph: &ModuleGraph,
    node: &kiln_module_graph::ModuleNode,
) -> Result<(), NativeError> {
    for import in &node.program.imports {
        let Some(dependency) = node
            .dependencies
            .iter()
            .find(|dependency| dependency.specifier == import.source)
        else {
            continue;
        };
        let Some(provider) = graph.module(&dependency.target) else {
            continue;
        };
        let ModuleSource::NodeBuiltin(builtin) = provider.source else {
            continue;
        };
        if import.imported != kiln_ir::NAMESPACE_IMPORT
            && !native_builtin_supports_export(builtin, &import.imported)
        {
            return Err(NativeError::UnsupportedNodeBuiltinExport {
                specifier: builtin.specifier(),
                export_name: import.imported.clone(),
            });
        }
    }
    Ok(())
}

pub(super) fn native_builtin_supports_export(
    builtin: kiln_module_resolver::NodeBuiltin,
    export_name: &str,
) -> bool {
    kiln_builtins::builtin_supported_exports(builtin).contains(&export_name)
}

pub(super) const NATIVE_BUILTIN_BOOTSTRAP_SOURCE: &str = r#"
const __kilnSegmentBoundaries = globalThis.__kilnIntlSegment;
delete globalThis.__kilnIntlSegment;

function Segmenter(locales, options) {
    const granularity = options === undefined || options.granularity === undefined
        ? "grapheme"
        : String(options.granularity);
    if (granularity !== "grapheme" && granularity !== "word" && granularity !== "sentence") {
        throw new RangeError("invalid granularity: " + granularity);
    }
    this.__kilnGranularity = granularity;
    this.__kilnLocale = typeof locales === "string" ? locales : "en-US";
}

Segmenter.prototype.segment = function (input) {
    const text = String(input);
    const kind = this.__kilnGranularity === "word"
        ? 1
        : (this.__kilnGranularity === "sentence" ? 2 : 0);
    const tape = __kilnSegmentBoundaries(text, kind);
    const records = [];
    for (let index = 0; index < tape.length; index += 4) {
        const record = {
            segment: text.slice(tape[index], tape[index + 1]),
            index: tape[index],
            input: text
        };
        if (tape[index + 2] !== 0) record.isWordLike = tape[index + 3] !== 0;
        records.push(record);
    }
    const segments = {
        containing(index) {
            const position = Number(index);
            if (!Number.isFinite(position) || position < 0 || position >= text.length) {
                return undefined;
            }
            for (let at = 0; at < records.length; at += 1) {
                const start = records[at].index;
                const end = start + records[at].segment.length;
                if (start <= position && position < end) return records[at];
            }
            return undefined;
        }
    };
    segments[Symbol.iterator] = function () { return records[Symbol.iterator](); };
    return segments;
};

Segmenter.prototype.resolvedOptions = function () {
    return { locale: this.__kilnLocale, granularity: this.__kilnGranularity };
};

Segmenter.supportedLocalesOf = function (locales) {
    if (locales === undefined) return [];
    return Array.isArray(locales) ? locales.slice() : [String(locales)];
};

Intl.Segmenter = Segmenter;


const __kilnUint8ToHex = globalThis.__kilnUint8ToHex;
delete globalThis.__kilnUint8ToHex;
const __kilnUint8ToBase64 = globalThis.__kilnUint8ToBase64;
delete globalThis.__kilnUint8ToBase64;

if (typeof Uint8Array !== "undefined" && Uint8Array.prototype) {
    Uint8Array.prototype.toHex = function toHex() {
        if (!(this instanceof Uint8Array)) {
            throw new TypeError("toHex called on incompatible receiver");
        }
        return __kilnUint8ToHex(this);
    };

    Uint8Array.prototype.toBase64 = function toBase64(options) {
        if (!(this instanceof Uint8Array)) {
            throw new TypeError("toBase64 called on incompatible receiver");
        }
        let url = false;
        let omitPadding = false;
        if (options !== undefined) {
            if (typeof options !== "object" || options === null) {
                throw new TypeError("options must be an object");
            }
            if (options.alphabet !== undefined) {
                const alphabet = String(options.alphabet);
                if (alphabet === "base64url") {
                    url = true;
                } else if (alphabet === "base64") {
                    url = false;
                } else {
                    throw new TypeError('alphabet must be "base64" or "base64url"');
                }
            }
            omitPadding = Boolean(options.omitPadding);
        }
        return __kilnUint8ToBase64(this, url, omitPadding);
    };
}

if (typeof JSON !== "undefined") {
    const __kilnOriginalParse = JSON.parse;
    const __kilnOriginalStringify = JSON.stringify;

    JSON.parse = function parse(text, reviver) {
        const root = __kilnOriginalParse(text);
        if (typeof reviver !== "function") return root;

        function internalize(holder, name) {
            const val = holder[name];
            if (val && typeof val === "object") {
                if (Array.isArray(val)) {
                    for (let i = 0; i < val.length; i++) {
                        const newElement = internalize(val, String(i));
                        if (newElement === undefined) {
                            delete val[i];
                        } else {
                            val[i] = newElement;
                        }
                    }
                } else {
                    for (const key of Object.keys(val)) {
                        const newProperty = internalize(val, key);
                        if (newProperty === undefined) {
                            delete val[key];
                        } else {
                            val[key] = newProperty;
                        }
                    }
                }
            }
            return reviver.call(holder, name, val);
        }

        return internalize({ "": root }, "");
    };

    JSON.stringify = function stringify(value, replacer, space) {
        if (typeof replacer !== "function" && !Array.isArray(replacer)) {
            return __kilnOriginalStringify(value, replacer, space);
        }
        let replacerFn = null;
        let propertyList = null;
        if (typeof replacer === "function") {
            replacerFn = replacer;
        } else if (Array.isArray(replacer)) {
            propertyList = [];
            for (let i = 0; i < replacer.length; i++) {
                const item = replacer[i];
                if (typeof item === "string" || typeof item === "number") {
                    const str = String(item);
                    if (propertyList.indexOf(str) === -1) {
                        propertyList.push(str);
                    }
                }
            }
        }

        const stack = [];
        function serialize(holder, key) {
            let val = holder[key];
            if (val && typeof val === "object" && typeof val.toJSON === "function") {
                val = val.toJSON(key);
            }
            if (replacerFn !== null) {
                val = replacerFn.call(holder, key, val);
            }
            if (val === undefined || typeof val === "function" || typeof val === "symbol") {
                return undefined;
            }
            if (val === null || typeof val !== "object") {
                return val;
            }
            if (stack.indexOf(val) !== -1) {
                throw new TypeError("Converting circular structure to JSON");
            }
            stack.push(val);
            let res;
            if (Array.isArray(val)) {
                res = [];
                for (let i = 0; i < val.length; i++) {
                    const item = serialize(val, String(i));
                    res.push(item === undefined ? null : item);
                }
            } else {
                res = {};
                const keys = propertyList !== null ? propertyList : Object.keys(val);
                for (let i = 0; i < keys.length; i++) {
                    const k = keys[i];
                    if (Object.prototype.hasOwnProperty.call(val, k)) {
                        const propVal = serialize(val, k);
                        if (propVal !== undefined) {
                            res[k] = propVal;
                        }
                    }
                }
            }
            stack.pop();
            return res;
        }

        const transformed = serialize({ "": value }, "");
        if (transformed === undefined) return undefined;
        return __kilnOriginalStringify(transformed, undefined, space);
    };
}

// The rest of `console`. The prelude C builds the global with `log`, `info`, `error` and `warn`
// and nothing else, while the interpreter's carries the twenty-four the language does plus a
// `Console` prototype. Every other method read `undefined` natively -- and `console.debug("x")`
// then died three frames later as "undefined is not a function" -- until 2026-09-19. What each
// does is the interpreter's rule, copied: `debug`, `dir`, `table` print as `log` does; `assert`
// and `trace` print to stderr under a fixed prefix; the timing, grouping, counting and profiling
// methods accept their arguments and do nothing, because the realm's clock is virtual
// (ADR 0044) and there is no debugger to speak to. Written here rather than in C per the
// Phase 5.1 freeze.
{
    const c = globalThis.console;
    // An instance made with `new Console(stdout[, stderr])` writes to those streams, as the
    // interpreter's does; the global `console` carries no streams and reaches the prelude's
    // `log` and `error`, which is where every method here ends up for it.
    const out = (self, stderr, args) => {
        const stream = stderr ? self._stderr : self._stdout;
        if (stream && typeof stream.write === "function") {
            stream.write(args.map(String).join(" ") + "\n");
        } else if (stderr) {
            c.error(...args);
        } else {
            c.log(...args);
        }
    };
    class Console {
        constructor(stdout, stderr) {
            let o = stdout;
            let e = stderr;
            if (o && typeof o === "object" && o.stdout) {
                e = o.stderr || o.stdout;
                o = o.stdout;
            }
            if (o === undefined) {
                throw new TypeError("Console expects a writable stream instance for stdout");
            }
            this._stdout = o;
            this._stderr = e || o;
        }
        log(...args) { out(this, false, args); }
        info(...args) { out(this, false, args); }
        debug(...args) { out(this, false, args); }
        dir(...args) { out(this, false, args); }
        table(...args) { out(this, false, args); }
        error(...args) { out(this, true, args); }
        warn(...args) { out(this, true, args); }
        // The interpreter formats the message first (`util.format`, so `%s` in its first
        // argument substitutes) and prefixes the result. Folding the prefix into a string first
        // argument is the same thing; a non-string first argument is inspected either way.
        assert(value, ...message) {
            if (value) return;
            if (message.length === 0) out(this, true, ["Assertion failed"]);
            else if (typeof message[0] === "string") {
                out(this, true, ["Assertion failed: " + message[0], ...message.slice(1)]);
            } else out(this, true, ["Assertion failed:", ...message]);
        }
        trace(...args) {
            if (args.length === 0) out(this, true, ["Trace"]);
            else if (typeof args[0] === "string") out(this, true, ["Trace: " + args[0], ...args.slice(1)]);
            else out(this, true, ["Trace:", ...args]);
        }
        clear() {}
        count() {}
        countReset() {}
        group() {}
        groupEnd() {}
        groupCollapsed() {}
        time() {}
        timeEnd() {}
        timeLog() {}
        dirxml() {}
        timeStamp() {}
        profile() {}
        profileEnd() {}
    }
    const proto = Console.prototype;
    Object.defineProperty(proto, Symbol.toStringTag, { value: "console", configurable: true });
    proto.Console = Console;
    const added = ["debug", "dir", "table", "assert", "trace", "clear", "count", "countReset",
                   "group", "groupEnd", "groupCollapsed", "time", "timeEnd", "timeLog", "dirxml",
                   "timeStamp", "profile", "profileEnd"];
    for (let i = 0; i < added.length; i++) {
        Object.defineProperty(c, added[i], { value: proto[added[i]], writable: true,
                                             configurable: true, enumerable: false });
    }
    Object.defineProperty(c, "Console", { value: Console, writable: true, configurable: true,
                                          enumerable: false });
    Object.setPrototypeOf(c, proto);
}

// Members the prelude C answers `undefined` for off a built-in's prototype, defined into the
// built-in's own store, which every read consults first. `Error.prototype.name` is the one that
// matters most: `class X extends Error {}` gives an instance whose chain ends here, so
// `new X().name` was `undefined` where node says "Error". The `@@`-keyed methods existed in the
// interpreter and read `undefined` natively for as long as the symbol catch-all stood; with that
// gone they would refuse, and these are the answers. Each is written the way the specification
// reduces it, to the ordinary method on the same receiver.
{
    const data = (target, key, value, writable) =>
        Object.defineProperty(target, key, { value, writable: writable === true,
                                             configurable: true, enumerable: false });
    Object.defineProperty(String.prototype, "length", { value: 0, writable: false,
                                                        configurable: false, enumerable: false });
    data(ArrayBuffer.prototype, Symbol.toStringTag, "ArrayBuffer");
    data(DataView.prototype, Symbol.toStringTag, "DataView");
    const methods = {
        [Symbol.match](string) { return String(string).match(this); },
        [Symbol.matchAll](string) { return String(string).matchAll(this); },
        [Symbol.replace](string, replacement) { return String(string).replace(this, replacement); },
        [Symbol.search](string) { return String(string).search(this); },
        [Symbol.split](string, limit) { return String(string).split(this, limit); },
    };
    data(RegExp.prototype, Symbol.match, methods[Symbol.match], true);
    data(RegExp.prototype, Symbol.matchAll, methods[Symbol.matchAll], true);
    data(RegExp.prototype, Symbol.replace, methods[Symbol.replace], true);
    data(RegExp.prototype, Symbol.search, methods[Symbol.search], true);
    data(RegExp.prototype, Symbol.split, methods[Symbol.split], true);
    const symbolToPrimitive = {
        [Symbol.toPrimitive]() { return this.valueOf(); },
    };
    data(Symbol.prototype, Symbol.toPrimitive, symbolToPrimitive[Symbol.toPrimitive]);
    const hasInstance = {
        [Symbol.hasInstance](value) {
            if (typeof this !== "function") return false;
            if (value === null || (typeof value !== "object" && typeof value !== "function")) {
                return false;
            }
            const target = this.prototype;
            let current = Object.getPrototypeOf(value);
            while (current !== null) {
                if (current === target) return true;
                current = Object.getPrototypeOf(current);
            }
            return false;
        },
    };
    data(Function.prototype, Symbol.hasInstance, hasInstance[Symbol.hasInstance]);
}

// The ES2025 Set methods, over the Set intrinsics. `other` is a set-like: anything with a
// numeric `size`, a callable `has` and a callable `keys` (GetSetRecord), which is what lets a
// Map stand in for a Set of its keys. Each method reads `this` through `has`/`keys` and never
// through the other side's `has` when the specification says to iterate instead, because the
// argument's `has` can lie and the result must be what the specification says, not what the
// argument claims.
{
    const setRecord = (other) => {
        if (other === null || (typeof other !== "object" && typeof other !== "function")) {
            throw new TypeError("The \"other\" argument must be set-like");
        }
        const rawSize = other.size;
        const size = Number(rawSize);
        if (rawSize === undefined || Number.isNaN(size)) {
            throw new TypeError("The \"other\" argument must have a numeric \"size\" property");
        }
        if (typeof other.has !== "function") {
            throw new TypeError("The \"other\" argument must have a callable \"has\" method");
        }
        if (typeof other.keys !== "function") {
            throw new TypeError("The \"other\" argument must have a callable \"keys\" method");
        }
        return { set: other, size: Math.trunc(size), has: other.has, keys: other.keys };
    };
    const requireSet = (value, name) => {
        if (!(value instanceof Set)) {
            throw new TypeError(`Method Set.prototype.${name} called on incompatible receiver`);
        }
    };
    const methods = {
        union(other) {
            requireSet(this, "union");
            const record = setRecord(other);
            const result = new Set(this);
            for (const value of record.keys.call(record.set)) result.add(value);
            return result;
        },
        intersection(other) {
            requireSet(this, "intersection");
            const record = setRecord(other);
            const result = new Set();
            if (this.size <= record.size) {
                for (const value of this) if (record.has.call(record.set, value)) result.add(value);
            } else {
                for (const value of record.keys.call(record.set)) if (this.has(value)) result.add(value);
            }
            return result;
        },
        difference(other) {
            requireSet(this, "difference");
            const record = setRecord(other);
            const result = new Set(this);
            if (this.size <= record.size) {
                for (const value of this) if (record.has.call(record.set, value)) result.delete(value);
            } else {
                for (const value of record.keys.call(record.set)) result.delete(value);
            }
            return result;
        },
        symmetricDifference(other) {
            requireSet(this, "symmetricDifference");
            const record = setRecord(other);
            const result = new Set(this);
            for (const value of record.keys.call(record.set)) {
                if (this.has(value)) result.delete(value);
                else result.add(value);
            }
            return result;
        },
        isSubsetOf(other) {
            requireSet(this, "isSubsetOf");
            const record = setRecord(other);
            if (this.size > record.size) return false;
            for (const value of this) if (!record.has.call(record.set, value)) return false;
            return true;
        },
        isSupersetOf(other) {
            requireSet(this, "isSupersetOf");
            const record = setRecord(other);
            if (this.size < record.size) return false;
            for (const value of record.keys.call(record.set)) if (!this.has(value)) return false;
            return true;
        },
        isDisjointFrom(other) {
            requireSet(this, "isDisjointFrom");
            const record = setRecord(other);
            if (this.size <= record.size) {
                for (const value of this) if (record.has.call(record.set, value)) return false;
            } else {
                for (const value of record.keys.call(record.set)) if (this.has(value)) return false;
            }
            return true;
        },
    };
    for (const name of Object.keys(methods)) {
        Object.defineProperty(Set.prototype, name, { value: methods[name], writable: true,
                                                     configurable: true, enumerable: false });
    }
}

// `Object.groupBy` and `Map.groupBy` (ES2024), over the iteration protocol; both refused by
// name natively until 2026-09-19. Defined into the constructors' stores, which every read of a
// built-in consults first.
{
    const data = (target, key, value) =>
        Object.defineProperty(target, key, { value, writable: true, configurable: true, enumerable: false });
    data(Object, "groupBy", function groupBy(items, callback) {
        if (typeof callback !== "function") throw new TypeError(String(callback) + " is not a function");
        const out = Object.create(null);
        let k = 0;
        for (const item of items) {
            const key = callback(item, k++);
            const name = typeof key === "symbol" ? key : String(key);
            if (!(name in out)) out[name] = [];
            out[name].push(item);
        }
        return out;
    });
    data(Map, "groupBy", function groupBy(items, callback) {
        if (typeof callback !== "function") throw new TypeError(String(callback) + " is not a function");
        const out = new Map();
        let k = 0;
        for (const item of items) {
            let key = callback(item, k++);
            if (key === 0) key = 0;
            const list = out.get(key);
            if (list === undefined) out.set(key, [item]);
            else list.push(item);
        }
        return out;
    });
}
"#;

#[cfg(test)]
pub(super) fn emit_native_graph_module(
    node: &kiln_module_graph::ModuleNode,
    logical_path: &str,
    policy: &NativeRealmPolicy,
) -> Result<kiln_backend_c::CModule, NativeError> {
    emit_native_graph_module_with_policy(node, logical_path, &policy.emission())
}

/// Cache before graph construction/C rendering, rather than hashing the rendered C first.
/// Only ordinary code modules with the split prelude use this cache: builtin materializers,
/// JSON and self-contained runtime bodies continue through their existing source identities.
pub(super) fn cached_graph_module(
    cache: &ArtifactCache,
    target: &str,
    node: &kiln_module_graph::ModuleNode,
    logical_path: &str,
    policy: &EmissionPolicy,
    profile: NativeProfile,
    emit: impl FnOnce() -> Result<kiln_backend_c::CModule, NativeError>,
) -> Result<(kiln_backend_c::CModule, bool), NativeError> {
    if !policy.decoupled_prelude
        || !matches!(node.source, ModuleSource::Source | ModuleSource::CommonJs)
    {
        return emit().map(|module| (module, false));
    }
    // These owned IR/policy types use derived Debug over ordered fields, Vecs and BTreeSets;
    // there are no addresses or unordered maps. Hash the complete representation so additions
    // to IR or policy automatically participate, including source spans, strictness, exports,
    // type profiles and closed-world liveness. This is a versioned internal cache encoding,
    // not a public serialization format. Representation changes can only cause cache misses.
    let (input, effective_policy) = source_module_input_id(node, logical_path, policy, profile);
    cached_emitted_module(
        cache,
        target,
        input,
        *graph_source_emitter::SOURCE_EMITTER_ID,
        || graph_source_emitter::emit(node, logical_path, &effective_policy),
    )
}

pub(super) fn source_module_input_id(
    node: &kiln_module_graph::ModuleNode,
    logical_path: &str,
    policy: &EmissionPolicy,
    profile: NativeProfile,
) -> (Digest, EmissionPolicy) {
    // Hash the final policy, including profile-dependent code-shape decisions. This ensures
    // profile, IR, source kind, path and every effective policy field participate in the key.
    let effective_policy = graph_source_emitter::effective_policy(node, policy, profile);
    let input = Digest::hash(
        format!(
            "{:?}",
            (
                &node.program,
                node.source,
                logical_path,
                profile,
                &effective_policy
            )
        )
        .as_bytes(),
    );
    (input, effective_policy)
}

fn cached_emitted_module(
    cache: &ArtifactCache,
    target: &str,
    input: Digest,
    emitter_id: Digest,
    emit: impl FnOnce() -> Result<kiln_backend_c::CModule, NativeError>,
) -> Result<(kiln_backend_c::CModule, bool), NativeError> {
    let cache_key = NativeArtifactFamily::ModuleSource.source_key(
        target,
        Digest::hash(b"native-graph-emission-input-v5"),
        [
            input,
            prelude_header_digest(),
            Digest::hash(kiln_backend_c::C_EMITTER_BUILD_ID.as_bytes()),
            emitter_id,
        ],
    );
    match cache.lookup(&cache_key)? {
        Lookup::Hit(artifact) => {
            let (program_hash, symbol, source, generic_only, chunks, parallel_workers): (
                u64,
                String,
                String,
                bool,
                Vec<String>,
                Option<String>,
            ) = serde_json::from_slice(&artifact.bytes).map_err(|error| CacheError::Corrupt {
                path: artifact.path,
                reason: format!("invalid cached C module: {error}"),
            })?;
            Ok((
                kiln_backend_c::CModule {
                    program_hash,
                    symbol,
                    source,
                    generic_only,
                    chunks,
                    parallel_workers,
                },
                true,
            ))
        }
        Lookup::Miss => {
            let module = emit()?;
            let bytes = serde_json::to_vec(&(
                module.program_hash,
                &module.symbol,
                &module.source,
                module.generic_only,
                &module.chunks,
                &module.parallel_workers,
            ))
            .expect("C module contains only strings and an integer");
            cache.put(&cache_key, &bytes)?;
            Ok((module, false))
        }
    }
}

pub(super) fn emit_native_graph_module_with_policy(
    node: &kiln_module_graph::ModuleNode,
    logical_path: &str,
    policy: &EmissionPolicy,
) -> Result<kiln_backend_c::CModule, NativeError> {
    match node.source {
        ModuleSource::NodeBuiltin(_builtin) => Ok(emit_module_with_logical_path_and_policy(
            &node.program,
            logical_path,
            policy.clone(),
        )?),
        ModuleSource::CommonJs => Ok(emit_commonjs_module_with_logical_path_and_policy(
            &node.program,
            logical_path,
            policy.clone(),
        )?),
        ModuleSource::Json => {
            let source = node.data.as_deref().ok_or_else(|| {
                NativeError::Graph(ModuleGraphError::Source {
                    path: node.path.clone(),
                    message: "JSON module has no captured source text".to_owned(),
                })
            })?;
            // `kiln_c99_json_document` answers the heap's own `kiln_val`; `exports` is an array
            // of boundary `kiln_value`, which is three times the size. Writing the answer
            // straight into the slot left the tag and length of the export unwritten, and the
            // module then handed out an object tag over a null pointer.
            let units: Vec<u16> = kiln_ir::StringUnits::from(source).units().collect();
            let latin1 = units.iter().all(|unit| *unit <= 0xff);
            let (flags, payload, type_name) = if latin1 {
                let payload = if units.is_empty() {
                    "0".to_owned()
                } else {
                    units
                        .iter()
                        .map(|unit| unit.to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                ("KILN_STRING_LATIN1", payload, "uint8_t")
            } else {
                let payload = if units.is_empty() {
                    "0".to_owned()
                } else {
                    units
                        .iter()
                        .map(|unit| format!("UINT16_C({unit})"))
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                ("KILN_STRING_UTF16", payload, "uint16_t")
            };
            let body = format!(
                "static const {type_name} json_source_data[{}] = {{ {payload} }};\n\
                 static const kiln_string json_source = {{ {}, {flags}, json_source_data }};\n\
                 kiln_val parsed;\n\
                 int status = kiln_c99_json_document(&json_source, &parsed);\n\
                 if (status != KILN_C99_OK) return status;\n\
                 exports[0] = kiln_val_to_boundary(parsed);",
                units.len().max(1),
                units.len()
            );
            Ok(emit_runtime_module_with_logical_path_and_policy(
                &node.program,
                logical_path,
                &body,
                policy.clone(),
            )?)
        }
        ModuleSource::Asset => {
            let path = node.data.as_deref().ok_or_else(|| {
                NativeError::Graph(ModuleGraphError::Source {
                    path: node.path.clone(),
                    message: "asset module has no path metadata".to_owned(),
                })
            })?;
            let body = format!(
                "exports[0] = kiln_c99_copy_text({}, (size_t){});",
                native_c_string(path),
                path.len()
            );
            Ok(emit_runtime_module_with_logical_path_and_policy(
                &node.program,
                logical_path,
                &body,
                policy.clone(),
            )?)
        }
        ModuleSource::Source => Ok(emit_module_with_logical_path_and_policy(
            &node.program,
            logical_path,
            policy.clone(),
        )?),
    }
}

struct NativeBuiltinBootstrapProgram {
    program: Result<kiln_ir::ModuleProgram, Vec<kiln_ir::CompileDiagnostic>>,
    source_digest: Digest,
    program_digest: Digest,
    provider_capabilities: ProviderCapabilities,
}

static NATIVE_BOOTSTRAP_NODE: OnceLock<NativeBuiltinBootstrapProgram> = OnceLock::new();
static NATIVE_BOOTSTRAP_URL: OnceLock<NativeBuiltinBootstrapProgram> = OnceLock::new();
static NATIVE_BOOTSTRAP_CORE: OnceLock<NativeBuiltinBootstrapProgram> = OnceLock::new();

fn compile_native_builtin_bootstrap(node: bool, retain_url: bool) -> NativeBuiltinBootstrapProgram {
    const LOGICAL_PATH: &str = "@kiln/runtime-bootstrap.ts";
    let url = if retain_url || node {
        kiln_builtins::URL_BOOTSTRAP_SOURCE
    } else {
        ""
    };
    let common_ts_builtins = kiln_builtins::common_bootstrap_source();
    let source = if node {
        format!(
            "{NATIVE_BUILTIN_BOOTSTRAP_SOURCE}\n{common_ts_builtins}\n{url}\n{}",
            kiln_builtins::WEB_BOOTSTRAP_SOURCE
        )
    } else {
        format!(
            "{NATIVE_BUILTIN_BOOTSTRAP_SOURCE}\n{common_ts_builtins}\n{url}\ndelete globalThis.__kilnBufferToString;\n"
        )
    };
    let source_digest = Digest::hash(source.as_bytes());
    let hir = kiln_frontend_oxc::parse(LOGICAL_PATH, &source);
    let program = kiln_ir::compile_module(&hir);
    let program_digest = program
        .as_ref()
        .map(|program| Digest::hash(format!("{program:?}").as_bytes()))
        .unwrap_or_default();
    let provider_capabilities = program
        .as_ref()
        .map(|program| kiln_liveness::program_provider_capabilities(program, None))
        .unwrap_or(ProviderCapabilities::NONE);
    NativeBuiltinBootstrapProgram {
        program,
        source_digest,
        program_digest,
        provider_capabilities,
    }
}

pub(super) fn native_builtin_bootstrap_module(
    cache: &ArtifactCache,
    target: &str,
    policy: &EmissionPolicy,
    node: bool,
    retain_url: bool,
) -> Result<
    (
        kiln_backend_c::CModule,
        &'static kiln_ir::ModuleProgram,
        ProviderCapabilities,
        bool,
    ),
    NativeError,
> {
    const LOGICAL_PATH: &str = "@kiln/runtime-bootstrap.ts";
    // `URL` rides the retain group liveness computed: a program that names neither it nor
    // anything built on it gets no class, and the prelude behind it compiled out. `--node`
    // always has it, because `fetch` is built on it.
    let compiled = if node {
        NATIVE_BOOTSTRAP_NODE.get_or_init(|| compile_native_builtin_bootstrap(true, true))
    } else if retain_url {
        NATIVE_BOOTSTRAP_URL.get_or_init(|| compile_native_builtin_bootstrap(false, true))
    } else {
        NATIVE_BOOTSTRAP_CORE.get_or_init(|| compile_native_builtin_bootstrap(false, false))
    };
    // The only source variants are fixed by the process build and runtime surface choice. Keep
    // compiled immutable bytecode and its source/provider analysis once per variant; every realm
    // still executes it into its own mutable objects.
    let program = compiled.program.as_ref().map_err(|diagnostics| {
        NativeError::Graph(ModuleGraphError::Compile {
            path: PathBuf::from(LOGICAL_PATH),
            diagnostics: diagnostics.clone(),
        })
    })?;
    // The bootstrap is cold code every binary carries -- the console methods, the Set algebra,
    // the URL class -- and emitted like hot user code, with a fast clone and a typed clone per
    // function, it put 0.9 MB on a hello world (2.74 MB against 1.83 MB, measured in a clean
    // cache). The generic path alone is what a program that never calls `console.table` pays for.
    let mut bootstrap_policy = policy.clone();
    bootstrap_policy.generic_only = true;
    if !bootstrap_policy.decoupled_prelude {
        let module =
            emit_module_with_logical_path_and_policy(program, LOGICAL_PATH, bootstrap_policy)?;
        return Ok((module, program, compiled.provider_capabilities, false));
    }
    // Bind cached emission to both source text and the parsed/compiled IR. The full bootstraps
    // keep their own precomputed fingerprints above, so a cache hit skips frontend work as well
    // as Value Graph construction and C rendering.
    let input = Digest::hash(
        format!(
            "{:?}",
            (
                compiled.source_digest,
                compiled.program_digest,
                ModuleSource::Source,
                LOGICAL_PATH,
                &bootstrap_policy
            )
        )
        .as_bytes(),
    );
    let (c_module, hit) =
        cached_emitted_module(cache, target, input, *bootstrap_emitter::EMITTER_ID, || {
            bootstrap_emitter::emit(program, LOGICAL_PATH, &bootstrap_policy)
        })?;
    Ok((c_module, program, compiled.provider_capabilities, hit))
}

pub(super) fn cached_payload(
    cache: &ArtifactCache,
    key: &ArtifactKey,
    expected: &[u8],
) -> Result<(Vec<u8>, Digest, bool), NativeError> {
    match cache.lookup_digest(key)? {
        Some((digest, size)) => {
            let expected_digest = Digest::hash(expected);
            if digest != expected_digest || size != expected.len() as u64 {
                match cache.lookup(key)? {
                    Lookup::Hit(artifact) if artifact.bytes != expected => {
                        return Err(CacheError::Corrupt {
                            path: artifact.path,
                            reason: format!(
                                "cached payload digest {} does not match deterministic expected digest {}",
                                artifact.digest.to_hex(),
                                expected_digest.to_hex()
                            ),
                        }
                        .into());
                    }
                    _ => {}
                }
            }
            Ok((expected.to_vec(), digest, true))
        }
        None => {
            let stored = cache.put(key, expected)?;
            Ok((expected.to_vec(), stored.digest, false))
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn module_abi_identity() -> String {
    // Generated modules carry the private value runtime in their translation unit today. Any
    // change to that source therefore changes the object ABI even when the public scalar C ABI
    // does not. Pin the generator revision here so a stale object can never be linked beside a
    // freshly generated sibling merely because both still expose C ABI v1.
    format!("host-{HOST_ABI_VERSION};c-{C_ABI_VERSION};generator-{C_GENERATOR_VERSION}")
}

pub(super) fn runtime_abi_identity() -> String {
    format!(
        "{};runtime-{NATIVE_RUNTIME_VERSION};async-{PRIVATE_ASYNC_ABI_VERSION}",
        module_abi_identity()
    )
}

pub(super) fn shared_embedding_abi_identity() -> String {
    format!(
        "{};module-api-{EMBEDDING_MODULE_API_VERSION}",
        runtime_abi_identity()
    )
}

pub(super) fn native_number_abi_identity() -> String {
    format!("kiln-native-number-provider-{PROVIDER_ABI_VERSION}")
}

pub(super) fn target_identity(identity: &ToolIdentity) -> String {
    let sdk_root = std::env::var("SDKROOT").unwrap_or_default();
    let deployment = std::env::var("MACOSX_DEPLOYMENT_TARGET").unwrap_or_default();
    format!(
        "compiler-target={};host={}-{};sdk={sdk_root};deployment={deployment}",
        identity.target,
        std::env::consts::ARCH,
        std::env::consts::OS,
    )
}

pub(super) fn compiler_identity(identity: &ToolIdentity) -> String {
    compiler_identity_from_parts(
        &identity.stable_key(),
        compiler_environment_digest(identity.kind),
    )
}

pub(super) fn compiler_identity_from_parts(tool: &str, environment: Digest) -> String {
    format!(
        "{}:{tool}|compiler-environment-sha256:{}",
        tool.len(),
        environment.to_hex()
    )
}

pub(super) fn compiler_environment_digest(kind: CompilerKind) -> Digest {
    compiler_environment_digest_with(kind, |name| std::env::var_os(name))
}

pub(super) fn compiler_environment_digest_with(
    kind: CompilerKind,
    mut value: impl FnMut(&str) -> Option<std::ffi::OsString>,
) -> Digest {
    fn frame(output: &mut Vec<u8>, bytes: &[u8]) {
        output.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
        output.extend_from_slice(bytes);
    }

    let mut encoded = Vec::new();
    encoded.extend_from_slice(b"kiln-native-compiler-environment-v1\0");
    let driver_environment: &[&str] = match kind {
        CompilerKind::Zig => &ZIG_COMPILER_ENVIRONMENT,
        CompilerKind::Gcc | CompilerKind::Cc => &GCC_COMPILER_ENVIRONMENT,
        CompilerKind::Clang => &[],
    };
    for name in C_COMPILER_ENVIRONMENT
        .into_iter()
        .chain(driver_environment.iter().copied())
    {
        frame(&mut encoded, name.as_bytes());
        match value(name) {
            Some(value) => {
                encoded.push(1);
                frame(&mut encoded, value.as_os_str().as_encoded_bytes());
            }
            None => encoded.push(0),
        }
    }
    Digest::hash(&encoded)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct LogicalModulePath(String);

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct CommonJsFilename(String);

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct CommonJsDirname(String);

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct CommonJsModulePaths {
    pub(super) filename: CommonJsFilename,
    pub(super) dirname: CommonJsDirname,
    diagnostic: LogicalModulePath,
}

impl LogicalModulePath {
    fn as_str(&self) -> &str {
        &self.0
    }
}

impl CommonJsFilename {
    fn new(path: &Path) -> Result<Self, NativeError> {
        if !path.is_absolute() {
            return Err(NativeError::Emit(EmitError::InvalidProgram {
                message: format!("CommonJS module path {} is not absolute", path.display()),
            }));
        }
        Ok(Self(path.to_string_lossy().into_owned()))
    }

    fn as_str(&self) -> &str {
        &self.0
    }

    fn as_bytes(&self) -> &[u8] {
        self.0.as_bytes()
    }
}

impl CommonJsDirname {
    fn new(path: &Path) -> Result<Self, NativeError> {
        let dirname = path.parent().ok_or_else(|| {
            NativeError::Emit(EmitError::InvalidProgram {
                message: format!("CommonJS module path {} has no parent", path.display()),
            })
        })?;
        Ok(Self(dirname.to_string_lossy().into_owned()))
    }

    fn as_str(&self) -> &str {
        &self.0
    }
}

impl CommonJsModulePaths {
    pub(super) fn new(root: &Path, path: &Path) -> Result<Self, NativeError> {
        Ok(Self {
            filename: CommonJsFilename::new(path)?,
            dirname: CommonJsDirname::new(path)?,
            diagnostic: LogicalModulePath(logical_module_path(root, path)?),
        })
    }
}

pub(super) fn logical_module_path(root: &Path, path: &Path) -> Result<String, NativeError> {
    let relative = path.strip_prefix(root).map_err(|_| {
        NativeError::Emit(EmitError::InvalidProgram {
            message: format!(
                "module path {} is outside project root {}",
                path.display(),
                root.display()
            ),
        })
    })?;
    let mut logical = String::new();
    for component in relative.components() {
        let Component::Normal(component) = component else {
            return Err(NativeError::Emit(EmitError::InvalidProgram {
                message: format!(
                    "module path {} is not a normalized project-relative path",
                    path.display()
                ),
            }));
        };
        let component = component.to_str().ok_or_else(|| {
            NativeError::Emit(EmitError::InvalidProgram {
                message: format!("module path {} is not valid UTF-8", path.display()),
            })
        })?;
        if component.is_empty() || component.contains(['\\', '\0']) {
            return Err(NativeError::Emit(EmitError::InvalidProgram {
                message: format!(
                    "module path {} is not a normalized slash-separated path",
                    path.display()
                ),
            }));
        }
        if !logical.is_empty() {
            logical.push('/');
        }
        logical.push_str(component);
    }
    if logical.is_empty() {
        return Err(NativeError::Emit(EmitError::InvalidProgram {
            message: format!(
                "module path {} has no logical relative path",
                path.display()
            ),
        }));
    }
    Ok(logical)
}

pub(super) fn digest_graph_path(graph: &ModuleGraph, path: &Path) -> String {
    logical_module_path(graph.project_root(), path)
        .unwrap_or_else(|_| path.to_string_lossy().into_owned())
}

pub(super) fn append_runtime_digest(
    bytes: &mut Vec<u8>,
    graph: &ModuleGraph,
    modules: &[(&kiln_module_graph::ModuleNode, kiln_backend_c::CModule)],
    native_runtime_failures: Option<&BTreeMap<(PathBuf, PathBuf, String), String>>,
) {
    let mut indices = std::collections::HashMap::with_capacity(modules.len());
    for (index, (node, _)) in modules.iter().enumerate() {
        indices.insert(node.path.clone(), index);
    }
    for (node, _) in modules {
        for dependency in &node.runtime_dependencies {
            bytes.extend_from_slice(b"runtime:");
            bytes.extend_from_slice(digest_graph_path(graph, &dependency.importer).as_bytes());
            bytes.push(0);
            bytes.extend_from_slice(dependency.specifier.as_bytes());
            bytes.push(0);
            bytes.extend_from_slice(digest_graph_path(graph, &dependency.target).as_bytes());
            bytes.push(0);
            bytes.extend_from_slice(
                indices
                    .get(&dependency.target)
                    .copied()
                    .unwrap_or(usize::MAX)
                    .to_string()
                    .as_bytes(),
            );
            bytes.push(0);
        }
        for ((captured, specifier), message) in graph.runtime_failures(&node.path) {
            bytes.extend_from_slice(b"runtime-failure:");
            bytes.extend_from_slice(digest_graph_path(graph, &node.path).as_bytes());
            bytes.push(0);
            bytes.extend_from_slice(digest_graph_path(graph, captured).as_bytes());
            bytes.push(0);
            bytes.extend_from_slice(specifier.as_bytes());
            bytes.push(0);
            bytes.extend_from_slice(message.as_bytes());
            bytes.push(0);
        }
    }
    if let Some(failures) = native_runtime_failures {
        for ((owner, captured, specifier), message) in failures {
            bytes.extend_from_slice(b"native-runtime-failure:");
            bytes.extend_from_slice(digest_graph_path(graph, owner).as_bytes());
            bytes.push(0);
            bytes.extend_from_slice(digest_graph_path(graph, captured).as_bytes());
            bytes.push(0);
            bytes.extend_from_slice(specifier.as_bytes());
            bytes.push(0);
            bytes.extend_from_slice(message.as_bytes());
            bytes.push(0);
        }
    }
}

pub(super) fn graph_topology_digest(
    graph: &ModuleGraph,
    modules: &[(&kiln_module_graph::ModuleNode, kiln_backend_c::CModule)],
    native_failures: Option<&BTreeMap<(PathBuf, String), String>>,
    native_runtime_failures: Option<&BTreeMap<(PathBuf, PathBuf, String), String>>,
) -> Digest {
    let mut bytes = Vec::new();
    let mut indices = std::collections::HashMap::with_capacity(modules.len());
    for (index, (node, _)) in modules.iter().enumerate() {
        indices.insert(node.path.clone(), index);
    }
    let entry_index = modules
        .iter()
        .position(|(node, _)| node.path == graph.entry())
        .unwrap_or(usize::MAX);
    bytes.extend_from_slice(entry_index.to_string().as_bytes());
    bytes.push(0);
    for (node, module) in modules {
        bytes.extend_from_slice(module.symbol.as_bytes());
        bytes.push(0);
        for dependency in &node.dependencies {
            bytes.extend_from_slice(dependency.specifier.as_bytes());
            bytes.push(0);
            let index = indices
                .get(&dependency.target)
                .copied()
                .unwrap_or(usize::MAX);
            bytes.extend_from_slice(index.to_string().as_bytes());
            bytes.push(0);
        }
        for dependency in graph.dynamic_dependencies(&node.path) {
            bytes.extend_from_slice(b"dynamic:");
            bytes.extend_from_slice(dependency.specifier.as_bytes());
            bytes.push(0);
            let index = indices
                .get(&dependency.target)
                .copied()
                .unwrap_or(usize::MAX);
            bytes.extend_from_slice(index.to_string().as_bytes());
            bytes.push(0);
        }
        for import in &node.program.imports {
            bytes.extend_from_slice(import.source.as_bytes());
            bytes.push(0);
            bytes.extend_from_slice(import.imported.as_bytes());
            bytes.push(0);
            bytes.extend_from_slice(import.local.as_bytes());
            bytes.push(0);
            bytes.extend_from_slice(import.local_slot.to_string().as_bytes());
            bytes.push(0);
        }
        for export in &node.program.exports {
            bytes.extend_from_slice(export.exported.as_bytes());
            bytes.push(0);
            bytes.extend_from_slice(export.local_slot.to_string().as_bytes());
            bytes.push(0);
        }
    }
    for ((importer, specifier), message) in graph.dynamic_failures() {
        bytes.extend_from_slice(b"dynamic-failure:");
        bytes.extend_from_slice(importer.to_string_lossy().as_bytes());
        bytes.push(0);
        bytes.extend_from_slice(specifier.as_bytes());
        bytes.push(0);
        bytes.extend_from_slice(message.as_bytes());
        bytes.push(0);
    }
    if let Some(failures) = native_failures {
        for ((importer, specifier), message) in failures {
            bytes.extend_from_slice(b"native-dynamic-failure:");
            bytes.extend_from_slice(importer.to_string_lossy().as_bytes());
            bytes.push(0);
            bytes.extend_from_slice(specifier.as_bytes());
            bytes.push(0);
            bytes.extend_from_slice(message.as_bytes());
            bytes.push(0);
        }
    }
    append_runtime_digest(&mut bytes, graph, modules, native_runtime_failures);
    Digest::hash(&bytes)
}

/// Identity of the graph wiring used by the shared wrapper. Local bytecode slot assignments are
/// deliberately excluded: changing a module body may renumber those private slots without
/// changing the provider/import topology or the wrapper source.
pub(super) fn graph_shared_topology_digest(
    graph: &ModuleGraph,
    modules: &[(&kiln_module_graph::ModuleNode, kiln_backend_c::CModule)],
    native_failures: Option<&BTreeMap<(PathBuf, String), String>>,
    native_runtime_failures: Option<&BTreeMap<(PathBuf, PathBuf, String), String>>,
) -> Digest {
    let mut bytes = Vec::new();
    let mut indices = std::collections::HashMap::with_capacity(modules.len());
    for (index, (node, _)) in modules.iter().enumerate() {
        indices.insert(node.path.clone(), index);
    }
    let entry_index = modules
        .iter()
        .position(|(node, _)| node.path == graph.entry())
        .unwrap_or(usize::MAX);
    bytes.extend_from_slice(entry_index.to_string().as_bytes());
    bytes.push(0);
    for (node, module) in modules {
        bytes.extend_from_slice(module.symbol.as_bytes());
        bytes.push(0);
        for dependency in &node.dependencies {
            bytes.extend_from_slice(dependency.specifier.as_bytes());
            bytes.push(0);
            let index = indices
                .get(&dependency.target)
                .copied()
                .unwrap_or(usize::MAX);
            bytes.extend_from_slice(index.to_string().as_bytes());
            bytes.push(0);
        }
        for dependency in graph.dynamic_dependencies(&node.path) {
            bytes.extend_from_slice(b"dynamic:");
            bytes.extend_from_slice(dependency.specifier.as_bytes());
            bytes.push(0);
            let index = indices
                .get(&dependency.target)
                .copied()
                .unwrap_or(usize::MAX);
            bytes.extend_from_slice(index.to_string().as_bytes());
            bytes.push(0);
        }
        for import in &node.program.imports {
            bytes.extend_from_slice(import.source.as_bytes());
            bytes.push(0);
            bytes.extend_from_slice(import.imported.as_bytes());
            bytes.push(0);
        }
        for export in &node.program.exports {
            bytes.extend_from_slice(export.exported.as_bytes());
            bytes.push(0);
        }
    }
    for ((importer, specifier), message) in graph.dynamic_failures() {
        bytes.extend_from_slice(b"dynamic-failure:");
        bytes.extend_from_slice(importer.to_string_lossy().as_bytes());
        bytes.push(0);
        bytes.extend_from_slice(specifier.as_bytes());
        bytes.push(0);
        bytes.extend_from_slice(message.as_bytes());
        bytes.push(0);
    }
    if let Some(failures) = native_failures {
        for ((importer, specifier), message) in failures {
            bytes.extend_from_slice(b"native-dynamic-failure:");
            bytes.extend_from_slice(importer.to_string_lossy().as_bytes());
            bytes.push(0);
            bytes.extend_from_slice(specifier.as_bytes());
            bytes.push(0);
            bytes.extend_from_slice(message.as_bytes());
            bytes.push(0);
        }
    }
    append_runtime_digest(&mut bytes, graph, modules, native_runtime_failures);
    Digest::hash(&bytes)
}

/// Derive the public graph wrapper identity from topology, ordered module bodies, and every
/// absolute CommonJS runtime path embedded by the wrapper. Private module symbols intentionally
/// remain topology-based, but a public wrapper must not collide with another DSO containing the
/// same logical graph and bytecode but different observable `__filename`/`__dirname` values.
pub(super) fn graph_shared_public_digest(
    topology: Digest,
    modules: &[(&kiln_module_graph::ModuleNode, kiln_backend_c::CModule)],
) -> Result<Digest, NativeError> {
    let mut bytes = topology.to_hex().into_bytes();
    bytes.push(0);
    for (_, module) in modules {
        let digest = Digest::hash(module.source.as_bytes());
        bytes.extend_from_slice(digest.to_hex().as_bytes());
        bytes.push(0);
    }
    if modules
        .iter()
        .any(|(node, _)| node.source == ModuleSource::CommonJs)
    {
        bytes.extend_from_slice(b"commonjs-runtime-paths-v1\0");
        for (node, _) in modules {
            if node.source != ModuleSource::CommonJs {
                continue;
            }
            let filename = CommonJsFilename::new(&node.path)?;
            bytes.extend_from_slice(&(filename.as_bytes().len() as u64).to_le_bytes());
            bytes.extend_from_slice(filename.as_bytes());
        }
    }
    Ok(Digest::hash(&bytes))
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum NativeGraphImport {
    Internal,
    Binding {
        provider_index: usize,
        export_index: usize,
    },
    Namespace {
        provider_index: usize,
    },
    CommonJsNamed {
        provider_index: usize,
        property: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum NativeNamespaceValue {
    Binding {
        provider_index: usize,
        export_index: usize,
    },
    Namespace {
        provider_index: usize,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct NativeNamespaceEntry {
    name: String,
    value: NativeNamespaceValue,
}

/// Resolve one canonical binding slot from the graph's export table.
///
/// Raw provider export tables are insufficient here: `export *` deliberately has only one
/// synthetic entry, while the graph linker resolves each forwarded name to its canonical origin.
pub(super) fn graph_export_slot(
    modules: &[(&kiln_module_graph::ModuleNode, kiln_backend_c::CModule)],
    indices: &std::collections::HashMap<PathBuf, usize>,
    importer: &Path,
    origin: &Path,
    local_slot: u32,
    exported_name: &str,
) -> Result<(usize, usize), NativeError> {
    let provider_index = *indices.get(origin).ok_or_else(|| {
        NativeError::Graph(ModuleGraphError::InternalMissingModule {
            path: origin.to_path_buf(),
        })
    })?;
    /* A CommonJS module publishes its entire `module.exports` projection in slot zero. Named
     * re-export aliases still resolve to that same origin even though their final spelling is no
     * longer `default`. */
    if modules[provider_index].0.source == ModuleSource::CommonJs {
        return Ok((provider_index, 0));
    }
    let export_index = modules[provider_index]
        .0
        .program
        .exports
        .iter()
        .position(|export| {
            export.local_slot == local_slot
                && !export.exported.starts_with(kiln_ir::STAR_REEXPORT_PREFIX)
        })
        .ok_or_else(|| {
            NativeError::Graph(ModuleGraphError::MissingExport {
                importer: importer.to_path_buf(),
                provider: origin.to_path_buf(),
                name: exported_name.to_owned(),
            })
        })?;
    Ok((provider_index, export_index))
}

/// Resolve one module import to either its concrete export-array slot or one graph namespace.
///
/// The synthetic namespace import used solely to carry a star table remains internal. A visible
/// namespace is a single snapshot object built after its provider finishes, then shared by every
/// importer during this graph invocation.
pub(super) fn graph_import_value(
    graph: &ModuleGraph,
    modules: &[(&kiln_module_graph::ModuleNode, kiln_backend_c::CModule)],
    indices: &std::collections::HashMap<PathBuf, usize>,
    node: &kiln_module_graph::ModuleNode,
    import: &kiln_ir::ImportBinding,
) -> Result<NativeGraphImport, NativeError> {
    /* CommonJS's five wrapper bindings are supplied by the graph launcher, not by another
     * module node. Keeping this sentinel out of ordinary dependency resolution is what lets the
     * launcher preserve one cache identity for `exports`/`module.exports` and attach a require
     * function whose target table is scoped to this importer. */
    if node.source == ModuleSource::CommonJs && import.source == "*commonjs-wrapper*" {
        return Ok(NativeGraphImport::Internal);
    }
    let dependency = node
        .dependencies
        .iter()
        .find(|dependency| dependency.specifier == import.source)
        .ok_or_else(|| {
            NativeError::Graph(ModuleGraphError::UnresolvedImport {
                importer: node.path.clone(),
                source: import.source.clone(),
            })
        })?;
    if import.imported == kiln_ir::NAMESPACE_IMPORT {
        let internal_star_table = node.program.exports.iter().any(|export| {
            export.local_slot == import.local_slot
                && export.exported.starts_with(kiln_ir::STAR_REEXPORT_PREFIX)
        });
        if internal_star_table {
            return Ok(NativeGraphImport::Internal);
        }
        let provider_index = *indices.get(&dependency.target).ok_or_else(|| {
            NativeError::Graph(ModuleGraphError::InternalMissingModule {
                path: dependency.target.clone(),
            })
        })?;
        let importer_index = *indices.get(&node.path).ok_or_else(|| {
            NativeError::Graph(ModuleGraphError::InternalMissingModule {
                path: node.path.clone(),
            })
        })?;
        if provider_index >= importer_index {
            return Err(NativeError::Emit(EmitError::Unsupported {
                feature: "self or cyclic namespace imports in native module graphs".to_owned(),
            }));
        }
        return Ok(NativeGraphImport::Namespace { provider_index });
    }

    let (origin, local_slot) = match graph.resolved_export(&dependency.target, &import.imported) {
        Some(ResolvedExport::CommonJsNamed { module, property }) => {
            let provider_index = *indices.get(module).ok_or_else(|| {
                NativeError::Graph(ModuleGraphError::InternalMissingModule {
                    path: module.clone(),
                })
            })?;
            let importer_index = *indices.get(&node.path).ok_or_else(|| {
                NativeError::Graph(ModuleGraphError::InternalMissingModule {
                    path: node.path.clone(),
                })
            })?;
            if provider_index >= importer_index {
                return Err(NativeError::Emit(EmitError::Unsupported {
                    feature: format!(
                        "`{}`, imported across a module cycle before `{}` has run, in native module graphs",
                        property,
                        module.display()
                    ),
                }));
            }
            return Ok(NativeGraphImport::CommonJsNamed {
                provider_index,
                property: property.clone(),
            });
        }
        Some(ResolvedExport::Local { module, local_slot }) => (module, *local_slot),
        Some(ResolvedExport::Namespace { module }) => {
            let provider_index = *indices.get(module).ok_or_else(|| {
                NativeError::Graph(ModuleGraphError::InternalMissingModule {
                    path: module.clone(),
                })
            })?;
            let importer_index = *indices.get(&node.path).ok_or_else(|| {
                NativeError::Graph(ModuleGraphError::InternalMissingModule {
                    path: node.path.clone(),
                })
            })?;
            if provider_index >= importer_index {
                return Err(NativeError::Emit(EmitError::Unsupported {
                    feature: "self or cyclic namespace re-exports in native module graphs"
                        .to_owned(),
                }));
            }
            return Ok(NativeGraphImport::Namespace { provider_index });
        }
        Some(ResolvedExport::Ambiguous) => {
            return Err(NativeError::Graph(ModuleGraphError::AmbiguousExport {
                importer: node.path.clone(),
                provider: dependency.target.clone(),
                name: import.imported.clone(),
            }));
        }
        None => {
            return Err(NativeError::Graph(ModuleGraphError::MissingExport {
                importer: node.path.clone(),
                provider: dependency.target.clone(),
                name: import.imported.clone(),
            }));
        }
    };
    let (provider_index, export_index) = graph_export_slot(
        modules,
        indices,
        &node.path,
        origin,
        local_slot,
        &import.imported,
    )?;
    /* The launcher is straight-line C: it calls each module in order and takes an importer's
     * values out of the export array the provider already returned. A cycle can make the origin
     * of a name come *after* the module that imports it, and there is no array to read then --
     * the generated code would read one that has not been filled. The interpreter answers that
     * with a binding cell it fills in later; this backend has no such thing, so it refuses by
     * name. A cyclic graph whose back edges carry no binding still compiles, because nothing in
     * it reads forward. */
    let importer_index = *indices.get(&node.path).ok_or_else(|| {
        NativeError::Graph(ModuleGraphError::InternalMissingModule {
            path: node.path.clone(),
        })
    })?;
    if provider_index >= importer_index {
        return Err(NativeError::Emit(EmitError::Unsupported {
            feature: format!(
                "`{}`, imported across a module cycle before `{}` has run, in native module \
                 graphs",
                import.imported,
                origin.display()
            ),
        }));
    }
    Ok(NativeGraphImport::Binding {
        provider_index,
        export_index,
    })
}

/// The sorted snapshot contents each graph module would expose through `import * as`.
pub(super) fn graph_namespace_plans(
    graph: &ModuleGraph,
    modules: &[(&kiln_module_graph::ModuleNode, kiln_backend_c::CModule)],
    indices: &std::collections::HashMap<PathBuf, usize>,
    live_imports: Option<&BTreeSet<(PathBuf, usize)>>,
    partial_namespace_members: Option<&BTreeMap<PathBuf, BTreeSet<String>>>,
) -> Result<Vec<Option<Vec<NativeNamespaceEntry>>>, NativeError> {
    let has_any_dynamic_import = modules
        .iter()
        .any(|(node, _)| node.program.program.has_dynamic_import());
    let mut required = vec![false; modules.len()];
    for (node, _) in modules {
        for (import_index, import) in node.program.imports.iter().enumerate() {
            if live_imports
                .is_some_and(|imports| !imports.contains(&(node.path.clone(), import_index)))
            {
                continue;
            }
            if let NativeGraphImport::Namespace { provider_index } =
                graph_import_value(graph, modules, indices, node, import)?
            {
                required[provider_index] = true;
            }
        }
    }
    if has_any_dynamic_import {
        for (idx, (node, _)) in modules.iter().enumerate() {
            if node.source != ModuleSource::CommonJs {
                required[idx] = true;
            }
        }
    }

    let mut plans = vec![None; modules.len()];
    /* Dependencies precede importers. Walking backwards lets a required barrel namespace mark
     * the namespace it re-exports before that provider is planned. */
    for module_index in (0..modules.len()).rev() {
        if !required[module_index] {
            continue;
        }
        let node = modules[module_index].0;
        if node.source == ModuleSource::CommonJs {
            plans[module_index] = None;
            continue;
        }
        let mut names = graph.resolved_export_names(&node.path).ok_or_else(|| {
            NativeError::Graph(ModuleGraphError::InternalMissingModule {
                path: node.path.clone(),
            })
        })?;
        if !has_any_dynamic_import {
            if let Some(members) = partial_namespace_members.and_then(|all| all.get(&node.path)) {
                names.retain(|name| members.contains(name));
            }
        }
        let entries = names
            .into_iter()
            .map(|name| {
                if name.starts_with("@@") {
                    return Err(NativeError::Emit(EmitError::Unsupported {
                        feature: "@@-prefixed exports in native module namespaces".to_owned(),
                    }));
                }
                let value = match graph.resolved_export(&node.path, &name) {
                    Some(ResolvedExport::Local { module, local_slot }) => {
                        let (provider_index, export_index) = graph_export_slot(
                            modules,
                            indices,
                            &node.path,
                            module,
                            *local_slot,
                            &name,
                        )?;
                        NativeNamespaceValue::Binding {
                            provider_index,
                            export_index,
                        }
                    }
                    Some(ResolvedExport::CommonJsNamed { module, .. }) => {
                        let (provider_index, export_index) =
                            graph_export_slot(modules, indices, &node.path, module, 0, &name)?;
                        NativeNamespaceValue::Binding {
                            provider_index,
                            export_index,
                        }
                    }
                    Some(ResolvedExport::Namespace { module }) => {
                        let provider_index = *indices.get(module).ok_or_else(|| {
                            NativeError::Graph(ModuleGraphError::InternalMissingModule {
                                path: module.clone(),
                            })
                        })?;
                        if provider_index >= module_index {
                            return Err(NativeError::Emit(EmitError::Unsupported {
                                feature:
                                    "self or cyclic namespace re-exports in native module graphs"
                                        .to_owned(),
                            }));
                        }
                        required[provider_index] = true;
                        NativeNamespaceValue::Namespace { provider_index }
                    }
                    Some(ResolvedExport::Ambiguous) | None => {
                        return Err(NativeError::Graph(ModuleGraphError::MissingExport {
                            importer: node.path.clone(),
                            provider: node.path.clone(),
                            name: name.clone(),
                        }));
                    }
                };
                Ok(NativeNamespaceEntry { name, value })
            })
            .collect::<Result<Vec<_>, NativeError>>()?;
        plans[module_index] = Some(entries);
    }
    Ok(plans)
}

pub(super) fn native_graph_export_count(node: &kiln_module_graph::ModuleNode) -> usize {
    if node.source == ModuleSource::CommonJs {
        1
    } else if let ModuleSource::NodeBuiltin(_builtin) = node.source {
        node.program.exports.len()
    } else {
        node.program.exports.len()
    }
}

/// The slot whose value CommonJS receives from `require()` for a typed builtin.
///
/// Typed builtins reserve one private slot after their public ESM exports. That slot is the only
/// CommonJS projection; loaders never guess that `default` or export zero happens to be suitable.
pub(super) fn native_commonjs_projection_index(
    node: &kiln_module_graph::ModuleNode,
) -> Option<usize> {
    match node.source {
        ModuleSource::Json | ModuleSource::Asset => Some(0),
        ModuleSource::NodeBuiltin(_) => node
            .program
            .exports
            .iter()
            .position(|e| e.exported == "default"),
        ModuleSource::Source | ModuleSource::CommonJs => None,
    }
}

pub(super) fn native_c_string(value: &str) -> String {
    let mut output = String::from("\"");
    for byte in value.bytes() {
        match byte {
            b'\"' => output.push_str("\\\""),
            b'\\' => output.push_str("\\\\"),
            b'\n' => output.push_str("\\n"),
            b'\r' => output.push_str("\\r"),
            b'\t' => output.push_str("\\t"),
            0x20..=0x7e => output.push(char::from(byte)),
            _ => write!(output, "\\x{byte:02X}\"\"").unwrap(),
        }
    }
    output.push('\"');
    output
}

pub(super) fn find_opentui_library(project_root: &Path) -> Option<PathBuf> {
    if let Some(env_path) = std::env::var_os("OPENTUI_LIB_PATH") {
        let p = PathBuf::from(env_path);
        if p.exists() {
            return Some(p);
        }
    }
    let standard = project_root.join("node_modules/@opentui/core-linux-x64/libopentui.so");
    if standard.exists() {
        return Some(standard);
    }
    let standard_musl =
        project_root.join("node_modules/@opentui/core-linux-x64-musl/libopentui.so");
    if standard_musl.exists() {
        return Some(standard_musl);
    }
    let bun_dir = project_root.join("node_modules/.bun");
    if bun_dir.is_dir() {
        if let Ok(entries) = fs::read_dir(&bun_dir) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name_str = name.to_string_lossy();
                if name_str.starts_with("@opentui+core-linux-x64") {
                    let candidate = entry
                        .path()
                        .join("node_modules/@opentui/core-linux-x64/libopentui.so");
                    if candidate.exists() {
                        return Some(candidate);
                    }
                    let candidate_musl = entry
                        .path()
                        .join("node_modules/@opentui/core-linux-x64-musl/libopentui.so");
                    if candidate_musl.exists() {
                        return Some(candidate_musl);
                    }
                }
            }
        }
    }
    let nm = project_root.join("node_modules");
    if nm.is_dir() {
        let mut queue = std::collections::VecDeque::new();
        queue.push_back((nm, 0usize));
        while let Some((dir, depth)) = queue.pop_front() {
            if depth > 4 {
                continue;
            }
            if let Ok(entries) = fs::read_dir(&dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_file() {
                        if let Some(file_name) = path.file_name() {
                            if file_name == "libopentui.so" || file_name == "libopentui.dylib" {
                                return Some(path);
                            }
                        }
                    } else if path.is_dir() {
                        queue.push_back((path, depth + 1));
                    }
                }
            }
        }
    }
    None
}

pub(super) fn write_dynamic_failure_dispatch(
    output: &mut String,
    graph: &ModuleGraph,
    modules: &[(&kiln_module_graph::ModuleNode, kiln_backend_c::CModule)],
    indent: &str,
    native_failures: Option<&BTreeMap<(PathBuf, String), String>>,
) {
    let native_iter = native_failures.into_iter().flat_map(BTreeMap::iter);
    for ((importer_path, specifier), message) in graph.dynamic_failures().chain(native_iter) {
        let Some(importer_index) = modules
            .iter()
            .position(|(node, _)| node.path == *importer_path)
        else {
            continue;
        };
        let importer = native_c_string(&modules[importer_index].1.symbol);
        let c_spec = native_c_string(specifier);
        let c_message = native_c_string(message);
        writeln!(
            output,
            "{indent}if (importer != NULL && strcmp(importer, {importer}) == 0 && kiln_c99_string_equals_ascii(specifier, {c_spec}, (size_t){})) {{ kiln_value err = kiln_c99_undefined(); (void)kiln_c99_new_type_error(&err, {c_message}); return kiln_c99_invoke_method(host, promise, KILN_C99_METHOD_PROMISE_REJECT, kiln_c99_undefined(), &err, 1, depth, thrown); }}",
            specifier.len()
        )
        .unwrap();
    }
}

pub(super) fn write_launcher_dynamic_import(
    output: &mut String,
    graph: &ModuleGraph,
    modules: &[(&kiln_module_graph::ModuleNode, kiln_backend_c::CModule)],
    logical_paths: &[String],
    native_failures: Option<&BTreeMap<(PathBuf, String), String>>,
) {
    let opentui_path = find_opentui_library(graph.project_root());
    output.push_str(
        &r#"
typedef struct kiln_graph_context kiln_graph_context;
static kiln_value *kiln_graph_module_result(kiln_graph_context *graph, size_t index);
static int kiln_graph_ensure_module(kiln_graph_context *graph, size_t index);
static kiln_value *kiln_graph_module_promise(kiln_graph_context *graph, size_t index);
int kiln_graph_module_completion(kiln_host *host, uint32_t index, int rejected, const kiln_value *value, kiln_value *result, int depth, kiln_value *thrown);
extern int kiln_c99_module_completion(kiln_host *, kiln_value, uint32_t, kiln_value *, int, kiln_value *);
extern int kiln_c99_finish_module_checkpoint(kiln_host *, kiln_value *);
#define KILN_C99_METHOD_THEN 107
extern int kiln_c99_validate_dynamic_import_options(kiln_host *, kiln_value, int, kiln_value *, int *);
int kiln_graph_dynamic_import(kiln_host *host, const char *importer, kiln_value specifier, kiln_value options, kiln_value *promise, int depth, kiln_value *thrown) {
    /* The graph whose host this is: its namespaces are the realm's. */
    kiln_graph_context *graph = host != NULL ? (kiln_graph_context *)host->context : NULL;
    if (promise == NULL || graph == NULL) return KILN_C99_CONTROL_ERROR;
    if (specifier.tag != KILN_VALUE_STRING) {
        kiln_value primitive = kiln_c99_undefined();
        int specifier_status = kiln_c99_to_primitive(host, specifier, KILN_C99_HINT_STRING, &primitive, depth, thrown);
        if (specifier_status != KILN_C99_OK) {
            if (thrown != NULL) return kiln_c99_invoke_method(host, promise, KILN_C99_METHOD_PROMISE_REJECT, kiln_c99_undefined(), thrown, 1, depth, thrown);
            return specifier_status;
        }
        specifier = kiln_c99_text(primitive);
    }
    if (specifier.tag != KILN_VALUE_STRING || (specifier.len != 0 && specifier.ptr == NULL)) {
        kiln_value err = kiln_c99_undefined();
        (void)kiln_c99_new_type_error(&err, "invalid import specifier");
        return kiln_c99_invoke_method(host, promise, KILN_C99_METHOD_PROMISE_REJECT, kiln_c99_undefined(), &err, 1, depth, thrown);
    }
    int attribute_type = 0;
    { kiln_value validation_thrown = kiln_c99_undefined();
      int attr_status = kiln_c99_validate_dynamic_import_options(host, options, depth, &validation_thrown, &attribute_type);
      if (attr_status != KILN_C99_OK) {
          kiln_value *reason = attr_status == KILN_C99_THROWN ? &validation_thrown : &validation_thrown;
          return kiln_c99_invoke_method(host, promise, KILN_C99_METHOD_PROMISE_REJECT, kiln_c99_undefined(), reason, 1, depth, thrown);
      }
    }
import_attributes_valid:
    if (kiln_c99_string_starts_ascii(specifier, "@opentui/core-", 14)) {
        if (kiln_opentui_namespace.tag != KILN_VALUE_UNDEFINED) {
            return kiln_c99_invoke_method(host, promise, KILN_C99_METHOD_PROMISE_RESOLVE, kiln_c99_undefined(), &kiln_opentui_namespace, 1, depth, thrown);
        }
        static const char *opentui_candidates[] = {
"#,
    );
    if let Some(path) = &opentui_path {
        writeln!(
            output,
            "            {},",
            native_c_string(&path.to_string_lossy())
        )
        .unwrap();
    }
    output.push_str(
        r#"            "./libopentui.so",
            "./libopentui.dylib",
            "./node_modules/@opentui/core-linux-x64/libopentui.so",
            "./node_modules/.bun/@opentui+core-linux-x64@0.4.2/node_modules/@opentui/core-linux-x64/libopentui.so",
            "./node_modules/.bun/@opentui+core-linux-x64-musl@0.4.2/node_modules/@opentui/core-linux-x64-musl/libopentui.so",
            "/usr/local/lib/libopentui.so",
            "/usr/lib/libopentui.so"
        };
        const char *so_path = getenv("OPENTUI_LIB_PATH");
        if (so_path != NULL && access(so_path, F_OK) != 0) {
            so_path = NULL;
        }
        if (so_path == NULL) {
            for (size_t i = 0; i < sizeof(opentui_candidates) / sizeof(opentui_candidates[0]); ++i) {
                if (opentui_candidates[i] != NULL && access(opentui_candidates[i], F_OK) == 0) {
                    so_path = opentui_candidates[i];
                    break;
                }
            }
        }
        if (so_path == NULL) {
            so_path = opentui_candidates[0];
        }
        kiln_value ns_obj;
        int status = kiln_c99_new_object(&ns_obj);
        if (status == KILN_C99_OK) {
            kiln_value def_val = kiln_c99_copy_text(so_path, strlen(so_path));
            status = kiln_c99_put(host, ns_obj, "default", 7, def_val, depth, thrown);
        }
        if (status != KILN_C99_OK) return status;
        kiln_opentui_namespace = ns_obj;
        return kiln_c99_invoke_method(host, promise, KILN_C99_METHOD_PROMISE_RESOLVE, kiln_c99_undefined(), &kiln_opentui_namespace, 1, depth, thrown);
    }
    if (kiln_c99_string_equals_ascii(specifier, "node:ffi", 8) ||
        kiln_c99_string_equals_ascii(specifier, "ffi", 3)) {
        if (kiln_node_ffi_namespace.tag != KILN_VALUE_UNDEFINED) {
            return kiln_c99_invoke_method(host, promise, KILN_C99_METHOD_PROMISE_RESOLVE, kiln_c99_undefined(), &kiln_node_ffi_namespace, 1, depth, thrown);
        }
        kiln_value dlopen = (kiln_value){ .tag = 11, .flags = 0, .len = 0, .bits = 495 };
        kiln_value get_raw_pointer = (kiln_value){ .tag = 11, .flags = 0, .len = 0, .bits = 496 };
        kiln_value to_array_buffer = (kiln_value){ .tag = 11, .flags = 0, .len = 0, .bits = 497 };
#if defined(_WIN32)
        kiln_value suffix = kiln_c99_copy_text(".dll", 4);
#elif defined(__APPLE__)
        kiln_value suffix = kiln_c99_copy_text(".dylib", 6);
#else
        kiln_value suffix = kiln_c99_copy_text(".so", 3);
#endif
        kiln_value ns_obj;
        int status = kiln_c99_new_object(&ns_obj);
        if (status == KILN_C99_OK) {
            status = kiln_c99_put(host, ns_obj, "default", 7, ns_obj, depth, thrown);
        }
        if (status == KILN_C99_OK) {
            status = kiln_c99_put(host, ns_obj, "dlopen", 6, dlopen, depth, thrown);
        }
        if (status == KILN_C99_OK) {
            status = kiln_c99_put(host, ns_obj, "getRawPointer", 13, get_raw_pointer, depth, thrown);
        }
        if (status == KILN_C99_OK) {
            status = kiln_c99_put(host, ns_obj, "suffix", 6, suffix, depth, thrown);
        }
        if (status == KILN_C99_OK) {
            status = kiln_c99_put(host, ns_obj, "toArrayBuffer", 13, to_array_buffer, depth, thrown);
        }
        if (status != KILN_C99_OK) return status;
        kiln_node_ffi_namespace = ns_obj;
        return kiln_c99_invoke_method(host, promise, KILN_C99_METHOD_PROMISE_RESOLVE, kiln_c99_undefined(), &kiln_node_ffi_namespace, 1, depth, thrown);
    }
"#,
    );
    write_dynamic_failure_dispatch(output, graph, modules, "    ", native_failures);
    for (index, (node, _)) in modules.iter().enumerate() {
        let mut resolutions = std::collections::BTreeSet::new();
        resolutions.insert((logical_paths[index].clone(), index));
        if let Some(stripped) = logical_paths[index].strip_prefix("./") {
            resolutions.insert((stripped.to_owned(), index));
        } else {
            resolutions.insert((format!("./{}", logical_paths[index]), index));
        }
        if let ModuleSource::NodeBuiltin(builtin) = node.source {
            resolutions.insert((builtin.specifier().to_owned(), index));
            if let Some(stripped) = builtin.specifier().strip_prefix("node:") {
                resolutions.insert((stripped.to_owned(), index));
            }
        }
        for (other_index, (other_node, _)) in modules.iter().enumerate() {
            for dep in graph.dynamic_dependencies(&other_node.path) {
                if dep.target == node.path {
                    resolutions.insert((dep.specifier.clone(), other_index));
                }
            }
        }
        for (spec, importer_index) in resolutions {
            let c_spec = native_c_string(&spec);
            let len = spec.len();
            let importer = native_c_string(&modules[importer_index].1.symbol);
            let attribute_guard = if matches!(node.source, ModuleSource::Asset) {
                " && attribute_type == 1"
            } else {
                " && attribute_type == 0"
            };
            writeln!(
                output,
                "    if (importer != NULL && strcmp(importer, {importer}) == 0{attribute_guard} && kiln_c99_string_equals_ascii(specifier, {c_spec}, (size_t){len})) {{"
            )
            .unwrap();
            writeln!(
                output,
                "        int status = kiln_graph_ensure_module((kiln_graph_context *)host->context, (size_t){index});\n        if (status != KILN_C99_OK) return kiln_c99_invoke_method(host, promise, KILN_C99_METHOD_PROMISE_REJECT, kiln_c99_undefined(), kiln_graph_module_result((kiln_graph_context *)host->context, (size_t){index}), 1, depth, thrown);\n        if (kiln_graph_module_promise((kiln_graph_context *)host->context, (size_t){index})->tag == 16 /* KILN_VALUE_PROMISE */) return kiln_c99_invoke_method(host, promise, KILN_C99_METHOD_THEN, *kiln_graph_module_promise((kiln_graph_context *)host->context, (size_t){index}), NULL, 0, depth, thrown);"
            )
            .unwrap();
            if modules[index].0.source == ModuleSource::CommonJs {
                writeln!(
                    output,
                    "        kiln_value load_res;\n        status = kiln_graph_load_commonjs(&kiln_commonjs_{index}, &load_res, thrown);\n        if (status != KILN_C99_OK) return status;\n        kiln_namespaces[{index}] = kiln_exports_{index}[0];"
                )
                .unwrap();
            }
            writeln!(
                output,
                "        return kiln_c99_invoke_method(host, promise, KILN_C99_METHOD_PROMISE_RESOLVE, kiln_c99_undefined(), &kiln_namespaces[{index}], 1, depth, thrown);\n    }}"
            )
            .unwrap();
        }
    }
    output.push_str(
        r#"    {
        kiln_value err = kiln_c99_undefined();
        (void)kiln_c99_new_type_error(&err, "no module loader is installed");
        return kiln_c99_invoke_method(host, promise, KILN_C99_METHOD_PROMISE_REJECT, kiln_c99_undefined(), &err, 1, depth, thrown);
    }
}
"#,
    );
}

pub(super) fn write_shared_wrapper_dynamic_import(
    output: &mut String,
    graph: &ModuleGraph,
    modules: &[(&kiln_module_graph::ModuleNode, kiln_backend_c::CModule)],
    logical_paths: &[String],
    native_failures: &BTreeMap<(PathBuf, String), String>,
) {
    let opentui_path = find_opentui_library(graph.project_root());
    output.push_str(
        r#"
extern int kiln_c99_validate_dynamic_import_options(kiln_host *, kiln_value, int, kiln_value *, int *);
int kiln_graph_dynamic_import(kiln_host *host, const char *importer, kiln_value specifier, kiln_value options, kiln_value *promise, int depth, kiln_value *thrown) {
    if (promise == NULL) return KILN_C99_CONTROL_ERROR;
    if (specifier.tag != KILN_VALUE_STRING) {
        kiln_value primitive = kiln_c99_undefined();
        int specifier_status = kiln_c99_to_primitive(host, specifier, KILN_C99_HINT_STRING, &primitive, depth, thrown);
        if (specifier_status != KILN_C99_OK) {
            if (thrown != NULL) return kiln_c99_invoke_method(host, promise, KILN_C99_METHOD_PROMISE_REJECT, kiln_c99_undefined(), thrown, 1, depth, thrown);
            return specifier_status;
        }
        specifier = kiln_c99_text(primitive);
    }
    if (specifier.tag != KILN_VALUE_STRING || (specifier.len != 0 && specifier.ptr == NULL)) {
        kiln_value err = kiln_c99_undefined();
        (void)kiln_c99_new_type_error(&err, "invalid import specifier");
        return kiln_c99_invoke_method(host, promise, KILN_C99_METHOD_PROMISE_REJECT, kiln_c99_undefined(), &err, 1, depth, thrown);
    }
    int attribute_type = 0;
    { kiln_value validation_thrown = kiln_c99_undefined();
      int attr_status = kiln_c99_validate_dynamic_import_options(host, options, depth, &validation_thrown, &attribute_type);
      if (attr_status != KILN_C99_OK) {
          return kiln_c99_invoke_method(host, promise, KILN_C99_METHOD_PROMISE_REJECT, kiln_c99_undefined(), &validation_thrown, 1, depth, thrown);
      }
    }
import_attributes_valid:
    if (specifier.tag != KILN_VALUE_STRING || (specifier.len != 0 && specifier.ptr == NULL)) {
        kiln_value err = kiln_c99_undefined();
        (void)kiln_c99_new_type_error(&err, "invalid import specifier");
        return kiln_c99_invoke_method(host, promise, KILN_C99_METHOD_PROMISE_REJECT, kiln_c99_undefined(), &err, 1, depth, thrown);
    }
    if (kiln_c99_string_starts_ascii(specifier, "@opentui/core-", 14)) {
        if (kiln_shared_opentui_namespace.tag != KILN_VALUE_UNDEFINED) {
            return kiln_c99_invoke_method(host, promise, KILN_C99_METHOD_PROMISE_RESOLVE, kiln_c99_undefined(), &kiln_shared_opentui_namespace, 1, depth, thrown);
        }
        static const char *opentui_candidates[] = {
"#,
    );
    if let Some(path) = &opentui_path {
        writeln!(
            output,
            "            {},",
            native_c_string(&path.to_string_lossy())
        )
        .unwrap();
    }
    output.push_str(
        r#"            "./libopentui.so",
            "./libopentui.dylib",
            "./node_modules/@opentui/core-linux-x64/libopentui.so",
            "./node_modules/.bun/@opentui+core-linux-x64@0.4.2/node_modules/@opentui/core-linux-x64/libopentui.so",
            "./node_modules/.bun/@opentui+core-linux-x64-musl@0.4.2/node_modules/@opentui/core-linux-x64-musl/libopentui.so",
            "/usr/local/lib/libopentui.so",
            "/usr/lib/libopentui.so"
        };
        const char *so_path = NULL;
        for (size_t i = 0; i < sizeof(opentui_candidates) / sizeof(opentui_candidates[0]); ++i) {
            if (opentui_candidates[i] != NULL && access(opentui_candidates[i], F_OK) == 0) {
                so_path = opentui_candidates[i];
                break;
            }
        }
        if (so_path == NULL) {
            so_path = opentui_candidates[0];
        }
        kiln_value ns_obj;
        int status = kiln_c99_new_object(&ns_obj);
        if (status == KILN_C99_OK) {
            kiln_value def_val = kiln_c99_copy_text(so_path, strlen(so_path));
            status = kiln_c99_put(host, ns_obj, "default", 7, def_val, depth, thrown);
        }
        if (status != KILN_C99_OK) return status;
        kiln_shared_opentui_namespace = ns_obj;
        return kiln_c99_invoke_method(host, promise, KILN_C99_METHOD_PROMISE_RESOLVE, kiln_c99_undefined(), &kiln_shared_opentui_namespace, 1, depth, thrown);
    }
    if (kiln_c99_string_equals_ascii(specifier, "node:ffi", 8) ||
        kiln_c99_string_equals_ascii(specifier, "ffi", 3)) {
        if (kiln_shared_node_ffi_namespace.tag != KILN_VALUE_UNDEFINED) {
            return kiln_c99_invoke_method(host, promise, KILN_C99_METHOD_PROMISE_RESOLVE, kiln_c99_undefined(), &kiln_shared_node_ffi_namespace, 1, depth, thrown);
        }
        kiln_value dlopen = (kiln_value){ .tag = 11, .flags = 0, .len = 0, .bits = 495 };
        kiln_value get_raw_pointer = (kiln_value){ .tag = 11, .flags = 0, .len = 0, .bits = 496 };
        kiln_value to_array_buffer = (kiln_value){ .tag = 11, .flags = 0, .len = 0, .bits = 497 };
#if defined(_WIN32)
        kiln_value suffix = kiln_c99_copy_text(".dll", 4);
#elif defined(__APPLE__)
        kiln_value suffix = kiln_c99_copy_text(".dylib", 6);
#else
        kiln_value suffix = kiln_c99_copy_text(".so", 3);
#endif
        kiln_value ns_obj;
        int status = kiln_c99_new_object(&ns_obj);
        if (status == KILN_C99_OK) {
            status = kiln_c99_put(host, ns_obj, "default", 7, ns_obj, depth, thrown);
        }
        if (status == KILN_C99_OK) {
            status = kiln_c99_put(host, ns_obj, "dlopen", 6, dlopen, depth, thrown);
        }
        if (status == KILN_C99_OK) {
            status = kiln_c99_put(host, ns_obj, "getRawPointer", 13, get_raw_pointer, depth, thrown);
        }
        if (status == KILN_C99_OK) {
            status = kiln_c99_put(host, ns_obj, "suffix", 6, suffix, depth, thrown);
        }
        if (status == KILN_C99_OK) {
            status = kiln_c99_put(host, ns_obj, "toArrayBuffer", 13, to_array_buffer, depth, thrown);
        }
        if (status != KILN_C99_OK) return status;
        kiln_shared_node_ffi_namespace = ns_obj;
        return kiln_c99_invoke_method(host, promise, KILN_C99_METHOD_PROMISE_RESOLVE, kiln_c99_undefined(), &kiln_shared_node_ffi_namespace, 1, depth, thrown);
    }
    if (kiln_shared_namespaces != NULL) {
"#,
    );
    write_dynamic_failure_dispatch(output, graph, modules, "        ", Some(native_failures));
    for (index, (node, _)) in modules.iter().enumerate() {
        let mut resolutions = std::collections::BTreeSet::new();
        resolutions.insert((logical_paths[index].clone(), index));
        if let Some(stripped) = logical_paths[index].strip_prefix("./") {
            resolutions.insert((stripped.to_owned(), index));
        } else {
            resolutions.insert((format!("./{}", logical_paths[index]), index));
        }
        if let ModuleSource::NodeBuiltin(builtin) = node.source {
            resolutions.insert((builtin.specifier().to_owned(), index));
            if let Some(stripped) = builtin.specifier().strip_prefix("node:") {
                resolutions.insert((stripped.to_owned(), index));
            }
        }
        for (other_index, (other_node, _)) in modules.iter().enumerate() {
            for dep in graph.dynamic_dependencies(&other_node.path) {
                if dep.target == node.path {
                    resolutions.insert((dep.specifier.clone(), other_index));
                }
            }
        }
        for (spec, importer_index) in resolutions {
            let c_spec = native_c_string(&spec);
            let len = spec.len();
            let importer = native_c_string(&modules[importer_index].1.symbol);
            let attribute_guard = if matches!(node.source, ModuleSource::Asset) {
                " && attribute_type == 1"
            } else {
                " && attribute_type == 0"
            };
            writeln!(
                output,
                "        if (importer != NULL && strcmp(importer, {importer}) == 0{attribute_guard} && kiln_c99_string_equals_ascii(specifier, {c_spec}, (size_t){len})) {{"
            )
            .unwrap();
            if modules[index].0.source == ModuleSource::CommonJs {
                writeln!(
                    output,
                    "            if (kiln_shared_commonjs != NULL) {{\n                kiln_value load_res;\n                int status = kiln_graph_load_commonjs(&kiln_shared_commonjs[{index}], &load_res, thrown);\n                if (status != KILN_C99_OK) return status;\n                kiln_shared_namespaces[{index}] = kiln_shared_commonjs[{index}].exports[0];\n            }}"
                )
                .unwrap();
            }
            writeln!(
                output,
                "            return kiln_c99_invoke_method(host, promise, KILN_C99_METHOD_PROMISE_RESOLVE, kiln_c99_undefined(), &kiln_shared_namespaces[{index}], 1, depth, thrown);\n        }}"
            )
            .unwrap();
        }
    }
    output.push_str(
        r#"    }
    {
        kiln_value err = kiln_c99_undefined();
        (void)kiln_c99_new_type_error(&err, "no module loader is installed");
        return kiln_c99_invoke_method(host, promise, KILN_C99_METHOD_PROMISE_REJECT, kiln_c99_undefined(), &err, 1, depth, thrown);
    }
}
"#,
    );
}

pub(super) fn write_graph_commonjs_realm_setup(
    output: &mut String,
    index: usize,
    module_symbol: &str,
    filename: &CommonJsFilename,
    dirname: &CommonJsDirname,
    diagnostic: &LogicalModulePath,
) {
    writeln!(
        output,
        "    status = {module_symbol}_commonjs_wrapper(kiln_imports_{index}, (size_t)5, {}, (size_t){}, {}, (size_t){}, kiln_commonjs_require_{index}, graph);\n    if (status != 0) {{ *result = kiln_graph_diagnostic(\"commonjs-wrapper\", {}); return status; }}\n    kiln_exports_{index}[0] = kiln_imports_{index}[0];\n    kiln_commonjs_{index}.module = {module_symbol};\n    kiln_commonjs_{index}.host = host;\n    kiln_commonjs_{index}.imports = kiln_imports_{index};\n    kiln_commonjs_{index}.import_count = (size_t)5;\n    kiln_commonjs_{index}.exports = kiln_exports_{index};\n    kiln_commonjs_{index}.state = 0;",
        native_c_string(filename.as_str()),
        filename.as_str().len(),
        native_c_string(dirname.as_str()),
        dirname.as_str().len(),
        native_c_string(diagnostic.as_str()),
    )
    .unwrap();
}

pub(super) fn write_graph_commonjs_shared_setup(
    output: &mut String,
    index: usize,
    module_symbol: &str,
    filename: &CommonJsFilename,
    dirname: &CommonJsDirname,
) {
    writeln!(
        output,
        "    status = {module_symbol}_commonjs_wrapper(import_arrays[{index}], (size_t)5, {}, (size_t){}, {}, (size_t){}, kiln_commonjs_require_{index}, commonjs_graph);\n    if (status != KILN_C99_OK) goto cleanup;\n    export_arrays[{index}][0] = import_arrays[{index}][0];\n    commonjs_modules[{index}] = (kiln_commonjs_context){{ {module_symbol}, proxy_host, import_arrays[{index}], (size_t)5, export_arrays[{index}], 0 }};",
        native_c_string(filename.as_str()),
        filename.as_str().len(),
        native_c_string(dirname.as_str()),
        dirname.as_str().len(),
    )
    .unwrap();
}

pub(super) fn write_graph_namespace_build(
    output: &mut String,
    module_index: usize,
    module_symbol: &str,
    entries: &[NativeNamespaceEntry],
    shared: bool,
) {
    let namespaces = if shared {
        "namespaces"
    } else {
        "kiln_namespaces"
    };
    if entries.is_empty() {
        writeln!(
            output,
            "    status = {module_symbol}_namespace_wrapper(&{namespaces}[{module_index}], NULL, NULL, NULL, (size_t)0);"
        )
        .unwrap();
        return;
    }
    let names = entries
        .iter()
        .map(|entry| native_c_string(&entry.name))
        .collect::<Vec<_>>()
        .join(", ");
    let lengths = entries
        .iter()
        .map(|entry| format!("(size_t){}", entry.name.len()))
        .collect::<Vec<_>>()
        .join(", ");
    let values = entries
        .iter()
        .map(|entry| match entry.value {
            NativeNamespaceValue::Binding {
                provider_index,
                export_index,
            } if shared => format!("export_arrays[{provider_index}][{export_index}]"),
            NativeNamespaceValue::Binding {
                provider_index,
                export_index,
            } => format!("kiln_exports_{provider_index}[{export_index}]"),
            NativeNamespaceValue::Namespace { provider_index } => {
                format!("{namespaces}[{provider_index}]")
            }
        })
        .collect::<Vec<_>>()
        .join(", ");
    writeln!(
        output,
        "    status = {module_symbol}_namespace_wrapper(&{namespaces}[{module_index}], (const char *const[]){{ {names} }}, (const size_t[]){{ {lengths} }}, (const kiln_value[]){{ {values} }}, (size_t){});",
        entries.len()
    )
    .unwrap();
}

/// The graph launcher's root function (ADR 0041): the export, import and namespace arrays it
/// links with, and the two result slots `main` reads after the realm is destroyed.
pub(super) fn render_graph_roots(
    output: &mut String,
    modules: &[(&kiln_module_graph::ModuleNode, kiln_backend_c::CModule)],
) {
    output.push_str(
        "static void kiln_graph_gc_roots(void *heap, void *context, int phase) {\n    kiln_graph_context *graph = (kiln_graph_context *)context;\n    size_t index;\n    if (phase == KILN_GC_PHASE_RESET || graph == NULL) return;\n",
    );
    for (index, (node, _)) in modules.iter().enumerate() {
        let export_count = native_graph_export_count(node);
        if export_count != 0 {
            writeln!(
                output,
                "    for (index = 0; index < (size_t){export_count}; ++index) (void)kiln_realm_gc_mark_value(heap, &kiln_exports_{index}[index]);"
            )
            .unwrap();
        }
        if !node.program.imports.is_empty() {
            writeln!(
                output,
                "    for (index = 0; index < (size_t){}; ++index) (void)kiln_realm_gc_mark_value(heap, &kiln_imports_{index}[index]);",
                node.program.imports.len()
            )
            .unwrap();
        }
    }
    writeln!(
        output,
        "    for (index = 0; index < (size_t){}; ++index) (void)kiln_realm_gc_mark_value(heap, &kiln_namespaces[index]);",
        modules.len()
    )
    .unwrap();
    if modules
        .iter()
        .any(|(node, _)| node.program.program.has_dynamic_import())
    {
        output.push_str("    (void)kiln_realm_gc_mark_value(heap, &kiln_opentui_namespace);\n");
        output.push_str("    (void)kiln_realm_gc_mark_value(heap, &kiln_node_ffi_namespace);\n");
    }
    writeln!(
        output,
        "    {{\n        (void)kiln_realm_gc_mark_value(heap, &graph->result);\n        (void)kiln_realm_gc_mark_value(heap, &graph->entry_result);\n        for (index = 0; index < (size_t){}; ++index) {{\n            (void)kiln_realm_gc_mark_value(heap, &graph->module_states[index].result);\n            (void)kiln_realm_gc_mark_value(heap, &graph->module_states[index].active_promise);\n            (void)kiln_realm_gc_mark_value(heap, &graph->module_states[index].completion_promise);\n            (void)kiln_realm_gc_mark_value(heap, &graph->module_states[index].completion_resolve);\n            (void)kiln_realm_gc_mark_value(heap, &graph->module_states[index].completion_reject);\n        }}\n    }}\n}}\n",
        modules.len()
    )
    .unwrap();
}

#[cfg(test)]
pub(super) fn graph_launcher_source(
    graph: &ModuleGraph,
    modules: &[(&kiln_module_graph::ModuleNode, kiln_backend_c::CModule)],
    policy: &NativeRealmPolicy,
) -> Result<String, NativeError> {
    graph_launcher_source_with_imports(graph, modules, policy, None, None, None, None, None, None)
}

/// What a startup module's completion handler does with a rejection nobody awaits.
pub(super) const FATAL_UNHANDLED_REJECTION: &str = r#"/* A startup module has no importer waiting on it, so its rejection is unhandled and, as in node, fatal. This runs as a `then` reaction, so a thrown status would only reject the derived promise; and the launcher reads the entry promise only once the turn loop is quiescent, which a listening server never lets it be -- a rejected top-level await beside a live server hung the process here. */ { kiln_cli_report_failure_in(KILN_C99_THROWN, kiln_cli_describe_error(*value), "module body"); exit(1); }"#;

pub(super) fn graph_launcher_source_with_imports(
    graph: &ModuleGraph,
    modules: &[(&kiln_module_graph::ModuleNode, kiln_backend_c::CModule)],
    policy: &NativeRealmPolicy,
    live_imports: Option<&BTreeSet<(PathBuf, usize)>>,
    inactive_commonjs_dependencies: Option<&BTreeSet<(PathBuf, String)>>,
    partial_namespace_members: Option<&BTreeMap<PathBuf, BTreeSet<String>>>,
    native_failures: Option<&BTreeMap<(PathBuf, String), String>>,
    native_runtime_failures: Option<&BTreeMap<(PathBuf, PathBuf, String), String>>,
    bootstrap: Option<&kiln_backend_c::CModule>,
) -> Result<String, NativeError> {
    if modules.is_empty() {
        return Err(NativeError::Graph(
            ModuleGraphError::InternalMissingModule {
                path: graph.entry().to_path_buf(),
            },
        ));
    }
    // Diagnostic names remain project-relative, while CommonJS path roles are constructed as
    // distinct types below so observable runtime paths cannot be replaced with diagnostics by a
    // broad path-normalization refactor.
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
        if let Some(failures) = native_runtime_failures {
            for ((owner, importer, _), _) in failures {
                if owner == &node.path {
                    let next = runtime_importers.len();
                    runtime_importers.entry(importer.clone()).or_insert(next);
                }
            }
        }
    }
    let mut indices = std::collections::HashMap::with_capacity(modules.len());
    for (index, (node, _)) in modules.iter().enumerate() {
        indices.insert(node.path.clone(), index);
    }
    let entry_index = modules
        .iter()
        .position(|(node, _)| node.path == graph.entry())
        .expect("entry module is present in the graph");
    // The entry is the sole startup root. Its static dependency walk recursively ensures every
    // reachable provider, while the realm turn loop drives any pending top-level-await
    // continuations. Starting every reachable module independently can evaluate a cycle from the
    // dependency side before the authoritative entry has established its state.
    let startup_indices = vec![entry_index];
    let namespace_plans = graph_namespace_plans(
        graph,
        modules,
        &indices,
        live_imports,
        partial_namespace_members,
    )?;
    let mut output = String::with_capacity(2048 + modules.len() * 512);
    let mut evaluation = String::with_capacity(modules.len() * 512);
    output.push_str(
        r#"/* Kiln native graph launcher. */
/* The worker supervisor in `main` needs `sigaction`, `kill`, `poll`, `fcntl` and
 * `clock_gettime`, which glibc hides under `-std=c11` unless POSIX is asked for before the first
 * include (the prelude does the same). Darwin declares them without it. */
#if !defined(_WIN32) && !defined(__APPLE__) && !defined(_POSIX_C_SOURCE)
#define _POSIX_C_SOURCE 200809L
#endif
#include <locale.h>
#include <stddef.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#ifndef _WIN32
#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <signal.h>
#include <stdio.h>
#include <time.h>
#include <sys/wait.h>
#include <unistd.h>
#if defined(__linux__)
#include <sys/prctl.h>
#else
#include <pthread.h>
#endif
#endif

enum kiln_value_tag {
    KILN_VALUE_UNDEFINED = 0,
    KILN_VALUE_NULL = 1,
    KILN_VALUE_BOOLEAN = 2,
    KILN_VALUE_NUMBER = 3,
    KILN_VALUE_STRING = 4
};

"#,
    );
    output.push_str(kiln_native_runtime::kiln_value_abi_declaration!());
    output.push_str(kiln_native_runtime::kiln_string_abi_declaration!());
    output.push_str(kiln_native_runtime::kiln_cli_host_state_declaration!());
    output.push_str(r#"/* The prelude owns this realm-local guard. The trusted builtin bootstrap may install methods;
 * the launcher seals those writes before any user module can observe or mutate the realm. */
extern uint64_t kiln_c99_builtin_store_epoch __attribute__((visibility("hidden")));

static kiln_value kiln_graph_diagnostic(const char *phase, const char *module) {
    kiln_value value = { .tag = KILN_VALUE_STRING, .flags = 0, .len = 0, .bits = 0 };
    /* Scratch per thread: the value is reported by this thread before it runs another realm. */
    static _Thread_local char text[2048];
    static _Thread_local kiln_string string = {0, KILN_STRING_LATIN1, NULL};
    size_t phase_len = strlen(phase);
    size_t module_len = strlen(module);
    const char prefix[] = "phase=";
    const char middle[] = " module=";
    size_t total = sizeof(prefix) - 1 + phase_len + sizeof(middle) - 1 + module_len;
    if (total >= sizeof(text)) total = sizeof(text) - 1;
    value.len = 0;
    memcpy(text + value.len, prefix, sizeof(prefix) - 1); value.len += sizeof(prefix) - 1;
    if (value.len + phase_len > total) phase_len = total - value.len;
    memcpy(text + value.len, phase, phase_len); value.len += phase_len;
    if (value.len < total) {
        size_t take = sizeof(middle) - 1;
        if (take > total - value.len) take = total - value.len;
        memcpy(text + value.len, middle, take); value.len += take;
    }
    if (value.len < total) {
        size_t take = module_len;
        if (take > total - value.len) take = total - value.len;
        memcpy(text + value.len, module, take); value.len += take;
    }
    text[value.len] = '\0';
    string.unit_len = (uint32_t)value.len;
    string.data = text;
    value.ptr = (const char *)&string;
    return value;
}

static inline int kiln_c99_string_equals_ascii(kiln_value value, const char *ascii, size_t length) {
    if (value.tag != KILN_VALUE_STRING || value.ptr == NULL) return 0;
    return kiln_string_equals_ascii((const kiln_string *)value.ptr, ascii, length);
}

static inline int kiln_c99_string_starts_ascii(kiln_value value, const char *ascii, size_t length) {
    if (value.tag != KILN_VALUE_STRING || value.ptr == NULL) return 0;
    return kiln_string_starts_ascii((const kiln_string *)value.ptr, ascii, length);
}

typedef int (*kiln_host_call_fn)(void *context, uint16_t operation, uint16_t argc,
                                 const kiln_value *arguments, kiln_value *result);
typedef struct kiln_host {
    kiln_host_call_fn call;
    void *context;
} kiln_host;

static int kiln_c99_filename_matches(kiln_value value, const char *path, size_t path_len) {
    if (path == NULL || value.tag != KILN_VALUE_STRING || (value.len != 0 && value.ptr == NULL)) return 0;
    const kiln_string *s = (const kiln_string *)value.ptr;
    if (s == NULL) return 0;
    if (s->unit_len == path_len && (path_len == 0 || kiln_string_equals_ascii(s, path, path_len))) return 1;
    if (s->unit_len >= 7 && kiln_string_starts_ascii(s, "file://", 7) && s->unit_len - 7 == path_len) {
        if (path_len == 0) return 1;
        for (size_t i = 0; i < path_len; ++i) {
            if (kiln_string_unit_at(s, i + 7) != (uint8_t)path[i]) return 0;
        }
        return 1;
    }
    return 0;
}

typedef int (*kiln_graph_module_fn)(kiln_host *host, kiln_value *result,
                                    const kiln_value *imports, size_t import_count,
                                    kiln_value *exports, size_t export_count);
typedef int (*kiln_commonjs_require_fn)(void *context, kiln_host *host, kiln_value specifier,
                                       kiln_value *result, kiln_value *thrown);
typedef int (*kiln_realm_entry_fn)(void *context);

extern int kiln_cli_host_call(void *context, uint16_t operation, uint16_t argc,
                              const kiln_value *arguments, kiln_value *result);
extern int kiln_cli_write_result(kiln_value value);
extern void kiln_cli_report_failure_in(int status, kiln_value value, const char *phase);
extern void kiln_cli_report_failure_with(int status, kiln_value value, const char *phase, const kiln_cli_host_state *host_state);
extern void kiln_cli_warn(const char *message);
extern kiln_value kiln_cli_describe_error(kiln_value value);
extern int kiln_c99_get(kiln_host *host, kiln_value target, const char *key, size_t key_len,
                        kiln_value *out, int depth, kiln_value *thrown);
extern int kiln_realm_create(uint64_t *out_realm);
extern int kiln_realm_enter(uint64_t realm, kiln_realm_entry_fn entry, void *context);
extern int kiln_realm_set_argv(uint64_t realm, uint64_t argc, const char *const *argv);
extern int kiln_realm_use_wall_clock(uint64_t realm);
extern int kiln_realm_drive(uint64_t realm, void *host, void *thrown);
extern int kiln_realm_destroy(uint64_t realm);
typedef void (*kiln_gc_roots_fn)(void *heap, void *context, int phase);
extern int kiln_realm_gc_register_roots(kiln_gc_roots_fn roots, void *context, uint32_t flags);
extern int kiln_realm_gc_unregister_roots(kiln_gc_roots_fn roots, void *context);
extern int kiln_realm_gc_mark_value(void *heap, const void *value);
#define KILN_GC_ROOT_SURVIVES_TEARDOWN 1u
#define KILN_GC_PHASE_RESET 2
#define KILN_C99_THROWN (-7)
#define KILN_C99_CONTROL_ERROR (-2)
#define KILN_C99_HOST_ERROR (-3)
extern int kiln_c99_process_object(kiln_value *result);
#define KILN_C99_INVALID_ARGUMENT (-1)
#define KILN_C99_HOST_MODULE_CREATE_REQUIRE UINT16_C(0x7FF0)
#define KILN_C99_HINT_STRING 1
extern int kiln_c99_commonjs_require_value(kiln_value *out, kiln_commonjs_require_fn call, void *context);
extern int kiln_c99_default_require(void *context, kiln_host *host, kiln_value specifier, kiln_value *result, kiln_value *thrown);
extern int kiln_c99_new_object(kiln_value *result);
extern int kiln_c99_put(kiln_host *host, kiln_value target, const char *key, size_t key_len,
                        kiln_value value, int depth, kiln_value *thrown);
extern kiln_value kiln_c99_copy_text(const char *text, size_t len);
extern kiln_value kiln_c99_undefined(void);
extern int kiln_c99_to_primitive(kiln_host *host, kiln_value value, int hint, kiln_value *out, int depth, kiln_value *thrown);
extern kiln_value kiln_c99_text(kiln_value value);
extern kiln_value kiln_c99_control_diagnostic(void);
extern int kiln_c99_diagnostic_available(void);
extern kiln_value kiln_c99_concat(kiln_value left, kiln_value right);
extern int kiln_c99_throw_error(kiln_value *out, uint64_t kind, const char *message);
extern int kiln_c99_new_type_error(kiln_value *out, const char *message);
extern int kiln_c99_invoke_method(kiln_host *host, kiln_value *result, uint64_t code,
                                  kiln_value receiver, const kiln_value *args, size_t argc,
                                  int depth, kiln_value *thrown);
extern int kiln_c99_call(kiln_host *host, kiln_value *result, kiln_value callee,
                         kiln_value receiver, const kiln_value *args, size_t argc,
                         int depth, kiln_value *thrown);
extern int kiln_c99_module_completion(kiln_host *, kiln_value, uint32_t, kiln_value *, int, kiln_value *);
extern int kiln_c99_finish_module_checkpoint(kiln_host *, kiln_value *);
#define KILN_C99_OK 0
#define KILN_C99_METHOD_PROMISE_RESOLVE 110
#define KILN_C99_METHOD_PROMISE_REJECT 111
#define KILN_C99_METHOD_PROMISE_WITH_RESOLVERS 411
"#,
    );
    for (node, module) in modules {
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
        if node
            .program
            .program
            .code
            .iter()
            .any(|i| matches!(i, kiln_ir::Instruction::Await | kiln_ir::Instruction::Yield))
        {
            writeln!(
                output,
                "__attribute__((weak)) void {}_publish_exports(kiln_value *exports) {{ (void)exports; }}",
                module.symbol
            )
            .unwrap();
        }
    }
    if let Some(bootstrap) = bootstrap {
        writeln!(
            output,
            "extern int {}(kiln_host *host, kiln_value *result, const kiln_value *imports, size_t import_count, kiln_value *exports, size_t export_count);",
            bootstrap.symbol
        )
        .unwrap();
    }
    for (node, module) in modules {
        if node.source == ModuleSource::CommonJs {
            writeln!(
                output,
                "extern int {}_commonjs_wrapper(kiln_value *imports, size_t import_count, const char *filename, size_t filename_len, const char *dirname, size_t dirname_len, kiln_commonjs_require_fn require_call, void *require_context);",
                module.symbol
            )
            .unwrap();
        }
    }
    // The graph's linkage -- every module's export and import arrays, the namespaces, the
    // CommonJS contexts -- is the realm's (ADR 0070, realms issue 04 step 5): fields of the
    // `kiln_graph_context` that `kiln_cli_run_realm` allocates for its one realm, spelled by the
    // names the rest of this launcher always used, through macros that read the `graph` in scope.
    let mut linkage_fields = String::new();
    let mut linkage_names = String::new();
    for (index, (node, _)) in modules.iter().enumerate() {
        let export_count = native_graph_export_count(node);
        if export_count != 0 {
            writeln!(linkage_fields, "    kiln_value exports_{index}[{export_count}];").unwrap();
            writeln!(
                linkage_names,
                "#define kiln_exports_{index} (graph->exports_{index})"
            )
            .unwrap();
        }
        if !node.program.imports.is_empty() {
            writeln!(
                linkage_fields,
                "    kiln_value imports_{index}[{}];",
                node.program.imports.len()
            )
            .unwrap();
            writeln!(
                linkage_names,
                "#define kiln_imports_{index} (graph->imports_{index})"
            )
            .unwrap();
        }
    }
    writeln!(linkage_fields, "    kiln_value namespaces[{}];", modules.len()).unwrap();
    linkage_names.push_str("#define kiln_namespaces (graph->namespaces)\n");
    if modules
        .iter()
        .any(|(node, _)| node.program.program.has_dynamic_import())
    {
        linkage_fields.push_str("    kiln_value opentui_namespace;\n    kiln_value node_ffi_namespace;\n");
        linkage_names.push_str("#define kiln_opentui_namespace (graph->opentui_namespace)\n#define kiln_node_ffi_namespace (graph->node_ffi_namespace)\n");
    }
    for (index, (node, _)) in modules.iter().enumerate() {
        if node.source == ModuleSource::CommonJs {
            writeln!(linkage_fields, "    kiln_commonjs_context commonjs_{index};").unwrap();
            writeln!(
                linkage_names,
                "#define kiln_commonjs_{index} (graph->commonjs_{index})"
            )
            .unwrap();
        }
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
"#,
    );
    for (index, (node, _)) in modules.iter().enumerate() {
        if node.source == ModuleSource::CommonJs {
            writeln!(
                output,
                "static int kiln_commonjs_require_{index}(void *opaque, kiln_host *host, kiln_value specifier, kiln_value *result, kiln_value *thrown);"
            )
            .unwrap();
        }
    }
    output.push_str(
        r#"
typedef struct kiln_graph_entry_context {
    kiln_graph_module_fn module;
    kiln_host *host;
    kiln_value *result;
    const kiln_value *imports;
    size_t import_count;
    kiln_value *exports;
    size_t export_count;
} kiln_graph_entry_context;

typedef struct kiln_graph_module_state {
    kiln_value result;
    kiln_value active_promise;
    kiln_value completion_promise;
    kiln_value completion_resolve;
    kiln_value completion_reject;
    size_t dependency_cursor;
    int module_settling;
    int body_started;
    int phase;
    int failure_status;
} kiln_graph_module_state;

enum { KILN_MODULE_UNINITIALIZED = 0, KILN_MODULE_EVALUATING = 1,
       KILN_MODULE_EVALUATED = 2, KILN_MODULE_FAILED = 3 };

/* KILN_GRAPH_CONTEXT */
static int kiln_graph_load_commonjs(kiln_commonjs_context *context, kiln_value *result,
                                    kiln_value *thrown) {
    int status;
    if (context == NULL || result == NULL || context->exports == NULL) return -1;
    if (context->state != 0) {
        *result = context->exports[0];
        return 0;
    }
    context->state = 1;
    status = context->module(context->host, result, context->imports, context->import_count,
                             context->exports, (size_t)1);
    if (status != 0) {
        context->state = 0;
        if (thrown != NULL) *thrown = *result;
        return status;
    }
    context->state = 2;
    *result = context->exports[0];
    return 0;
}

"#,
    );
    let context = format!(
        "typedef struct kiln_graph_context kiln_graph_context;\n\
         typedef struct kiln_graph_require_context {{\n    kiln_graph_context *graph;\n    size_t base_index;\n}} kiln_graph_require_context;\n\
         /* One realm's graph, allocated by `kiln_cli_run_realm` for its realm and freed after it. */\n\
         struct kiln_graph_context {{\n    kiln_host host;\n    /* The CLI host's state for this realm: its last refusal. */\n    kiln_cli_host_state cli;\n    kiln_value result;\n    kiln_value entry_result;\n    kiln_graph_module_state module_states[{}];\n    size_t current_module;\n    int roots_registered;\n    /* The entry's top-level await never settled with the loop out of work (node: warn, exit 13). */\n    int unsettled_entry;\n    struct kiln_graph_require_context require_contexts[{}];\n{linkage_fields}}};\n{linkage_names}",
        modules.len(),
        runtime_importers.len().max(1),
    );
    output = output.replace("/* KILN_GRAPH_CONTEXT */\n", &context);
    for (index, (node, _)) in modules.iter().enumerate() {
        writeln!(
            output,
            "\nstatic int kiln_commonjs_require_{index}(void *opaque, kiln_host *host, kiln_value specifier, kiln_value *result, kiln_value *thrown) {{\n    kiln_graph_context *graph = (kiln_graph_context *)opaque;\n    (void)graph;\n    (void)host;\n    if (result == NULL || specifier.tag != KILN_VALUE_STRING || (specifier.len != 0 && specifier.ptr == NULL)) return -1;"
        )
        .unwrap();
        for dependency in &node.dependencies {
            if inactive_commonjs_dependencies.is_some_and(|inactive| {
                inactive.contains(&(node.path.clone(), dependency.specifier.clone()))
            }) {
                continue;
            }
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
                    "    if (kiln_c99_string_equals_ascii(specifier, {specifier}, (size_t){specifier_len})) return kiln_graph_load_commonjs(&kiln_commonjs_{provider_index}, result, thrown);"
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
                    "    if (kiln_c99_string_equals_ascii(specifier, {specifier}, (size_t){specifier_len})) {{ *result = kiln_exports_{provider_index}[{commonjs_export}]; return 0; }}"
                )
                .unwrap();
            }
        }
        output.push_str("    if (thrown != NULL) *thrown = specifier;\n    return -2;\n}\n");
    }
    if modules
        .iter()
        .any(|(node, _)| node.program.program.has_dynamic_import())
    {
        write_launcher_dynamic_import(&mut output, graph, modules, &logical_paths, native_failures);
    }
    output.push_str(
        &r#"
typedef struct kiln_launcher_promise_view {
    int state;
    kiln_value value;
} kiln_launcher_promise_view;


static kiln_value *kiln_graph_module_result(kiln_graph_context *graph, size_t index) {
    return &graph->module_states[index].result;
}

static kiln_value *kiln_graph_module_promise(kiln_graph_context *graph, size_t index) {
    static kiln_value undefined = { .tag = KILN_VALUE_UNDEFINED, .flags = 0, .len = 0, .bits = 0 };
    return graph->module_states[index].completion_promise.tag == 16 /* KILN_VALUE_PROMISE */
        ? &graph->module_states[index].completion_promise
        : &undefined;
}

static int kiln_graph_prepare_completion(kiln_graph_context *graph, size_t index) {
    kiln_graph_module_state *state;
    kiln_value deferred;
    int status;
    if (graph == NULL || index >= (size_t)MODULE_COUNT) return KILN_C99_CONTROL_ERROR;
    state = &graph->module_states[index];
    if (state->completion_promise.tag == 16 /* KILN_VALUE_PROMISE */) return KILN_C99_OK;
    status = kiln_c99_invoke_method(&graph->host,
                                    &deferred,
                                    KILN_C99_METHOD_PROMISE_WITH_RESOLVERS,
                                    kiln_c99_undefined(), NULL, 0, 0, NULL);
    if (status != KILN_C99_OK) return status;
    status = kiln_c99_get(&graph->host, deferred, "promise", 7,
                          &state->completion_promise, 0, NULL);
    if (status == KILN_C99_OK)
        status = kiln_c99_get(&graph->host, deferred, "resolve", 7,
                              &state->completion_resolve, 0, NULL);
    if (status == KILN_C99_OK)
        status = kiln_c99_get(&graph->host, deferred, "reject", 6,
                              &state->completion_reject, 0, NULL);
    return status;
}
"#
        .replace("MODULE_COUNT", &modules.len().to_string())
        .replace(
            "{runtime_context_count}",
            &runtime_importers.len().max(1).to_string(),
        ),
    );
    output.push_str(
        "static int kiln_graph_host_call(void *context, uint16_t operation, uint16_t argc, const kiln_value *arguments, kiln_value *result);\nstatic int kiln_graph_ensure_module(kiln_graph_context *graph, size_t index);\n",
    );
    output.push_str("static int kiln_graph_runtime_require(void *opaque, kiln_host *host, kiln_value specifier, kiln_value *result, kiln_value *thrown) {\n    kiln_graph_require_context *context = (kiln_graph_require_context *)opaque;\n    kiln_graph_context *graph;\n    if (context == NULL || (graph = context->graph) == NULL || result == NULL || specifier.tag != KILN_VALUE_STRING || (specifier.len != 0 && specifier.ptr == NULL)) return KILN_C99_INVALID_ARGUMENT;\n    (void)host;\n    switch (context->base_index) {\n");
    for (base_index, importer) in runtime_importers.keys().enumerate() {
        output.push_str(&format!("        case {base_index}:\n"));
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
                    writeln!(output, "            if (kiln_c99_string_equals_ascii(specifier, {specifier}, (size_t){specifier_len})) return kiln_graph_load_commonjs(&kiln_commonjs_{provider_index}, result, thrown);").unwrap();
                } else if let Some(projection) =
                    native_commonjs_projection_index(modules[provider_index].0)
                {
                    writeln!(output, "            if (kiln_c99_string_equals_ascii(specifier, {specifier}, (size_t){specifier_len})) {{ int status = kiln_graph_ensure_module(graph, (size_t){provider_index}); if (status != KILN_C99_OK) {{ if (thrown != NULL) *thrown = graph->module_states[{provider_index}].result; return status; }} *result = kiln_exports_{provider_index}[{projection}]; return KILN_C99_OK; }}").unwrap();
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
                writeln!(
                    output,
                    "            if (kiln_c99_string_equals_ascii(specifier, {}, (size_t){})) {{ kiln_value error = kiln_c99_undefined(); (void)kiln_c99_new_type_error(&error, {}); if (thrown != NULL) *thrown = error; return KILN_C99_THROWN; }}",
                    native_c_string(failure_specifier),
                    failure_specifier.len(),
                    native_c_string(message),
                )
                .unwrap();
            }
            if let Some(failures) = native_runtime_failures {
                for ((owner, failure_importer, failure_specifier), message) in failures {
                    if owner != &node.path || failure_importer != importer {
                        continue;
                    }
                    writeln!(
                        output,
                        "            if (kiln_c99_string_equals_ascii(specifier, {}, (size_t){})) {{ kiln_value error = kiln_c99_undefined(); (void)kiln_c99_new_type_error(&error, {}); if (thrown != NULL) *thrown = error; return KILN_C99_THROWN; }}",
                        native_c_string(failure_specifier),
                        failure_specifier.len(),
                        native_c_string(message),
                    )
                    .unwrap();
                }
            }
        }
        output.push_str("            break;\n");
    }
    output.push_str("        default: break;\n    }\n    return kiln_c99_default_require(NULL, host, specifier, result, thrown);\n}\n\n");
    output.push_str(
        "static int kiln_graph_require_builtin(void *opaque, kiln_host *host, kiln_value specifier, kiln_value *result, kiln_value *thrown) {\n    kiln_graph_context *graph = (kiln_graph_context *)opaque;\n    (void)host;\n    if (graph == NULL || result == NULL || specifier.tag != KILN_VALUE_STRING || (specifier.len != 0 && specifier.ptr == NULL)) return KILN_C99_INVALID_ARGUMENT;\n",
    );
    for (index, (node, _)) in modules.iter().enumerate() {
        let ModuleSource::NodeBuiltin(builtin) = node.source else {
            continue;
        };
        let Some(projection) = native_commonjs_projection_index(node) else {
            continue;
        };
        let specifier = builtin.specifier();
        writeln!(
            output,
            "    if (kiln_c99_string_equals_ascii(specifier, {}, (size_t){})) {{ int status = kiln_graph_ensure_module(graph, (size_t){}); if (status != KILN_C99_OK) {{ if (thrown != NULL) *thrown = graph->module_states[{}].result; return status; }} *result = kiln_exports_{}[{}]; return KILN_C99_OK; }}",
            native_c_string(specifier), specifier.len(), index, index, index, projection
        )
        .unwrap();
    }
    output.push_str("    return kiln_c99_default_require(NULL, host, specifier, result, thrown);\n}\n\nstatic int kiln_graph_create_require(kiln_graph_context *graph, uint16_t argc, const kiln_value *arguments, kiln_value *result) {\n    if (graph == NULL || result == NULL || argc != 1 || arguments == NULL || arguments[0].tag != KILN_VALUE_STRING || (arguments[0].len != 0 && arguments[0].ptr == NULL)) return KILN_C99_INVALID_ARGUMENT;\n");
    for (base_index, importer) in runtime_importers.keys().enumerate() {
        let importer = importer.to_string_lossy();
        let file_url = Url::from_file_path(importer.as_ref())
            .ok()
            .map(|url| url.to_string());
        writeln!(
            output,
            "    if (kiln_c99_filename_matches(arguments[0], {}, (size_t){}){} ) {{ graph->require_contexts[{base_index}].graph = graph; graph->require_contexts[{base_index}].base_index = (size_t){base_index}; return kiln_c99_commonjs_require_value(result, kiln_graph_runtime_require, &graph->require_contexts[{base_index}]); }}",
            native_c_string(importer.as_ref()),
            importer.len(),
            file_url.as_ref().map(|url| format!(" || kiln_c99_filename_matches(arguments[0], {}, (size_t){})", native_c_string(url), url.len())).unwrap_or_default(),
        ).unwrap();
    }
    output.push_str(
        "    return kiln_c99_commonjs_require_value(result, kiln_graph_require_builtin, graph);\n}\n\nstatic int kiln_graph_host_call(void *context, uint16_t operation, uint16_t argc, const kiln_value *arguments, kiln_value *result) {\n    kiln_graph_context *graph = (kiln_graph_context *)context;\n    if (operation == KILN_C99_HOST_MODULE_CREATE_REQUIRE) return kiln_graph_create_require(graph, argc, arguments, result);\n    return kiln_cli_host_call(graph != NULL ? &graph->cli : NULL, operation, argc, arguments, result);\n}\n",
    );
    output.push_str("static const size_t kiln_graph_startup_indices[] = {");
    for (position, index) in startup_indices.iter().enumerate() {
        if position != 0 {
            output.push_str(", ");
        }
        output.push_str(&index.to_string());
    }
    output.push_str("};\n");
    render_graph_roots(&mut output, modules);
    for index in 0..modules.len() {
        writeln!(
            output,
            "static int kiln_graph_eval_module_{index}(kiln_graph_context *graph);"
        )
        .unwrap();
    }
    output.push_str(
        "static int kiln_graph_ensure_module(kiln_graph_context *graph, size_t index);\n",
    );
    output.push_str(
        r#"
static int kiln_install_native_modules(kiln_host *host) {
    kiln_value proc_obj = kiln_c99_undefined();
    kiln_value modules_obj = kiln_c99_undefined();
    int status = kiln_c99_process_object(&proc_obj);
    if (status != KILN_C99_OK) return status;
    status = kiln_c99_new_object(&modules_obj);
    if (status != KILN_C99_OK) return status;

    kiln_value net_obj = kiln_c99_undefined();
    status = kiln_c99_new_object(&net_obj);
    if (status != KILN_C99_OK) return status;
    kiln_value net_connect = (kiln_value){ .tag = 11, .flags = 0, .len = 0, .bits = 338 /* KILN_C99_METHOD_NODE_NET_CONNECT */ };
    kiln_value net_create_server = (kiln_value){ .tag = 11, .flags = 0, .len = 0, .bits = 268 /* KILN_C99_METHOD_NODE_NET_CREATE_SERVER */ };
    status = kiln_c99_put(host, net_obj, "connect", 7, net_connect, 0, NULL);
    if (status != KILN_C99_OK) return status;
    status = kiln_c99_put(host, net_obj, "createConnection", 16, net_connect, 0, NULL);
    if (status != KILN_C99_OK) return status;
    status = kiln_c99_put(host, net_obj, "createServer", 12, net_create_server, 0, NULL);
    if (status != KILN_C99_OK) return status;
    status = kiln_c99_put(host, modules_obj, "net", 3, net_obj, 0, NULL);
    if (status != KILN_C99_OK) return status;

    kiln_value http_obj = kiln_c99_undefined();
    status = kiln_c99_new_object(&http_obj);
    if (status != KILN_C99_OK) return status;
    kiln_value http_server = (kiln_value){ .tag = 11, .flags = 0, .len = 0, .bits = 403 /* KILN_C99_METHOD_NODE_HTTP_SERVER */ };
    kiln_value http_server_response = (kiln_value){ .tag = 11, .flags = 0, .len = 0, .bits = 426 /* KILN_C99_METHOD_NODE_HTTP_SERVER_RESPONSE */ };
    kiln_value http_create_server = (kiln_value){ .tag = 11, .flags = 0, .len = 0, .bits = 275 /* KILN_C99_METHOD_NODE_HTTP_CREATE_SERVER */ };
    kiln_value http_get = (kiln_value){ .tag = 11, .flags = 0, .len = 0, .bits = 280 /* KILN_C99_METHOD_NODE_HTTP_GET */ };
    kiln_value http_request = (kiln_value){ .tag = 11, .flags = 0, .len = 0, .bits = 281 /* KILN_C99_METHOD_NODE_HTTP_REQUEST */ };
    status = kiln_c99_put(host, http_obj, "Server", 6, http_server, 0, NULL);
    if (status != KILN_C99_OK) return status;
    status = kiln_c99_put(host, http_obj, "ServerResponse", 14, http_server_response, 0, NULL);
    if (status != KILN_C99_OK) return status;
    status = kiln_c99_put(host, http_obj, "createServer", 12, http_create_server, 0, NULL);
    if (status != KILN_C99_OK) return status;
    status = kiln_c99_put(host, http_obj, "get", 3, http_get, 0, NULL);
    if (status != KILN_C99_OK) return status;
    status = kiln_c99_put(host, http_obj, "request", 7, http_request, 0, NULL);
    if (status != KILN_C99_OK) return status;
    /* The private host half of `serve()` (kiln-builtins web.ts). */
    kiln_value http_create_fetch_server = (kiln_value){ .tag = 11, .flags = 0, .len = 0, .bits = 530 /* KILN_C99_METHOD_NODE_HTTP_CREATE_FETCH_SERVER */ };
    kiln_value http_fetch_respond = (kiln_value){ .tag = 11, .flags = 0, .len = 0, .bits = 531 /* KILN_C99_METHOD_NODE_HTTP_FETCH_RESPOND */ };
    status = kiln_c99_put(host, http_obj, "createFetchServer", 17, http_create_fetch_server, 0, NULL);
    if (status != KILN_C99_OK) return status;
    status = kiln_c99_put(host, http_obj, "fetchRespond", 12, http_fetch_respond, 0, NULL);
    if (status != KILN_C99_OK) return status;
    status = kiln_c99_put(host, modules_obj, "http", 4, http_obj, 0, NULL);
    if (status != KILN_C99_OK) return status;

    kiln_value fsp_obj = kiln_c99_undefined();
    status = kiln_c99_new_object(&fsp_obj);
    if (status != KILN_C99_OK) return status;
    kiln_value fs_append_file = (kiln_value){ .tag = 11, .flags = 0, .len = 0, .bits = 482 /* KILN_C99_METHOD_NODE_BUILTIN_UNMATERIALIZED */ };
    kiln_value fs_mkdir = (kiln_value){ .tag = 11, .flags = 0, .len = 0, .bits = 507 /* KILN_C99_METHOD_NODE_FS_MKDIR */ };
    kiln_value fs_read_file = (kiln_value){ .tag = 11, .flags = 0, .len = 0, .bits = 379 /* KILN_C99_METHOD_NODE_FS_READ_FILE */ };
    kiln_value fs_stat = (kiln_value){ .tag = 11, .flags = 0, .len = 0, .bits = 380 /* KILN_C99_METHOD_NODE_FS_STAT */ };
    kiln_value fs_write_file = (kiln_value){ .tag = 11, .flags = 0, .len = 0, .bits = 381 /* KILN_C99_METHOD_NODE_FS_WRITE_FILE */ };
    status = kiln_c99_put(host, fsp_obj, "appendFile", 10, fs_append_file, 0, NULL);
    if (status != KILN_C99_OK) return status;
    status = kiln_c99_put(host, fsp_obj, "mkdir", 5, fs_mkdir, 0, NULL);
    if (status != KILN_C99_OK) return status;
    status = kiln_c99_put(host, fsp_obj, "readFile", 8, fs_read_file, 0, NULL);
    if (status != KILN_C99_OK) return status;
    status = kiln_c99_put(host, fsp_obj, "stat", 4, fs_stat, 0, NULL);
    if (status != KILN_C99_OK) return status;
    status = kiln_c99_put(host, fsp_obj, "writeFile", 9, fs_write_file, 0, NULL);
    if (status != KILN_C99_OK) return status;
    status = kiln_c99_put(host, modules_obj, "fs_promises", 11, fsp_obj, 0, NULL);
    if (status != KILN_C99_OK) return status;

    kiln_value fs_obj = kiln_c99_undefined();
    status = kiln_c99_new_object(&fs_obj);
    if (status != KILN_C99_OK) return status;
    kiln_value fs_exists_sync = (kiln_value){ .tag = 11, .flags = 0, .len = 0, .bits = 378 /* KILN_C99_METHOD_NODE_FS_EXISTS_SYNC */ };
    kiln_value fs_read_file_sync = (kiln_value){ .tag = 11, .flags = 0, .len = 0, .bits = 514 /* KILN_C99_METHOD_NODE_FS_READ_FILE_SYNC */ };
    kiln_value fs_write_file_sync = (kiln_value){ .tag = 11, .flags = 0, .len = 0, .bits = 515 /* KILN_C99_METHOD_NODE_FS_WRITE_FILE_SYNC */ };
    kiln_value fs_mkdir_sync = (kiln_value){ .tag = 11, .flags = 0, .len = 0, .bits = 516 /* KILN_C99_METHOD_NODE_FS_MKDIR_SYNC */ };
    kiln_value fs_stat_sync = (kiln_value){ .tag = 11, .flags = 0, .len = 0, .bits = 517 /* KILN_C99_METHOD_NODE_FS_STAT_SYNC */ };
    kiln_value fs_lstat_sync = (kiln_value){ .tag = 11, .flags = 0, .len = 0, .bits = 518 /* KILN_C99_METHOD_NODE_FS_LSTAT_SYNC */ };
    kiln_value fs_readdir_sync = (kiln_value){ .tag = 11, .flags = 0, .len = 0, .bits = 519 /* KILN_C99_METHOD_NODE_FS_READDIR_SYNC */ };
    kiln_value fs_realpath_sync = (kiln_value){ .tag = 11, .flags = 0, .len = 0, .bits = 520 /* KILN_C99_METHOD_NODE_FS_REALPATH_SYNC */ };
    kiln_value fs_unlink_sync = (kiln_value){ .tag = 11, .flags = 0, .len = 0, .bits = 521 /* KILN_C99_METHOD_NODE_FS_UNLINK_SYNC */ };
    kiln_value fs_rmdir_sync = (kiln_value){ .tag = 11, .flags = 0, .len = 0, .bits = 522 /* KILN_C99_METHOD_NODE_FS_RMDIR_SYNC */ };
    status = kiln_c99_put(host, fs_obj, "existsSync", 10, fs_exists_sync, 0, NULL);
    if (status != KILN_C99_OK) return status;
    status = kiln_c99_put(host, fs_obj, "readFileSync", 12, fs_read_file_sync, 0, NULL);
    if (status != KILN_C99_OK) return status;
    status = kiln_c99_put(host, fs_obj, "writeFileSync", 13, fs_write_file_sync, 0, NULL);
    if (status != KILN_C99_OK) return status;
    status = kiln_c99_put(host, fs_obj, "mkdirSync", 9, fs_mkdir_sync, 0, NULL);
    if (status != KILN_C99_OK) return status;
    status = kiln_c99_put(host, fs_obj, "statSync", 8, fs_stat_sync, 0, NULL);
    if (status != KILN_C99_OK) return status;
    status = kiln_c99_put(host, fs_obj, "lstatSync", 9, fs_lstat_sync, 0, NULL);
    if (status != KILN_C99_OK) return status;
    status = kiln_c99_put(host, fs_obj, "readdirSync", 11, fs_readdir_sync, 0, NULL);
    if (status != KILN_C99_OK) return status;
    status = kiln_c99_put(host, fs_obj, "realpathSync", 12, fs_realpath_sync, 0, NULL);
    if (status != KILN_C99_OK) return status;
    status = kiln_c99_put(host, fs_obj, "unlinkSync", 10, fs_unlink_sync, 0, NULL);
    if (status != KILN_C99_OK) return status;
    status = kiln_c99_put(host, fs_obj, "rmdirSync", 9, fs_rmdir_sync, 0, NULL);
    if (status != KILN_C99_OK) return status;
    status = kiln_c99_put(host, fs_obj, "promises", 8, fsp_obj, 0, NULL);
    if (status != KILN_C99_OK) return status;
    status = kiln_c99_put(host, modules_obj, "fs", 2, fs_obj, 0, NULL);
    if (status != KILN_C99_OK) return status;

    kiln_value ffi_obj = kiln_c99_undefined();
    status = kiln_c99_new_object(&ffi_obj);
    if (status != KILN_C99_OK) return status;
    kiln_value ffi_dlopen = (kiln_value){ .tag = 11, .flags = 0, .len = 0, .bits = 495 /* KILN_C99_METHOD_NODE_FFI_DLOPEN */ };
    kiln_value ffi_get_raw_pointer = (kiln_value){ .tag = 11, .flags = 0, .len = 0, .bits = 496 /* KILN_C99_METHOD_NODE_FFI_GET_RAW_POINTER */ };
    kiln_value ffi_to_array_buffer = (kiln_value){ .tag = 11, .flags = 0, .len = 0, .bits = 497 /* KILN_C99_METHOD_NODE_FFI_TO_ARRAY_BUFFER */ };
#if defined(_WIN32)
    kiln_value ffi_suffix = kiln_c99_copy_text(".dll", 4);
#elif defined(__APPLE__)
    kiln_value ffi_suffix = kiln_c99_copy_text(".dylib", 6);
#else
    kiln_value ffi_suffix = kiln_c99_copy_text(".so", 3);
#endif
    status = kiln_c99_put(host, ffi_obj, "dlopen", 6, ffi_dlopen, 0, NULL);
    if (status != KILN_C99_OK) return status;
    status = kiln_c99_put(host, ffi_obj, "getRawPointer", 13, ffi_get_raw_pointer, 0, NULL);
    if (status != KILN_C99_OK) return status;
    status = kiln_c99_put(host, ffi_obj, "toArrayBuffer", 13, ffi_to_array_buffer, 0, NULL);
    if (status != KILN_C99_OK) return status;
    status = kiln_c99_put(host, ffi_obj, "suffix", 6, ffi_suffix, 0, NULL);
    if (status != KILN_C99_OK) return status;
    status = kiln_c99_put(host, modules_obj, "ffi", 3, ffi_obj, 0, NULL);
    if (status != KILN_C99_OK) return status;

    status = kiln_c99_put(host, proc_obj, "__kilnNativeModules", 19, modules_obj, 0, NULL);
    return status;
}

static int kiln_graph_enter(void *opaque) {
    kiln_graph_context *graph = (kiln_graph_context *)opaque;
    kiln_graph_entry_context context;
    kiln_host *host;
    kiln_value *result;
    int status = 0;
    if (graph == NULL) return -1;
    host = &graph->host;
    result = &graph->result;
    (void)context;
"#,
    );
    for base_index in 0..runtime_importers.len() {
        writeln!(
            output,
            "    graph->require_contexts[{base_index}].graph = graph; graph->require_contexts[{base_index}].base_index = (size_t){base_index};"
        )
        .unwrap();
    }
    output.push_str(
        r#"
    if (!graph->roots_registered) {
        status = kiln_realm_gc_register_roots(kiln_graph_gc_roots, graph, KILN_GC_ROOT_SURVIVES_TEARDOWN);
        if (status != 0) return status;
        graph->roots_registered = 1;
        status = kiln_install_native_modules(host);
        if (status != 0) return status;
"#,
    );
    if let Some(bootstrap) = bootstrap {
        writeln!(
            output,
            "        status = {}(host, result, NULL, 0, NULL, 0);\n        if (status != 0) {{ if (result->tag == 9 /* KILN_VALUE_OBJECT */) *result = kiln_cli_describe_error(*result); if (result->tag != KILN_VALUE_STRING || result->len == 0) *result = kiln_graph_diagnostic(\"builtin-bootstrap\", \"@kiln/runtime-bootstrap\"); return status; }}\n        kiln_c99_builtin_store_epoch = 0;",
            bootstrap.symbol
        )
        .unwrap();
    }
    for (index, (node, module)) in modules.iter().enumerate() {
        if node.source == ModuleSource::CommonJs {
            let paths = CommonJsModulePaths::new(graph.project_root(), &node.path)?;
            write_graph_commonjs_realm_setup(
                &mut output,
                index,
                &module.symbol,
                &paths.filename,
                &paths.dirname,
                &paths.diagnostic,
            );
        }
    }
    output.push_str("    }\n");
    writeln!(
        evaluation,
        "    while (graph->current_module < (size_t){}) {{\n        switch (graph->current_module) {{",
        startup_indices.len()
    )
    .unwrap();
    let mut helper_defs = String::new();
    // Single Rust-level emission point for a module case; generated C remains unchanged.
    let mut emit_module_case = |index: usize,
                                node: &kiln_module_graph::ModuleNode,
                                module: &kiln_backend_c::CModule|
     -> Result<(), NativeError> {
        let mut evaluation = &mut helper_defs;
        let is_async = node
            .program
            .program
            .code
            .iter()
            .any(|i| matches!(i, kiln_ir::Instruction::Await | kiln_ir::Instruction::Yield));
        writeln!(
            evaluation,
            "static int kiln_graph_eval_module_{index}(kiln_graph_context *graph) {{\n    kiln_graph_entry_context context;\n    kiln_graph_module_state *state = &graph->module_states[{index}];\n    kiln_host *host = &graph->host;\n    kiln_value *result = &state->result;\n    kiln_value continuation_promise = kiln_c99_undefined();\n    int status = 0;"
        )
        .unwrap();
        if !is_async {
            evaluation.push_str("    if (state->phase == KILN_MODULE_EVALUATED) return 0;\n    state->phase = KILN_MODULE_EVALUATING;\n");
        }
        let export_count = native_graph_export_count(node);
        let exports = if export_count == 0 {
            "NULL".to_owned()
        } else {
            format!("kiln_exports_{index}")
        };
        if node.source == ModuleSource::CommonJs {
            writeln!(
                evaluation,
                "            status = kiln_graph_load_commonjs(&kiln_commonjs_{index}, result, result);\n            if (status != 0) {{ if (status != KILN_C99_THROWN && result->tag == KILN_VALUE_UNDEFINED) {{ if (status == KILN_C99_HOST_ERROR && graph->cli.last_error.tag == KILN_VALUE_STRING && graph->cli.last_error.len != 0) *result = graph->cli.last_error; else *result = kiln_graph_diagnostic(\"commonjs-evaluate\", {}); }} return status; }}",
                native_c_string(&logical_paths[index])
            )
            .unwrap();
            if node.path == graph.entry() {
                evaluation.push_str("            graph->entry_result = *result;\n");
            }
            writeln!(
                evaluation,
                "            kiln_namespaces[{index}] = kiln_exports_{index}[0];"
            )
            .unwrap();
            if let Some(plan) = &namespace_plans[index] {
                write_graph_namespace_build(&mut evaluation, index, &module.symbol, plan, false);
                writeln!(
                    evaluation,
                    "            if (status != 0) {{ *result = kiln_graph_diagnostic(\"namespace\", {}); return status; }}",
                    native_c_string(&logical_paths[index])
                )
                .unwrap();
            }
            if !is_async {
                evaluation.push_str("    state->phase = KILN_MODULE_EVALUATED;\n");
            }
            evaluation.push_str("    return 0;\n}\n");
            return Ok(());
        }

        if is_async {
            evaluation.push_str("    if (state->phase == KILN_MODULE_EVALUATED) return 0;\n    if (state->phase == KILN_MODULE_FAILED) return state->failure_status;\n");
            writeln!(
                evaluation,
                r#"            if (state->module_settling) {{
                kiln_launcher_promise_view *promise = (kiln_launcher_promise_view *)(uintptr_t)state->active_promise.bits;
                if (promise != NULL) {{
                    if (promise->state == 2 /* KILN_C99_REJECTED */) {{
                        *result = promise->value;
                        return KILN_C99_THROWN;
                    }}
                    if (promise->state == 0 /* KILN_C99_PENDING */) {{
                        *result = kiln_graph_diagnostic("top-level await", "unsettled");
                        {mark_unsettled}return KILN_C99_CONTROL_ERROR;
                    }}
                }}
                {}_publish_exports({exports});
                state->phase = KILN_MODULE_EVALUATED;"#,
                module.symbol,
                mark_unsettled = if node.path == graph.entry() {
                    "graph->unsettled_entry = 1;\n                        "
                } else {
                    ""
                }
            )
            .unwrap();
            if node.path == graph.entry() {
                evaluation.push_str("                graph->entry_result = (promise != NULL) ? promise->value : *result;\n");
            }
            if let Some(plan) = &namespace_plans[index] {
                write_graph_namespace_build(&mut evaluation, index, &module.symbol, plan, false);
                writeln!(
                    evaluation,
                    "                if (status != 0) {{ *result = kiln_graph_diagnostic(\"namespace\", {}); return status; }}",
                    native_c_string(&logical_paths[index])
                )
                .unwrap();
            }
            evaluation.push_str(
                r#"                state->module_settling = 0;
                return 0;
            }
"#,
            );
        }

        let static_dependencies: Vec<usize> = node
            .dependencies
            .iter()
            .filter(|dependency| {
                node.program
                    .dependencies
                    .iter()
                    .any(|specifier| specifier == &dependency.specifier)
            })
            .filter_map(|dependency| indices.get(&dependency.target).copied())
            .collect();
        for (dependency_cursor, provider_index) in static_dependencies.iter().enumerate() {
            writeln!(
                evaluation,
                "            if (state->dependency_cursor == (size_t){dependency_cursor}) {{ status = kiln_graph_ensure_module(graph, (size_t){provider_index}); if (status != KILN_C99_OK) return status; state->dependency_cursor++; if (kiln_graph_module_promise(graph, (size_t){provider_index})->tag == 16 /* KILN_VALUE_PROMISE */) {{ status = kiln_graph_prepare_completion(graph, (size_t){index}); if (status != KILN_C99_OK) return status; status = kiln_c99_module_completion(host, *kiln_graph_module_promise(graph, (size_t){provider_index}), (uint32_t){index}, &continuation_promise, 1, NULL); if (status != KILN_C99_OK) return status; state->module_settling = 1; return 0; }} }}"
            )
            .unwrap();
        }

        for (import_index, import) in node.program.imports.iter().enumerate() {
            if live_imports
                .is_some_and(|imports| !imports.contains(&(node.path.clone(), import_index)))
            {
                continue;
            }
            match graph_import_value(graph, modules, &indices, node, import)? {
                NativeGraphImport::Internal => {}
                NativeGraphImport::Binding {
                    provider_index,
                    export_index,
                } => {
                    if modules[provider_index].0.source == ModuleSource::CommonJs {
                        writeln!(
                            evaluation,
                            "            status = kiln_graph_load_commonjs(&kiln_commonjs_{provider_index}, result, result);\n            if (status != 0) return status;"
                        )
                        .unwrap();
                    }
                    writeln!(
                        evaluation,
                        "            kiln_imports_{index}[{import_index}] = kiln_exports_{provider_index}[{export_index}];"
                    )
                    .unwrap();
                }
                NativeGraphImport::Namespace { provider_index } => {
                    if modules[provider_index].0.source == ModuleSource::CommonJs {
                        writeln!(
                            evaluation,
                            "            status = kiln_graph_load_commonjs(&kiln_commonjs_{provider_index}, result, result);\n            if (status != 0) return status;\n            kiln_namespaces[{provider_index}] = kiln_exports_{provider_index}[0];"
                        )
                        .unwrap();
                    }
                    writeln!(
                        evaluation,
                        "            kiln_imports_{index}[{import_index}] = kiln_namespaces[{provider_index}];"
                    )
                    .unwrap();
                }
                NativeGraphImport::CommonJsNamed {
                    provider_index,
                    property,
                } => {
                    writeln!(
                        evaluation,
                        "            status = kiln_graph_load_commonjs(&kiln_commonjs_{provider_index}, result, result);\n            if (status != 0) return status;"
                    )
                    .unwrap();
                    let prop_c = native_c_string(&property);
                    let prop_len = property.len();
                    writeln!(
                        evaluation,
                        "            status = kiln_c99_get(host, kiln_exports_{provider_index}[0], {prop_c}, (size_t){prop_len}, &kiln_imports_{index}[{import_index}], 0, result);\n            if (status != 0) return status;"
                    )
                    .unwrap();
                }
            }
        }
        let imports = if node.program.imports.is_empty() {
            "NULL".to_owned()
        } else {
            format!("kiln_imports_{index}")
        };
        writeln!(
            evaluation,
            "            context.module = {};",
            module.symbol
        )
        .unwrap();
        evaluation
            .push_str("            context.host = host;\n            context.result = result;\n");
        writeln!(evaluation, "            context.imports = {imports};").unwrap();
        writeln!(
            evaluation,
            "            context.import_count = (size_t){};",
            node.program.imports.len()
        )
        .unwrap();
        writeln!(evaluation, "            context.exports = {exports};").unwrap();
        writeln!(
            evaluation,
            "            context.export_count = (size_t){};",
            export_count
        )
        .unwrap();
        evaluation.push_str(
            "            state->body_started = 1;\n            status = context.module(context.host, context.result, context.imports, context.import_count, context.exports, context.export_count);\n",
        );
        if node.path == graph.entry() && !is_async {
            // The whole graph evaluates inside one job (a dependency's body runs synchronously
            // before the entry's), so the checkpoint that follows is the entry's, once. An entry
            // with a top-level await takes it below, once its completion is attached.
            evaluation.push_str(
                "            if (status == 0) status = kiln_c99_finish_module_checkpoint(host, result);\n",
            );
        }
        writeln!(
            evaluation,
            "            if (status != 0) {{ if (status != KILN_C99_THROWN && result->tag == KILN_VALUE_UNDEFINED) {{ if (status == KILN_C99_HOST_ERROR && graph->cli.last_error.tag == KILN_VALUE_STRING && graph->cli.last_error.len != 0) *result = graph->cli.last_error; else *result = kiln_graph_diagnostic(\"module-enter\", {}); }} return status; }}",
            native_c_string(&logical_paths[index])
        )
        .unwrap();
        if is_async {
            writeln!(
                evaluation,
                r#"            if (result->tag == 16 /* KILN_VALUE_PROMISE */) {{
                kiln_launcher_promise_view *promise = (kiln_launcher_promise_view *)(uintptr_t)result->bits;
                if (promise != NULL && promise->state == 0 /* KILN_C99_PENDING */) {{
                    state->active_promise = *result;
                    status = kiln_graph_prepare_completion(graph, (size_t){index});
                    if (status != KILN_C99_OK) return status;
                    status = kiln_c99_module_completion(host, *result, (uint32_t){index}, &continuation_promise, 1, NULL);
                    if (status != KILN_C99_OK) return status;
                    state->module_settling = 1;
                    {checkpoint}return 0;
                }}
                if (promise != NULL && promise->state == 2 /* KILN_C99_REJECTED */) {{
                    *result = promise->value;
                    return KILN_C99_THROWN;
                }}
                {}_publish_exports({exports});
                state->phase = KILN_MODULE_EVALUATED;
            }}"#,
                module.symbol,
                // The entry's body suspended inside the evaluation job; the checkpoint that
                // follows -- the promise queue before the tick lane -- runs once its completion
                // is attached, so a rejection of the module promise is seen the ordinary way.
                checkpoint = if node.path == graph.entry() {
                    "status = kiln_c99_finish_module_checkpoint(host, result);\n                    if (status != KILN_C99_OK) return status;\n                    "
                } else {
                    ""
                }
            )
            .unwrap();
        }
        if node.path == graph.entry() {
            if is_async {
                evaluation.push_str(
                    r#"            if (result->tag == 16) {
                kiln_launcher_promise_view *promise = (kiln_launcher_promise_view *)(uintptr_t)result->bits;
                graph->entry_result = (promise != NULL && promise->state == 1) ? promise->value : *result;
            } else {
                graph->entry_result = *result;
            }
"#,
                );
            } else {
                evaluation.push_str("            graph->entry_result = *result;\n");
            }
        }
        if let Some(plan) = &namespace_plans[index] {
            write_graph_namespace_build(&mut evaluation, index, &module.symbol, plan, false);
            writeln!(
                evaluation,
                "            if (status != 0) {{ *result = kiln_graph_diagnostic(\"namespace\", {}); return status; }}",
                native_c_string(&logical_paths[index])
            )
            .unwrap();
        }
        if !is_async {
            evaluation.push_str("    state->phase = KILN_MODULE_EVALUATED;\n");
        }
        evaluation.push_str("    if (state->completion_resolve.tag != KILN_VALUE_UNDEFINED) { kiln_value settled = kiln_c99_undefined(); status = kiln_c99_call(host, &settled, state->completion_resolve, kiln_c99_undefined(), &kiln_namespaces[");
        evaluation.push_str(&index.to_string());
        evaluation.push_str("], 1, 0, NULL); if (status != KILN_C99_OK) return status; }\n");
        evaluation.push_str("    return 0;\n}\n");
        Ok(())
    };
    for (index, (node, module)) in modules.iter().enumerate() {
        emit_module_case(index, node, module)?;
    }
    helper_defs.push_str("int kiln_graph_module_completion(kiln_host *host, uint32_t index, int rejected, const kiln_value *value, kiln_value *result, int depth, kiln_value *thrown) {\n    kiln_graph_context *graph = (kiln_graph_context *)host->context;\n    int status = KILN_C99_OK;\n    (void)depth; (void)thrown;\n    if (graph == NULL || result == NULL || value == NULL) return KILN_C99_CONTROL_ERROR;\n    switch (index) {\n");
    for (index, (node, module)) in modules.iter().enumerate() {
        let fatal = if startup_indices.contains(&index) {
            FATAL_UNHANDLED_REJECTION
        } else {
            ""
        };
        helper_defs.push_str(&format!(
            "        case {index}: {{\n            kiln_graph_module_state *state = &graph->module_states[{index}];\n            kiln_value settled = kiln_c99_undefined();\n            if (rejected) {{ state->result = *value; state->phase = KILN_MODULE_FAILED; state->failure_status = KILN_C99_THROWN; state->module_settling = 0; *result = *value; if (state->completion_reject.tag != KILN_VALUE_UNDEFINED) {{ status = kiln_c99_call(host, &settled, state->completion_reject, kiln_c99_undefined(), value, 1, depth, thrown); if (status != KILN_C99_OK) return status; }} {fatal} return KILN_C99_OK; }}\n            if (!state->body_started) {{ state->module_settling = 0; status = kiln_graph_eval_module_{index}(graph); if (status != KILN_C99_OK) {{ state->phase = KILN_MODULE_FAILED; state->failure_status = status; *result = state->result; if (state->completion_reject.tag != KILN_VALUE_UNDEFINED) {{ status = kiln_c99_call(host, &settled, state->completion_reject, kiln_c99_undefined(), &state->result, 1, depth, thrown); if (status != KILN_C99_OK) return status; }} return KILN_C99_OK; }} *result = kiln_c99_undefined(); return KILN_C99_OK; }}\n            if (state->phase == KILN_MODULE_EVALUATED) {{ *result = kiln_namespaces[{index}]; return KILN_C99_OK; }}\n",
            index = index,
            fatal = fatal
        ));
        let is_async = node
            .program
            .program
            .code
            .iter()
            .any(|i| matches!(i, kiln_ir::Instruction::Await | kiln_ir::Instruction::Yield));
        if is_async && native_graph_export_count(node) != 0 {
            helper_defs.push_str(&format!(
                "            {symbol}_publish_exports(kiln_exports_{index});\n",
                symbol = module.symbol,
                index = index
            ));
        }
        if let Some(plan) = &namespace_plans[index] {
            write_graph_namespace_build(&mut helper_defs, index, &module.symbol, plan, false);
        }
        helper_defs.push_str(&format!("            state->result = kiln_namespaces[{index}];\n            state->phase = KILN_MODULE_EVALUATED;\n            state->module_settling = 0;\n            *result = state->result;\n            if (state->completion_resolve.tag != KILN_VALUE_UNDEFINED) {{ status = kiln_c99_call(host, &settled, state->completion_resolve, kiln_c99_undefined(), &state->result, 1, depth, thrown); if (status != KILN_C99_OK) return status; }}\n", index = index));
        if node.path == graph.entry() {
            helper_defs.push_str("            graph->entry_result = *value;\n");
        }
        helper_defs.push_str("            return KILN_C99_OK;\n        }\n");
        let _ = node;
    }
    helper_defs.push_str("        default: return KILN_C99_CONTROL_ERROR;\n    }\n}\n");
    for (startup_order, &index) in startup_indices.iter().enumerate() {
        let is_async = modules[index]
            .0
            .program
            .program
            .code
            .iter()
            .any(|i| matches!(i, kiln_ir::Instruction::Await | kiln_ir::Instruction::Yield));
        if is_async {
            writeln!(evaluation, "        case {startup_order}: {{ status = kiln_graph_eval_module_{index}(graph); graph->result = graph->module_states[{index}].result; if (status != 0) return status; if (!graph->module_states[{index}].module_settling) graph->current_module++; return 0; }}").unwrap();
        } else {
            writeln!(evaluation, "        case {startup_order}: {{ status = kiln_graph_ensure_module(graph, (size_t){index}); graph->result = graph->module_states[{index}].result; if (status != 0) return status; graph->current_module++; return 0; }}").unwrap();
        }
    }
    evaluation.push_str(
        r#"        default:
            return 0;
        }
    }
"#,
    );
    output.push_str(&evaluation);
    output.push_str("    return 0;\n}\n");
    output.push_str(&helper_defs);
    output.push_str(
        "static int kiln_graph_ensure_module(kiln_graph_context *graph, size_t index) {\n",
    );
    output.push_str("    int status;\n");
    output.push_str("    if (graph == NULL || index >= (size_t)");
    output.push_str(&modules.len().to_string());
    output.push_str(
        ") return KILN_C99_CONTROL_ERROR;\n    if (graph->module_states[index].phase == KILN_MODULE_EVALUATED) return 0;\n    if (graph->module_states[index].phase == KILN_MODULE_FAILED) return graph->module_states[index].failure_status;\n    if (graph->module_states[index].phase == KILN_MODULE_EVALUATING) return 0;\n    graph->module_states[index].phase = KILN_MODULE_EVALUATING;\n    switch (index) {\n",
    );
    for index in 0..modules.len() {
        writeln!(output, "        case {index}: {{").unwrap();
        for (dependency_index, dependency) in modules[index].0.dependencies.iter().enumerate() {
            if let Some(&provider) = indices.get(&dependency.target) {
                writeln!(
                    output,
                    "            {}status = kiln_graph_ensure_module(graph, (size_t){provider}); if (status != KILN_C99_OK) {{ graph->module_states[index].result = graph->module_states[{provider}].result; graph->module_states[index].phase = KILN_MODULE_FAILED; graph->module_states[index].failure_status = status; return status; }}",
                    if dependency_index == 0 { "int " } else { "" }
                )
                .unwrap();
            }
        }
        writeln!(
            output,
            "            status = kiln_graph_eval_module_{index}(graph);\n            if (status != KILN_C99_OK) {{ graph->module_states[index].phase = KILN_MODULE_FAILED; graph->module_states[index].failure_status = status; return status; }}\n            return 0;\n        }}"
        )
        .unwrap();
    }
    output.push_str("        default: return KILN_C99_CONTROL_ERROR;\n    }\n}\n");
    output.push_str(
        r#"/* argc/argv reach the realm before it is entered, so the first `process` a module builds
 * already has the command line. The strings are copied on the other side of this call. */
static int kiln_cli_run_realm(int argc, char **argv, int writes_result) {
    /* The realm's graph: its module states and every module's linkage, one per realm run and freed
     * after the realm is destroyed (a result slot is read after teardown). */
    kiln_graph_context *graph = (kiln_graph_context *)calloc(1, sizeof *graph);
    uint64_t realm = 0;
    /* A refusal that reaches here has no diagnostic of its own -- the realm stopped before it
     * could leave one -- so the launcher says which of the three phases was running. */
    const char *phase = "realm creation";
    int status;
    int destroy_status;
    int unsettled_entry = 0;
    if (graph == NULL) {
        kiln_cli_warn("kiln: out of memory for the module graph");
        return 1;
    }
    graph->result.tag = KILN_VALUE_UNDEFINED;
    graph->entry_result.tag = KILN_VALUE_UNDEFINED;
    graph->host.call = kiln_graph_host_call;
    graph->host.context = graph;
    status = kiln_realm_create(&realm);
    if (status == 0 && argc > 0)
        status = kiln_realm_set_argv(realm, (uint64_t)argc, (const char *const *)argv);
"#,
    );
    if policy.host_clock {
        // ADR 0044: a shipped realm keeps wall time, and the switch happens before the realm is
        // entered so no timer is ever armed against the virtual clock.
        output.push_str("    if (status == 0) status = kiln_realm_use_wall_clock(realm);\n");
    }
    writeln!(
        output,
        r#"    while (status == 0 && (graph->current_module < (size_t){} || !graph->roots_registered)) {{
        if (graph->current_module < (size_t){} && graph->module_states[kiln_graph_startup_indices[graph->current_module]].module_settling) {{
            phase = "the turn loop";
            status = kiln_realm_drive(realm, &graph->host, &graph->result);
            if (status != 0) break;
        }}
        phase = "module body";
        status = kiln_realm_enter(realm, kiln_graph_enter, graph);
    }}
        if (status == 0) {{ phase = "the turn loop"; status = kiln_realm_drive(realm, &graph->host, &graph->result); }}"#,
        startup_indices.len(),
        startup_indices.len()
    )
    .unwrap();
    output.push_str(
        r##"
    if (status == 0 && graph->entry_result.tag == 16 /* KILN_VALUE_PROMISE */) {
        typedef struct kiln_launcher_promise_view {
            int state;
            kiln_value value;
        } kiln_launcher_promise_view;
        kiln_launcher_promise_view *promise = (kiln_launcher_promise_view *)(uintptr_t)graph->entry_result.bits;
        if (promise != NULL) {
            if (promise->state == 2 /* KILN_C99_REJECTED */) {
                status = KILN_C99_THROWN;
                phase = "module body";
                graph->result = kiln_cli_describe_error(promise->value);
            } else if (promise->state == 0 /* KILN_C99_PENDING */) {
                status = KILN_C99_CONTROL_ERROR;
                phase = "the turn loop";
                unsettled_entry = 1;
                graph->result = kiln_graph_diagnostic("top-level await", "unsettled");
            } else if (promise->state == 1 /* KILN_C99_FULFILLED */) {
                graph->entry_result = promise->value;
            }
        }
    }
    /* Graph module entries carry the original thrown value across the module ABI so dynamic
     * import and catch preserve language identity. Only now, after the graph and its turn loop
     * have established that the value is uncaught, attach the native refusal provenance. A
     * handled throw resets the realm diagnostic bridge, so this cannot leak an earlier refusal
     * into a later ordinary error. */
    if (status == KILN_C99_THROWN) {
        kiln_value message = kiln_cli_describe_error(graph->result);
        if (message.tag != KILN_VALUE_STRING) {
            message = kiln_c99_text(message);
        }
        kiln_value site = kiln_c99_control_diagnostic();
        if (kiln_c99_diagnostic_available() && message.tag == KILN_VALUE_STRING &&
            site.tag == KILN_VALUE_STRING && site.len != 0) {
            kiln_value separator = kiln_c99_copy_text(" at ", 4);
            graph->result = kiln_c99_concat(kiln_c99_concat(message, separator), site);
        } else {
            graph->result = message;
        }
    }
    /* An entry whose top-level await never settled, with the loop out of work: node's answer is a
     * warning and status 13 rather than a refusal, and both engines give that one. */
    unsettled_entry = status == KILN_C99_CONTROL_ERROR && (unsettled_entry || graph->unsettled_entry);
    if (unsettled_entry) kiln_cli_warn("Warning: Detected unsettled top-level await");
    else if (status != 0) kiln_cli_report_failure_with(status, graph->result, phase, &graph->cli);
    if (writes_result && status == 0 && kiln_cli_write_result(graph->entry_result) != 0) {
        static const kiln_string refusal_str = { 17, 0, "filesystem access" };
        kiln_cli_report_failure_with(KILN_C99_HOST_ERROR, (kiln_value){ .tag = KILN_VALUE_STRING, .flags = 0, .len = 17, .ptr = (const char *)&refusal_str }, "result write", &graph->cli);
        status = 1;
    }
    destroy_status = kiln_realm_destroy(realm);
    free(graph);
    /* `main` folds every failure to 1, the way it does a refusal; this is the one status a
     * program's caller is told apart by, as `process.exit(code)` is. */
    if (unsettled_entry) exit(13);
    if (status == 0) status = destroy_status;
    return status == 0 ? 0 : 1;
}

/* How many realms to run, read from the environment rather than fixed at build time.
 *
 * A server binds with SO_REUSEPORT, so N of these share a port and the kernel spreads the accepts
 * across them. Reading the count at startup keeps one artifact correct on every machine it is
 * copied to, which is the point of shipping a binary; baking it in at `kiln build` would tie the
 * artifact to the box that built it.
 *
 * "auto" (case-insensitive) queries online CPU cores via sysconf(_SC_NPROCESSORS_ONLN).
 * Absent, unparseable or below one means one realm -- exactly the behaviour before this existed. */
static long kiln_cli_worker_count(void) {
    const char *setting = getenv("KILN_WORKERS");
    char *end = NULL;
    long count;
    if (setting == NULL || *setting == '\0') return 1;
    if ((setting[0] == 'a' || setting[0] == 'A') &&
        (setting[1] == 'u' || setting[1] == 'U') &&
        (setting[2] == 't' || setting[2] == 'T') &&
        (setting[3] == 'o' || setting[3] == 'O') &&
        setting[4] == '\0') {
#ifndef _WIN32
        long cpus = sysconf(_SC_NPROCESSORS_ONLN);
        count = cpus > 0 ? cpus : 1;
#else
        count = 1;
#endif
    } else {
        count = strtol(setting, &end, 10);
        if (end == setting || *end != '\0' || count < 1) return 1;
    }
    if (count < 1) count = 1;
    if (count > 1024) count = 1024;
    return count;
}

/* Whether HTTP/3 support is enabled via KILN_HTTP3=1 or --http3 flag. */
#ifndef _WIN32
extern int setenv(const char *name, const char *value, int overwrite);
#endif
static int kiln_cli_http3_enabled(int argc, char **argv) {
    const char *setting = getenv("KILN_HTTP3");
    int i;
    if (setting != NULL && *setting != '\0' && strcmp(setting, "0") != 0) return 1;
    for (i = 1; i < argc; ++i) {
        if (strcmp(argv[i], "--http3") == 0) return 1;
    }
    return 0;
}

/* Extra realms are separate *processes*, not threads. That was a constraint: a module's
 * namespaces lived in a process-global `kiln_namespaces` while the values inside it belong to the
 * realm that made them (ADR 0038), so two realms in one address space wrote one array and read
 * each other's values -- `undefined is not a function` at the first import, measured before this
 * used fork. Since realms issue 04 step 5 the namespaces, module globals and the rest of a realm's
 * state are the realm's own (`kiln_graph_context`, the units' realm blocks), and realms in threads
 * are issue 05's to build; until then this stays a process per realm.
 *
 * A process per realm is also what `node`'s cluster does, for the same reason, and the text
 * segment is shared copy-on-write so the cost is the heaps, which were never shareable anyway.
 *
 * With more than one worker the first process is a *supervisor* and runs no realm of its own,
 * which is what lets it restart a worker at all: a re-fork has to come from a process that has
 * never built a realm, because a realm is threads (the provider's runtime) plus process-global
 * state, and forking either into a fresh worker hands it locks some other thread held and
 * namespaces another realm filled. So `KILN_WORKERS=N` is N realm processes (indices 0..N-1)
 * under one supervisor that never enters JavaScript -- node's cluster primary, without the
 * program running in it. The supervisor:
 *
 *  - forwards SIGTERM, SIGINT, SIGHUP and SIGQUIT to every live worker, stops restarting, waits
 *    for all of them, then dies of the same signal (or exits 128+signal when it is PID 1, where
 *    a default action never fires). A worker receives exactly the signal a single-process
 *    server would, so it behaves exactly as one does. `docker stop` therefore drains every
 *    worker before PID 1 exits and the namespace is torn down. SIGUSR1 (the introspection dump)
 *    is forwarded without stopping anything.
 *  - restarts a worker that dies of a signal or exits non-zero, with backoff; one that exits 0
 *    is finished and is not restarted. A worker that crashes more than KILN_SUP_MAX_STREAK times in a row,
 *    each within KILN_SUP_STABLE_MS of starting, is a crash loop: the supervisor stops the
 *    others and exits with that worker's status.
 *  - arranges for workers to die with it: PR_SET_PDEATHSIG on Linux, and elsewhere a pipe whose
 *    far end only the supervisor holds, watched by one thread in each worker.
 *
 * Each worker sees `KILN_WORKER_INDEX` (0..N-1; a restarted worker keeps its index) and
 * `KILN_WORKER_COUNT` in its environment before its realm starts, so `process.env` answers in
 * both engines with no new built-in. A single realm sets neither, so an unset variable means
 * "the only worker", and a program run without `KILN_WORKERS` sees the environment node would.
 * Worker 0 is the one that writes the result file: one path, one writer.
 *
 * Where the count comes from is `kiln_cli_worker_count`'s business alone; a count recorded in
 * the binary at build time would be a default there, and nothing below cares. */
#ifndef _WIN32
#define KILN_SUP_MAX_STREAK 5
#define KILN_SUP_STABLE_MS 5000LL
#define KILN_SUP_BACKOFF_MS 100LL
#define KILN_SUP_BACKOFF_CAP_MS 2000LL

static int kiln_sup_pipe[2] = { -1, -1 };
static volatile sig_atomic_t kiln_sup_stop_signal = 0;
static const int kiln_sup_signals[] = { SIGTERM, SIGINT, SIGHUP, SIGQUIT, SIGUSR1, SIGCHLD };
#define KILN_SUP_SIGNAL_COUNT (sizeof kiln_sup_signals / sizeof kiln_sup_signals[0])

static int kiln_sup_is_stop_signal(int sig) {
    return sig == SIGTERM || sig == SIGINT || sig == SIGHUP || sig == SIGQUIT;
}

/* Async-signal-safe: record, and wake the loop through the self-pipe. Everything else happens
 * in the loop, outside the handler. */
static void kiln_sup_on_signal(int sig) {
    int saved = errno;
    unsigned char byte = (unsigned char)sig;
    ssize_t ignored;
    if (kiln_sup_is_stop_signal(sig) && kiln_sup_stop_signal == 0) kiln_sup_stop_signal = sig;
    if (kiln_sup_pipe[1] >= 0) {
        ignored = write(kiln_sup_pipe[1], &byte, 1);
        (void)ignored;
    }
    errno = saved;
}

static long long kiln_sup_now_ms(void) {
    struct timespec now;
    if (clock_gettime(CLOCK_MONOTONIC, &now) != 0) return 0;
    return (long long)now.tv_sec * 1000LL + (long long)(now.tv_nsec / 1000000L);
}

static void kiln_sup_cloexec_nonblock(int fd, int nonblock) {
    int flags = fcntl(fd, F_GETFD);
    if (flags >= 0) fcntl(fd, F_SETFD, flags | FD_CLOEXEC);
    if (nonblock) {
        flags = fcntl(fd, F_GETFL);
        if (flags >= 0) fcntl(fd, F_SETFL, flags | O_NONBLOCK);
    }
}

#if !defined(__linux__)
/* The supervisor holds the only write end, so the read returns end-of-file exactly when it has
 * gone, however it went. The worker then gets the SIGTERM PR_SET_PDEATHSIG would have sent. */
static void *kiln_sup_watch_supervisor(void *argument) {
    int fd = (int)(intptr_t)argument;
    char byte;
    for (;;) {
        ssize_t read_count = read(fd, &byte, 1);
        if (read_count < 0 && errno == EINTR) continue;
        break;
    }
    kill(getpid(), SIGTERM);
    return NULL;
}
#endif

/* Runs in the fresh child and never returns. Signals are blocked on entry (the supervisor
 * blocks them around fork), so the supervisor's handler can never run here. */
static void kiln_sup_worker(int argc, char **argv, long index, long workers, pid_t supervisor,
                            const sigset_t *original_mask, int death_fd) {
    char text[32];
    size_t signal_index;
    struct sigaction action;
    memset(&action, 0, sizeof action);
    action.sa_handler = SIG_DFL;
    sigemptyset(&action.sa_mask);
    for (signal_index = 0; signal_index < KILN_SUP_SIGNAL_COUNT; ++signal_index)
        sigaction(kiln_sup_signals[signal_index], &action, NULL);
    close(kiln_sup_pipe[0]);
    close(kiln_sup_pipe[1]);
    kiln_sup_pipe[0] = kiln_sup_pipe[1] = -1;
#if defined(__linux__)
    (void)death_fd;
    (void)prctl(PR_SET_PDEATHSIG, SIGTERM);
    /* The supervisor may have died between fork and prctl, which then never fires. */
    if (getppid() != supervisor) _exit(1);
#else
    (void)supervisor;
    if (death_fd >= 0) {
        pthread_t watcher;
        pthread_attr_t attributes;
        if (pthread_attr_init(&attributes) == 0) {
            pthread_attr_setdetachstate(&attributes, PTHREAD_CREATE_DETACHED);
            if (pthread_create(&watcher, &attributes, kiln_sup_watch_supervisor,
                               (void *)(intptr_t)death_fd) != 0)
                fputs("kiln: worker cannot watch its supervisor; it will outlive it\n", stderr);
            pthread_attr_destroy(&attributes);
        }
    }
#endif
    snprintf(text, sizeof text, "%ld", index);
    setenv("KILN_WORKER_INDEX", text, 1);
    snprintf(text, sizeof text, "%ld", workers);
    setenv("KILN_WORKER_COUNT", text, 1);
    sigprocmask(SIG_SETMASK, original_mask, NULL);
    /* `exit`, not `_exit`: a worker ends the way a single-realm `main` returning does. */
    exit(kiln_cli_run_realm(argc, argv, index == 0) == 0 ? 0 : 1);
}

static void kiln_sup_forward(const pid_t *pids, long workers, int sig) {
    long index;
    for (index = 0; index < workers; ++index)
        if (pids[index] > 0) kill(pids[index], sig);
}

static int kiln_sup_run(int argc, char **argv, long workers) {
    pid_t *pids = (pid_t *)calloc((size_t)workers, sizeof *pids);
    long long *started = (long long *)calloc((size_t)workers, sizeof *started);
    long long *restart_at = (long long *)calloc((size_t)workers, sizeof *restart_at);
    int *streak = (int *)calloc((size_t)workers, sizeof *streak);
    int death_pipe[2] = { -1, -1 };
    pid_t self = getpid();
    sigset_t blocked, original;
    struct sigaction action;
    size_t signal_index;
    long index, live = 0;
    int stopping = 0, stop_signal = 0, exit_status = 0;
    if (pids == NULL || started == NULL || restart_at == NULL || streak == NULL) return 1;
    if (pipe(kiln_sup_pipe) != 0) {
        fprintf(stderr, "kiln: cannot start the worker supervisor: %s\n", strerror(errno));
        return 1;
    }
    kiln_sup_cloexec_nonblock(kiln_sup_pipe[0], 1);
    kiln_sup_cloexec_nonblock(kiln_sup_pipe[1], 1);
#if !defined(__linux__)
    if (pipe(death_pipe) == 0) {
        kiln_sup_cloexec_nonblock(death_pipe[0], 0);
        kiln_sup_cloexec_nonblock(death_pipe[1], 0);
    } else {
        death_pipe[0] = death_pipe[1] = -1;
    }
#endif
    sigemptyset(&blocked);
    for (signal_index = 0; signal_index < KILN_SUP_SIGNAL_COUNT; ++signal_index)
        sigaddset(&blocked, kiln_sup_signals[signal_index]);
    sigprocmask(SIG_BLOCK, &blocked, &original);
    memset(&action, 0, sizeof action);
    action.sa_handler = kiln_sup_on_signal;
    sigemptyset(&action.sa_mask);
    for (signal_index = 0; signal_index < KILN_SUP_SIGNAL_COUNT; ++signal_index) {
        action.sa_flags = SA_RESTART | (kiln_sup_signals[signal_index] == SIGCHLD ? SA_NOCLDSTOP : 0);
        sigaction(kiln_sup_signals[signal_index], &action, NULL);
    }
    sigprocmask(SIG_SETMASK, &original, NULL);
    for (index = 0; index < workers; ++index) restart_at[index] = 0; /* due now */

    for (;;) {
        long long now = kiln_sup_now_ms();
        long long wait_ms = -1;
        struct pollfd wake;
        unsigned char bytes[64];
        ssize_t read_count;
        pid_t pid;
        int status;

        if (!stopping) {
            for (index = 0; index < workers; ++index) {
                if (pids[index] != 0 || restart_at[index] < 0 || restart_at[index] > now) continue;
                /* Blocked across fork, so the child starts with no supervisor handler able to run;
                 * pending signals are not inherited, and the child resets them before unblocking. */
                sigprocmask(SIG_BLOCK, &blocked, NULL);
                pid = fork();
                if (pid == 0) kiln_sup_worker(argc, argv, index, workers, self, &original, death_pipe[0]);
                sigprocmask(SIG_SETMASK, &original, NULL);
                if (pid < 0) {
                    long long delay;
                    int fork_error = errno;
                    streak[index] += 1;
                    if (streak[index] > KILN_SUP_MAX_STREAK) {
                        fprintf(stderr, "kiln: cannot fork worker %ld: %s; giving up\n", index, strerror(fork_error));
                        exit_status = 1;
                        stopping = 1;
                        kiln_sup_forward(pids, workers, SIGTERM);
                        break;
                    }
                    delay = KILN_SUP_BACKOFF_MS << (streak[index] - 1);
                    if (delay > KILN_SUP_BACKOFF_CAP_MS) delay = KILN_SUP_BACKOFF_CAP_MS;
                    fprintf(stderr, "kiln: cannot fork worker %ld: %s; retrying in %lld ms\n", index, strerror(fork_error), delay);
                    restart_at[index] = now + delay;
                    continue;
                }
                pids[index] = pid;
                started[index] = now;
                restart_at[index] = -1;
                live += 1;
            }
        }
        if (!stopping) {
            for (index = 0; index < workers; ++index) {
                if (pids[index] != 0 || restart_at[index] < 0) continue;
                if (wait_ms < 0 || restart_at[index] - now < wait_ms)
                    wait_ms = restart_at[index] > now ? restart_at[index] - now : 0;
            }
        }
        if (live == 0 && wait_ms < 0) break;

        /* A second's cap keeps a lost wakeup (a full pipe) from ever mattering. */
        if (wait_ms < 0 || wait_ms > 1000) wait_ms = 1000;
        wake.fd = kiln_sup_pipe[0];
        wake.events = POLLIN;
        wake.revents = 0;
        (void)poll(&wake, 1, (int)wait_ms);

        while ((read_count = read(kiln_sup_pipe[0], bytes, sizeof bytes)) > 0) {
            ssize_t byte_index;
            for (byte_index = 0; byte_index < read_count; ++byte_index) {
                int sig = (int)bytes[byte_index];
                if (sig == SIGUSR1) {
                    kiln_sup_forward(pids, workers, SIGUSR1);
                } else if (kiln_sup_is_stop_signal(sig)) {
                    if (!stopping) stop_signal = sig;
                    stopping = 1;
                    kiln_sup_forward(pids, workers, sig);
                }
            }
        }
        if (!stopping && kiln_sup_stop_signal != 0) {
            stop_signal = (int)kiln_sup_stop_signal;
            stopping = 1;
            kiln_sup_forward(pids, workers, stop_signal);
        }

        now = kiln_sup_now_ms();
        for (;;) {
            long long lived, delay;
            pid = waitpid(-1, &status, WNOHANG);
            if (pid < 0 && errno == EINTR) continue;
            if (pid <= 0) break;
            /* PID 1 also inherits orphans of whatever the program spawned; reaping them is
             * part of the job, and they are nobody's worker. */
            for (index = 0; index < workers && pids[index] != pid; ++index) {}
            if (index == workers) continue;
            pids[index] = 0;
            live -= 1;
            if (WIFEXITED(status) && WEXITSTATUS(status) == 0) continue;
            if (stopping) {
                /* A worker that dies of the forwarded signal is the drain succeeding. */
                continue;
            }
            lived = now - started[index];
            if (lived >= KILN_SUP_STABLE_MS) streak[index] = 0;
            streak[index] += 1;
            if (streak[index] > KILN_SUP_MAX_STREAK) {
                if (WIFSIGNALED(status))
                    fprintf(stderr, "kiln: worker %ld (pid %ld) was killed by signal %d, crash %d in a row within %lld ms of starting; giving up\n",
                            index, (long)pid, WTERMSIG(status), streak[index], KILN_SUP_STABLE_MS);
                else
                    fprintf(stderr, "kiln: worker %ld (pid %ld) exited with status %d, crash %d in a row within %lld ms of starting; giving up\n",
                            index, (long)pid, WEXITSTATUS(status), streak[index], KILN_SUP_STABLE_MS);
                exit_status = WIFEXITED(status) ? WEXITSTATUS(status) : 128 + WTERMSIG(status);
                stopping = 1;
                kiln_sup_forward(pids, workers, SIGTERM);
                continue;
            }
            delay = KILN_SUP_BACKOFF_MS << (streak[index] - 1);
            if (delay > KILN_SUP_BACKOFF_CAP_MS) delay = KILN_SUP_BACKOFF_CAP_MS;
            if (WIFSIGNALED(status))
                fprintf(stderr, "kiln: worker %ld (pid %ld) was killed by signal %d after %lld ms; restarting in %lld ms\n",
                        index, (long)pid, WTERMSIG(status), lived, delay);
            else
                fprintf(stderr, "kiln: worker %ld (pid %ld) exited with status %d after %lld ms; restarting in %lld ms\n",
                        index, (long)pid, WEXITSTATUS(status), lived, delay);
            restart_at[index] = now + delay;
        }
    }

    free(pids);
    free(started);
    free(restart_at);
    free(streak);
    if (stop_signal != 0) {
        /* Die the way a single-process server does. As PID 1 the default action never fires,
         * so fall through to the shell's spelling of the same thing. */
        memset(&action, 0, sizeof action);
        action.sa_handler = SIG_DFL;
        sigemptyset(&action.sa_mask);
        sigaction(stop_signal, &action, NULL);
        sigemptyset(&blocked);
        sigaddset(&blocked, stop_signal);
        sigprocmask(SIG_UNBLOCK, &blocked, NULL);
        raise(stop_signal);
        return 128 + stop_signal;
    }
    return exit_status;
}
#endif

int main(int argc, char **argv) {
    long workers;
    if (setlocale(LC_NUMERIC, "C") == NULL) return 1;
    if (kiln_cli_http3_enabled(argc, argv)) {
#ifndef _WIN32
        setenv("KILN_HTTP3", "1", 1);
#else
        _putenv("KILN_HTTP3=1");
#endif
    }
    workers = kiln_cli_worker_count();
#ifdef _WIN32
    (void)workers;
    return kiln_cli_run_realm(argc, argv, 1) == 0 ? 0 : 1;
#else
    if (workers <= 1) return kiln_cli_run_realm(argc, argv, 1) == 0 ? 0 : 1;
    return kiln_sup_run(argc, argv, workers);
#endif
}
"##,
    );
    Ok(output.replace(
        "UINT16_C(0x7FF0)",
        &format!("UINT16_C(0x{:04X})", C99_HOST_MODULE_CREATE_REQUIRE),
    ))
}
