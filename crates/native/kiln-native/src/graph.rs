use super::*;
use kiln_module_resolver::NodeBuiltin;
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

    let mut static_reachable = graph.statically_reachable();
    // A declared module is another possible runtime entry. Traverse its ordinary imports now
    // so validation and emission see the whole closure, while the launcher still evaluates only
    // the program entry until a worker or dynamic loader asks for this module.
    let mut include_work = graph
        .declared_includes()
        .iter()
        .cloned()
        .collect::<Vec<_>>();
    while let Some(path) = include_work.pop() {
        if !static_reachable.insert(path.clone()) {
            continue;
        }
        if let Some(node) = graph.module(&path) {
            include_work.extend(
                node.dependencies
                    .iter()
                    .map(|dependency| dependency.target.clone()),
            );
        }
    }
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
                    .filter(|dependency| !dependency.resolve_only)
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
                    if nested.resolve_only {
                        continue;
                    }
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
const __kilnRuntimeModules = globalThis.process.__kilnNativeModules;
const __kilnModuleRequireResolve = __kilnRuntimeModules.__kilnHostModuleRequireResolve;
delete __kilnRuntimeModules.__kilnHostModuleRequireResolve;

function __kilnDecorateCommonJsRequire(require) {
    const resolve = function resolve(specifier, options) {
        if (typeof specifier !== "string") {
            throw new TypeError("The module specifier must be a string");
        }
        return __kilnModuleRequireResolve(require, specifier);
    };
    Object.defineProperty(require, "resolve", {
        value: resolve,
        enumerable: true,
        writable: true,
        configurable: true
    });
    return require;
}
Object.defineProperty(__kilnRuntimeModules, "__kilnDecorateCommonJsRequire", {
    value: __kilnDecorateCommonJsRequire,
    configurable: true
});

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


// The shared typedarray.ts bootstrap owns Uint8Array's public codecs on both engines.
// Remove the native-only backing hooks after that bootstrap has captured any it needs.
delete globalThis.__kilnUint8ToHex;
delete globalThis.__kilnUint8ToBase64;

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
    // One descriptor for every member (realms issue 08: a literal per call was garbage in
    // every realm's bootstrap); `defineProperty` reads it and keeps nothing.
    const hidden = { value: undefined, writable: true, configurable: true, enumerable: false };
    for (let i = 0; i < added.length; i++) {
        hidden.value = proto[added[i]];
        Object.defineProperty(c, added[i], hidden);
    }
    hidden.value = Console;
    Object.defineProperty(c, "Console", hidden);
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
    // One descriptor for every member, as the console's above.
    const descriptor = { value: undefined, writable: false, configurable: true, enumerable: false };
    const data = (target, key, value, writable) => {
        descriptor.value = value;
        descriptor.writable = writable === true;
        Object.defineProperty(target, key, descriptor);
        descriptor.value = undefined;
    };
    Object.defineProperty(String.prototype, "length", { value: 0, writable: false,
                                                        configurable: false, enumerable: false });
    // Preserve the low-level native algorithms before the public TypeScript protocol methods
    // are installed. RegExp's @@ methods use these directly to avoid redispatching to themselves.
    globalThis.__kilnRawStringProtocols = {
        match: String.prototype.match,
        matchAll: String.prototype.matchAll,
        replace: String.prototype.replace,
        replaceAll: String.prototype.replaceAll,
        search: String.prototype.search,
        split: String.prototype.split,
    };
    const intrinsicApply = Reflect.apply;
    const rawStringProtocols = globalThis.__kilnRawStringProtocols;
    data(ArrayBuffer.prototype, Symbol.toStringTag, "ArrayBuffer");
    data(DataView.prototype, Symbol.toStringTag, "DataView");
    const methods = {
        [Symbol.match](string) { return intrinsicApply(rawStringProtocols.match, string, [this]); },
        [Symbol.matchAll](string) { return intrinsicApply(rawStringProtocols.matchAll, string, [this]); },
        [Symbol.replace](string, replacement) { return intrinsicApply(rawStringProtocols.replace, string, [this, replacement]); },
        [Symbol.search](string) { return intrinsicApply(rawStringProtocols.search, string, [this]); },
        [Symbol.split](string, limit) { return intrinsicApply(rawStringProtocols.split, string, [this, limit]); },
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
            if (target === null || (typeof target !== "object" && typeof target !== "function")) {
                throw new TypeError("Function has non-object prototype in instanceof check");
            }
            let current = Object.getPrototypeOf(value);
            while (current !== null) {
                if (current === target) return true;
                current = Object.getPrototypeOf(current);
            }
            return false;
        },
    };
    Object.defineProperty(Function.prototype, Symbol.hasInstance, {
        value: hasInstance[Symbol.hasInstance], writable: false, configurable: false, enumerable: false,
    });
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
    // One descriptor and an index loop: a literal and an iterator result per member were
    // garbage in every realm's bootstrap (realms issue 08).
    const names = Object.keys(methods);
    const hidden = { value: undefined, writable: true, configurable: true, enumerable: false };
    for (let i = 0; i < names.length; i++) {
        hidden.value = methods[names[i]];
        Object.defineProperty(Set.prototype, names[i], hidden);
    }
}

// `Object.groupBy` and `Map.groupBy` (ES2024), over the iteration protocol; both refused by
// name natively until 2026-09-19. Defined into the constructors' stores, which every read of a
// built-in consults first.
{
    const hidden = { value: undefined, writable: true, configurable: true, enumerable: false };
    const data = (target, key, value) => {
        hidden.value = value;
        Object.defineProperty(target, key, hidden);
        hidden.value = undefined;
    };
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

/// Non-filesystem base reserved for the realm-wide `process.getBuiltinModule` loader. Unlike
/// source modules, this identity must never select an importer-scoped `createRequire` table.
const NATIVE_BUILTIN_LOADER_BASE: &str = "kiln:runtime-builtin-loader";

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
    instantiate: bool,
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
    let input = Digest::hash(&[input.as_bytes().as_slice(), &[u8::from(instantiate)]].concat());
    cached_emitted_module(
        cache,
        target,
        input,
        *graph_source_emitter::SOURCE_EMITTER_ID,
        || graph_source_emitter::emit(node, logical_path, &effective_policy, instantiate),
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
        Digest::hash(b"native-graph-emission-input-v6"),
        [
            input,
            prelude_header_digest(),
            Digest::hash(kiln_backend_c::C_MODULE_EMITTER_BUILD_ID.as_bytes()),
            emitter_id,
        ],
    );
    match cache.lookup(&cache_key)? {
        Lookup::Hit(artifact) => {
            let (
                program_hash,
                symbol,
                source,
                generic_only,
                chunks,
                parallel_workers,
                instantiation,
            ): (
                u64,
                String,
                String,
                bool,
                Vec<String>,
                Option<String>,
                Option<kiln_backend_c::CModuleUnit>,
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
                    instantiation,
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
                &module.instantiation,
            ))
            .expect("C module contains only strings and an integer");
            cache.put(&cache_key, &bytes)?;
            Ok((module, false))
        }
    }
}

#[cfg(test)]
pub(super) fn emit_native_graph_module_with_policy(
    node: &kiln_module_graph::ModuleNode,
    logical_path: &str,
    policy: &EmissionPolicy,
) -> Result<kiln_backend_c::CModule, NativeError> {
    emit_native_graph_module_with_policy_and_instantiation(node, logical_path, policy, false)
}

/// About this many bytes of C per unit of engine code emitted generic-only: the runtime bootstrap
/// and the builtin modules. Small enough that a cold build's units fill the cores; large enough
/// that the per-unit header does not dominate, and that a module this size stays one unit.
const GENERIC_BUILTIN_UNIT_BYTES: usize = 1 << 20;

pub(super) fn emit_native_graph_module_with_policy_and_instantiation(
    node: &kiln_module_graph::ModuleNode,
    logical_path: &str,
    policy: &EmissionPolicy,
    instantiate: bool,
) -> Result<kiln_backend_c::CModule, NativeError> {
    match node.source {
        ModuleSource::NodeBuiltin(_builtin) => {
            // Engine code, like the bootstrap: split by bytes so a large builtin (`kiln:gui` is
            // 1.9 MB of C) compiles as parallel units instead of being a cold build's critical
            // path. A builtin smaller than one unit renders as a single unit as before.
            let mut builtin_policy = policy.clone();
            if builtin_policy.unit_chunk_bytes.is_none() {
                builtin_policy.unit_chunk_bytes = Some(GENERIC_BUILTIN_UNIT_BYTES);
            }
            Ok(emit_module_with_logical_path_and_policy(
                &node.program,
                logical_path,
                builtin_policy,
            )?)
        }
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
        ModuleSource::Source => Ok(
            kiln_backend_c::emit_module_with_logical_path_and_policy_and_instantiation(
                &node.program,
                logical_path,
                policy.clone(),
                instantiate,
            )?,
        ),
    }
}

struct NativeBuiltinBootstrapProgram {
    program: Result<std::sync::Arc<kiln_ir::ModuleProgram>, Vec<kiln_ir::CompileDiagnostic>>,
    source_digest: Digest,
    program_digest: Digest,
    provider_capabilities: ProviderCapabilities,
}

type NativeBootstrapKey = (bool, bool, u32);
type NativeBootstrapCache = (
    BTreeMap<NativeBootstrapKey, std::sync::Arc<NativeBuiltinBootstrapProgram>>,
    std::collections::VecDeque<NativeBootstrapKey>,
);
static NATIVE_BOOTSTRAPS: OnceLock<std::sync::Mutex<NativeBootstrapCache>> = OnceLock::new();

/// `process.getBuiltinModule` for the native backend, written in TypeScript over the
/// `__kilnHostModuleCreateRequire` intrinsic (ADR 0005: builtins are TypeScript, not C).
/// The C runtime leaves the name absent on its process object; this snippet installs the
/// implementation when a process global exists and does not already provide one. The id gate
/// uses the resolver's canonical builtin list so the answer matches node: a builtin name, with
/// or without the `node:` prefix, returns the linked builtin's namespace; anything else
/// returns `undefined`; a non-string id throws a TypeError like node's `validateString`.
/// The filename handed to the create-require intrinsic is never used for relative resolution
/// here -- only `node:` specifiers are ever required through it. Its reserved non-filesystem
/// identity keeps it separate from importer-scoped tables owned by the runtime bootstrap.
fn native_get_builtin_module_bootstrap() -> String {
    let mut names: Vec<&str> = kiln_builtins::ts_builtin_specifiers()
        .into_iter()
        .filter_map(|specifier| specifier.strip_prefix("node:"))
        .collect();
    names.sort_unstable();
    names.dedup();
    let list = names
        .iter()
        .map(|name| format!("{name:?}"))
        .collect::<Vec<_>>()
        .join(",");
    format!(
        r#"
if (typeof globalThis.process !== "undefined" && globalThis.process !== null && !globalThis.process.getBuiltinModule) {{
    const __kilnBuiltinModuleNames = new Set([{list}]);
    const __kilnBuiltinCreateRequire = process.__kilnNativeModules.__kilnHostModuleCreateRequire;
    let __kilnBuiltinRequire;
    globalThis.process.getBuiltinModule = function getBuiltinModule(id) {{
        if (typeof id !== "string") {{
            throw new TypeError('The "id" argument must be of type string');
        }}
        const name = id.startsWith("node:") ? id.slice(5) : id;
        if (!__kilnBuiltinModuleNames.has(name)) return undefined;
        if (__kilnBuiltinRequire === undefined) {{
            __kilnBuiltinRequire =
                __kilnBuiltinCreateRequire({builtin_loader_base:?});
        }}
        return __kilnBuiltinRequire("node:" + name);
    }};
}}
"#,
        builtin_loader_base = NATIVE_BUILTIN_LOADER_BASE,
    )
}

/// `import.meta` for native graph modules, written in TypeScript over the captured source
/// identity. The entry's URL must be the source URL used during graph planning: relative dynamic
/// imports and `new URL(..., import.meta.url)` resolve from that module, not from the generated
/// executable's location. `process.execPath` remains the executable path independently.
fn native_import_meta_bootstrap() -> String {
    r#"
const __kilnImportMetaModules = globalThis.process.__kilnNativeModules;
if (typeof __kilnImportMetaModules.__kilnNativeImportMeta === "undefined") {
    const __kilnEncodeURIComponent = encodeURIComponent;
    const __kilnApply = Reflect.apply;
    const __kilnReplace = String.prototype.replace;
    const __kilnPathEscapes = {"%2F":"/", "%24":"$", "%26":"&", "%2B":"+", "%2C":",", "%3B":";", "%3D":"=", "%3A":":", "%40":"@"};
    const __kilnPathEscapePattern = /%(2F|24|26|2B|2C|3B|3D|3A|40)/g;
    function __kilnPathToFileURL(path) {
        const encoded = __kilnEncodeURIComponent(path);
        const pathEncoded = __kilnApply(__kilnReplace, encoded, [__kilnPathEscapePattern, escape => __kilnPathEscapes[escape]]);
        return "file://" + pathEncoded;
    }
    __kilnImportMetaModules.__kilnNativeImportMeta = function __kilnNativeImportMeta(isEntry, sourceFilename) {
        return {
            // Encode the raw filesystem path before assembling its URL. URL construction from
            // `file://` + path treats `#` and `?` as URL syntax instead of path characters.
            url: __kilnPathToFileURL(sourceFilename),
            filename: sourceFilename,
            dirname: sourceFilename.slice(0, sourceFilename.lastIndexOf("/")),
            main: isEntry,
        };
    };
}
"#
    .to_string()
}

fn compile_native_builtin_bootstrap(
    node: bool,
    retain_url: bool,
    families: u32,
) -> NativeBuiltinBootstrapProgram {
    const LOGICAL_PATH: &str = "@kiln/runtime-bootstrap.ts";
    let url = if retain_url || node {
        kiln_builtins::URL_BOOTSTRAP_SOURCE
    } else {
        ""
    };
    let common_ts_builtins = kiln_builtins::common_bootstrap_source_selected(families);
    let get_builtin_module = native_get_builtin_module_bootstrap();
    let import_meta = native_import_meta_bootstrap();
    // WHATWG web standards (fetch, Headers, Request, Response, etc.) are global,
    // not gated behind the node adapter: the HTTP host capability
    // (process.__kilnNativeModules.http.request) exists in both modes.
    let web = kiln_builtins::WEB_BOOTSTRAP_SOURCE;
    let source = if node {
        format!(
            "{NATIVE_BUILTIN_BOOTSTRAP_SOURCE}\n{common_ts_builtins}\n{url}\n{web}\n{get_builtin_module}\n{import_meta}"
        )
    } else {
        format!(
            "{NATIVE_BUILTIN_BOOTSTRAP_SOURCE}\n{common_ts_builtins}\n{url}\n{web}\ndelete globalThis.__kilnBufferToString;\n{get_builtin_module}\n{import_meta}"
        )
    };
    let source_digest = Digest::hash(source.as_bytes());
    let hir = kiln_frontend_oxc::parse(LOGICAL_PATH, &source);
    let program = kiln_ir::compile_module(&hir).map(std::sync::Arc::new);
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

/// The unconditional web-global bootstrap uses Math/Number in its stream, buffer, ArrayBuffer,
/// and typed-array implementations. Preserve that dispatch group even when the user graph does
/// not directly demand those globals; family pruning and method-group pruning have separate
/// liveness inputs.
pub(super) const NATIVE_WEB_BOOTSTRAP_METHOD_GROUPS: u32 = kiln_graph::method_groups::MATH_NUMBER;

/// A native executable with no open-world access constructs only the intrinsic families it can
/// reach. Unknown computed/global access keeps the full surface. The remaining conditions are
/// deliberately conservative: a constructor or prototype reached through a stored value, for
/// example, defeats a direct-global-only proof. Each retained family still runs its ordinary
/// TypeScript bootstrap, so its identity and descriptors have exactly the full-build semantics.
pub(super) fn native_bootstrap_families(
    members: &kiln_graph::DemandedMembers,
    node: bool,
    group_mask: u32,
) -> u32 {
    use kiln_builtins::native_bootstrap_family as family;
    let full_groups = kiln_graph::CORE_METHOD_GROUPS | kiln_graph::PRUNABLE_METHOD_GROUPS;
    if node
        || members.has_open_world
        || members.has_computed_access
        || members.builtins.contains("GlobalThis")
        || members.globals.contains("globalThis")
        || members
            .names
            .iter()
            .any(|name| matches!(name.as_str(), "constructor" | "__proto__" | "prototype"))
        || group_mask == full_groups
    {
        return family::ALL;
    }
    let names = |values: &[&str]| values.iter().any(|name| members.names.contains(*name));
    let builtin = |values: &[&str]| {
        values
            .iter()
            .any(|name| members.builtins.contains(*name) || members.globals.contains(*name))
    };
    // WEB_BOOTSTRAP_SOURCE is emitted unconditionally and its MessagePort implementation
    // captures structuredClone; structuredClone uses ArrayBuffer's transfer/detach support.
    // These are bootstrap dependencies rather than user-code demands, so seed them here
    // instead of retaining every native builtin family.
    let mut selected = family::WEB_BOOTSTRAP_DEPENDENCIES;
    if builtin(&["JSON"]) || names(&["stringify"]) {
        selected |= family::JSON;
    }
    if names(&["toLocaleLowerCase", "toLocaleUpperCase"]) {
        selected |= family::STRING_LOCALE;
    }
    if builtin(&["Intl"]) || names(&["DateTimeFormat", "formatToParts", "resolvedOptions"]) {
        selected |= family::INTL_DATETIME;
    }
    if builtin(&["Date", "Intl"]) || names(&["getTime", "setTime", "toISOString", "toUTCString"]) {
        selected |= family::DATE;
    }
    let mut typed_array = builtin(&[
        "Int8Array",
        "Uint8Array",
        "Uint8ClampedArray",
        "Int16Array",
        "Uint16Array",
        "Int32Array",
        "Uint32Array",
        "Float16Array",
        "Float32Array",
        "Float64Array",
        "BigInt64Array",
        "BigUint64Array",
        "DataView",
        "TextEncoder",
        "TextDecoder",
        "Buffer",
    ]) || names(&[
        "BYTES_PER_ELEMENT",
        "byteOffset",
        "buffer",
        "subarray",
        "copyWithin",
    ]);
    // `Math.f16round` delegates conversion to the existing Float16Array element store, so its
    // bootstrap dependency is the typed-array runtime. Keep ordinary statically named Math
    // operations (for example Math.floor) free of that chunk. Since member demand is aggregated
    // across receivers, preserve the augmentation when Math and an operation that can observe
    // its own-property surface are both retained. Open-world and computed access already took
    // the full-family path above.
    let observes_own_properties = names(&[
        "assign",
        "defineProperty",
        "entries",
        "freeze",
        "getOwnPropertyDescriptor",
        "getOwnPropertyDescriptors",
        "getOwnPropertyNames",
        "getOwnPropertySymbols",
        "hasOwn",
        "hasOwnProperty",
        "isExtensible",
        "isFrozen",
        "isSealed",
        "keys",
        "ownKeys",
        "preventExtensions",
        "propertyIsEnumerable",
        "seal",
        "values",
    ]);
    if names(&["f16round"]) || (builtin(&["Math"]) && observes_own_properties) {
        selected |= family::NUMERIC_MATH;
        typed_array = true;
    }
    if builtin(&["BigInt"]) || names(&["asIntN", "asUintN"]) {
        selected |= family::NUMERIC_BIGINT;
    }
    if typed_array {
        selected |= family::TYPED_ARRAY;
    }
    if builtin(&[
        "Symbol",
        "DisposableStack",
        "AsyncDisposableStack",
        "SuppressedError",
    ]) || names(&["dispose", "asyncDispose", "defer", "adopt", "move"])
    {
        selected |= family::DISPOSABLE;
    }
    if builtin(&["WeakRef", "FinalizationRegistry"]) {
        selected |= family::WEAK_REFS;
    }
    if builtin(&["Atomics", "SharedArrayBuffer"]) {
        selected |= family::ATOMICS;
    }
    if builtin(&["structuredClone"]) || members.globals.contains("structuredClone") {
        selected |= family::STRUCTURED_CLONE;
    }
    // The WHATWG web bootstrap (fetch, streams, events) is included unconditionally and
    // web_events_streams.ts calls __kilnStructuredClone, so the family must always be present.
    selected |= family::STRUCTURED_CLONE;
    if builtin(&["Map", "Set"]) || names(&["getOrInsert", "getOrInsertComputed"]) {
        selected |= family::MAP_UPSERT;
    }
    if builtin(&["Promise"]) || names(&["try", "allKeyed", "allSettledKeyed"]) {
        selected |= family::PROMISE;
    }
    if builtin(&["Object", "Reflect"])
        || names(&[
            "seal",
            "isSealed",
            "isExtensible",
            "preventExtensions",
            "setPrototypeOf",
        ])
    {
        selected |= family::OBJECT_REFLECT;
    }
    if typed_array
        || selected & (family::ATOMICS | family::WEAK_REFS | family::STRUCTURED_CLONE) != 0
        || builtin(&["ArrayBuffer", "SharedArrayBuffer"])
    {
        selected |= family::ARRAY_BUFFER;
    }
    selected
}

pub(super) fn native_builtin_bootstrap_module(
    cache: &ArtifactCache,
    target: &str,
    policy: &EmissionPolicy,
    node: bool,
    retain_url: bool,
    families: u32,
    specialized_unit_bytes: Option<usize>,
) -> Result<
    (
        kiln_backend_c::CModule,
        std::sync::Arc<kiln_ir::ModuleProgram>,
        ProviderCapabilities,
        bool,
    ),
    NativeError,
> {
    const LOGICAL_PATH: &str = "@kiln/runtime-bootstrap.ts";
    // Key source compilation by the complete surface selection. A long-lived build daemon can
    // see many of the 10-bit family masks, so retain only the latest 16 variants. An Arc held
    // through emission keeps an evicted program alive until that build has finished using it.
    let key = (node, retain_url, families);
    let programs = NATIVE_BOOTSTRAPS.get_or_init(|| {
        std::sync::Mutex::new((BTreeMap::new(), std::collections::VecDeque::new()))
    });
    let compiled = {
        let mut cache = programs.lock().expect("bootstrap cache lock");
        if let Some(program) = cache.0.get(&key) {
            std::sync::Arc::clone(program)
        } else {
            let program =
                std::sync::Arc::new(compile_native_builtin_bootstrap(node, retain_url, families));
            cache.0.insert(key, std::sync::Arc::clone(&program));
            cache.1.push_back(key);
            if cache.1.len() > 16 {
                let oldest = cache.1.pop_front().expect("nonempty bootstrap cache");
                cache.0.remove(&oldest);
            }
            program
        }
    };
    let program = std::sync::Arc::clone(compiled.program.as_ref().map_err(|diagnostics| {
        NativeError::Graph(ModuleGraphError::Compile {
            path: PathBuf::from(LOGICAL_PATH),
            diagnostics: diagnostics.clone(),
        })
    })?);
    // The bootstrap is code every binary carries -- the console methods, the Set algebra, the
    // URL class -- and emitted like hot user code, with a fast clone and a typed clone per
    // function, it once put 0.9 MB on a hello world; so by default it is emitted generic-only.
    // `specialized_unit_bytes` (a release build that opted in, issue 17) emits it specialized
    // instead -- the `%TypedArray%` and `Date` methods are hot loops, 4-12x slower generic at
    // -O0 -- with its C split into units of about this many bytes, so the one-time -O2 compile
    // runs as parallel units rather than one minute-long one.
    //
    // The generic-only default is split by bytes too. Split by function count it came out as one
    // unit of 6.9 MB (a hello world; 8.9 MB plus 0.9 MB for a GUI program) that took clang 11.8 s
    // at -O0 on its own while every other unit finished in under 2 s: the cold compile of *every*
    // program was that one unit, 11.5-16.8 s of a 12-17 s build. Its own text -- body, key table,
    // constants -- is only 75 KB; the rest is 774 or more functions, so units of about a megabyte
    // compile side by side on the ordinary pool instead.
    let mut bootstrap_policy = policy.clone();
    bootstrap_policy.generic_only = specialized_unit_bytes.is_none();
    bootstrap_policy.unit_chunk_bytes =
        Some(specialized_unit_bytes.unwrap_or(GENERIC_BUILTIN_UNIT_BYTES));
    if !bootstrap_policy.decoupled_prelude {
        let module =
            emit_module_with_logical_path_and_policy(&program, LOGICAL_PATH, bootstrap_policy)?;
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
            bootstrap_emitter::emit(&program, LOGICAL_PATH, &bootstrap_policy)
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
            bytes.push(u8::from(dependency.resolve_only));
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
        live_cell: bool,
    },
    Namespace {
        provider_index: usize,
    },
    CommonJsNamed {
        provider_index: usize,
        property: String,
    },
}

/// ESM source modules requiring live bindings need an instantiation entrypoint. Keep the cycle
/// decision independent of source order: a self-edge is a cycle too.
pub(super) fn preinstantiated_module_paths(
    graph: &ModuleGraph,
) -> std::collections::HashSet<PathBuf> {
    // Modules participating in a cycle and their ESM dependencies are instantiated before any
    // evaluation. Modules whose ESM namespace is observed also need preallocated export cells:
    // a namespace keeps live views of exports even when its provider is acyclic. The dependency
    // closure supplies live cells for imports captured by closures created in these modules.
    let nodes = graph
        .modules()
        .filter(|node| node.source == ModuleSource::Source)
        .collect::<Vec<_>>();
    let indices = nodes
        .iter()
        .enumerate()
        .map(|(index, node)| (node.path.clone(), index))
        .collect::<std::collections::HashMap<_, _>>();
    let mut edges = vec![Vec::new(); nodes.len()];
    let mut reverse = vec![Vec::new(); nodes.len()];
    for (index, node) in nodes.iter().enumerate() {
        for dependency in &node.dependencies {
            if let Some(&target) = indices.get(&dependency.target) {
                edges[index].push(target);
                reverse[target].push(index);
            }
        }
    }
    // Iterative Kosaraju keeps stack use bounded even for a deeply nested package graph.
    let mut seen = vec![false; nodes.len()];
    let mut finish_order = Vec::with_capacity(nodes.len());
    for start in 0..nodes.len() {
        if seen[start] {
            continue;
        }
        seen[start] = true;
        let mut stack = vec![(start, 0usize)];
        while let Some((index, next_edge)) = stack.last_mut() {
            if *next_edge < edges[*index].len() {
                let target = edges[*index][*next_edge];
                *next_edge += 1;
                if !seen[target] {
                    seen[target] = true;
                    stack.push((target, 0));
                }
            } else {
                finish_order.push(*index);
                stack.pop();
            }
        }
    }
    let mut component_seen = vec![false; nodes.len()];
    let mut cyclic = std::collections::HashSet::new();
    for start in finish_order.into_iter().rev() {
        if component_seen[start] {
            continue;
        }
        component_seen[start] = true;
        let mut members = Vec::new();
        let mut stack = vec![start];
        while let Some(index) = stack.pop() {
            members.push(index);
            for &source in &reverse[index] {
                if !component_seen[source] {
                    component_seen[source] = true;
                    stack.push(source);
                }
            }
        }
        if members.len() > 1 || edges[start].contains(&start) {
            cyclic.extend(members.into_iter().map(|index| nodes[index].path.clone()));
        }
    }
    // Namespace providers cannot publish snapshots. Namespace imports and CommonJS `require`
    // of ESM both expose namespace objects; dynamic imports can target any ESM module retained
    // by the graph, so preserve cells for all of them when one is present.
    let mut namespace_providers = std::collections::HashSet::new();
    let has_dynamic_import = graph
        .modules()
        .any(|node| node.program.program.has_dynamic_import());
    if has_dynamic_import {
        namespace_providers.extend(nodes.iter().map(|node| node.path.clone()));
    }
    for node in graph.modules() {
        for import in &node.program.imports {
            if import.imported != kiln_ir::NAMESPACE_IMPORT {
                continue;
            }
            let internal_star_table = node.program.exports.iter().any(|export| {
                export.local_slot == import.local_slot
                    && export.exported.starts_with(kiln_ir::STAR_REEXPORT_PREFIX)
            });
            if internal_star_table {
                continue;
            }
            if let Some(dependency) = node
                .dependencies
                .iter()
                .find(|dependency| dependency.specifier == import.source)
                && graph
                    .module(&dependency.target)
                    .is_some_and(|provider| provider.source == ModuleSource::Source)
            {
                namespace_providers.insert(dependency.target.clone());
            }
        }
        // CommonJS `require(esm)` returns the provider's namespace object as well.
        if node.source == ModuleSource::CommonJs {
            for dependency in &node.dependencies {
                if graph
                    .module(&dependency.target)
                    .is_some_and(|provider| provider.source == ModuleSource::Source)
                {
                    namespace_providers.insert(dependency.target.clone());
                }
            }
        }
        for dependency in &node.runtime_dependencies {
            if dependency.resolve_only {
                continue;
            }
            if graph
                .module(&dependency.target)
                .is_some_and(|provider| provider.source == ModuleSource::Source)
            {
                namespace_providers.insert(dependency.target.clone());
            }
        }
    }
    for path in graph.declared_includes() {
        if graph
            .module(path)
            .is_some_and(|provider| provider.source == ModuleSource::Source)
        {
            namespace_providers.insert(path.clone());
        }
    }
    let mut pending = cyclic
        .iter()
        .chain(namespace_providers.iter())
        .cloned()
        .collect::<Vec<_>>();
    cyclic.extend(namespace_providers);
    while let Some(path) = pending.pop() {
        let Some(node) = graph.module(&path) else {
            continue;
        };
        for dependency in &node.dependencies {
            if graph
                .module(&dependency.target)
                .is_some_and(|target| target.source == ModuleSource::Source)
                && cyclic.insert(dependency.target.clone())
            {
                pending.push(dependency.target.clone());
            }
        }
    }
    cyclic
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum NativeNamespaceValue {
    Binding {
        provider_index: usize,
        export_index: usize,
        live_cell: bool,
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
        if provider_index >= importer_index && modules[provider_index].1.instantiation.is_none() {
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
            if provider_index >= importer_index && modules[provider_index].1.instantiation.is_none()
            {
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
            if provider_index >= importer_index && modules[provider_index].1.instantiation.is_none()
            {
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
    /* A forward binding is available only when its source module participates in the graph's
     * instantiation phase, which preallocates export cells before any body runs. */
    let importer_index = *indices.get(&node.path).ok_or_else(|| {
        NativeError::Graph(ModuleGraphError::InternalMissingModule {
            path: node.path.clone(),
        })
    })?;
    let live_cell = modules[provider_index].1.instantiation.is_some();
    if provider_index >= importer_index && !live_cell {
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
        live_cell,
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
        // Synchronous `require(esm)` returns the module namespace on Node versions that support
        // it. The namespace must exist even though this edge is a runtime require rather than an
        // ESM namespace import.
        for dependency in &node.dependencies {
            if node.source != ModuleSource::CommonJs {
                continue;
            }
            let Some(&provider_index) = indices.get(&dependency.target) else {
                continue;
            };
            if modules[provider_index].0.source == ModuleSource::Source {
                required[provider_index] = true;
            }
        }
        for dependency in &node.runtime_dependencies {
            if dependency.resolve_only {
                continue;
            }
            let Some(&provider_index) = indices.get(&dependency.target) else {
                continue;
            };
            if modules[provider_index].0.source == ModuleSource::Source {
                required[provider_index] = true;
            }
        }
    }
    for path in graph.declared_includes() {
        if let Some(&provider_index) = indices.get(path)
            && modules[provider_index].0.source == ModuleSource::Source
        {
            required[provider_index] = true;
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
                            live_cell: modules[provider_index].1.instantiation.is_some(),
                        }
                    }
                    Some(ResolvedExport::CommonJsNamed { module, .. }) => {
                        let (provider_index, export_index) =
                            graph_export_slot(modules, indices, &node.path, module, 0, &name)?;
                        NativeNamespaceValue::Binding {
                            provider_index,
                            export_index,
                            live_cell: modules[provider_index].1.instantiation.is_some(),
                        }
                    }
                    Some(ResolvedExport::Namespace { module }) => {
                        let provider_index = *indices.get(module).ok_or_else(|| {
                            NativeError::Graph(ModuleGraphError::InternalMissingModule {
                                path: module.clone(),
                            })
                        })?;
                        if provider_index >= module_index
                            && modules[provider_index].1.instantiation.is_none()
                        {
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

/// Whether synchronously requiring this ESM module would encounter top-level await anywhere in
/// its static ESM dependency graph. `require(esm)` must reject before evaluation, including when
/// the awaited module is several imports away.
fn module_transitive_top_level_await(
    modules: &[(&kiln_module_graph::ModuleNode, kiln_backend_c::CModule)],
    indices: &std::collections::HashMap<PathBuf, usize>,
) -> Vec<bool> {
    // Reverse the static ESM edges and spread asynchrony backwards from top-level Await nodes.
    // This is linear in the graph size and handles SCCs without an order-sensitive DFS cache.
    let mut importers = vec![Vec::new(); modules.len()];
    let mut asynchronous = vec![false; modules.len()];
    for index in 0..modules.len() {
        let node = modules[index].0;
        if node.source != ModuleSource::Source {
            continue;
        }
        asynchronous[index] = node
            .program
            .program
            .code
            .iter()
            .any(|instruction| matches!(instruction, kiln_ir::Instruction::Await));
        let static_specifiers = node
            .program
            .dependencies
            .iter()
            .map(String::as_str)
            .collect::<std::collections::HashSet<_>>();
        for dependency in &node.dependencies {
            if !static_specifiers.contains(dependency.specifier.as_str()) {
                continue;
            }
            let Some(&provider) = indices.get(&dependency.target) else {
                continue;
            };
            if modules[provider].0.source == ModuleSource::Source {
                importers[provider].push(index);
            }
        }
    }
    let mut pending = asynchronous
        .iter()
        .enumerate()
        .filter_map(|(index, async_module)| async_module.then_some(index))
        .collect::<std::collections::VecDeque<_>>();
    while let Some(provider) = pending.pop_front() {
        for importer in &importers[provider] {
            if !asynchronous[*importer] {
                asynchronous[*importer] = true;
                pending.push_back(*importer);
            }
        }
    }
    asynchronous
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
extern void kiln_c99_gc_enter_job(void);
extern void kiln_c99_gc_leave_job(void);
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
        if graph.declared_includes().contains(&node.path) {
            resolutions.insert((node.path.to_string_lossy().into_owned(), modules.len()));
            if let Ok(url) = Url::from_file_path(&node.path) {
                resolutions.insert((url.to_string(), modules.len()));
            }
        }
        // `spawnActor({ module })` includes its statically named target as a lazy graph node.
        // The actor worker loads that node by the absolute URL passed from the spawning realm;
        // it may not load arbitrary files missing from the closed graph.
        if matches!(node.source, ModuleSource::Source | ModuleSource::CommonJs)
            && let Some(actor_worker) = modules.iter().position(|(candidate, _)| {
                candidate.source == ModuleSource::NodeBuiltin(NodeBuiltin::ActorsWorker)
            })
        {
            resolutions.insert((node.path.to_string_lossy().into_owned(), actor_worker));
            if let Ok(url) = Url::from_file_path(&node.path) {
                resolutions.insert((url.to_string(), actor_worker));
            }
        }
        for (spec, importer_index) in resolutions {
            let c_spec = native_c_string(&spec);
            let len = spec.len();
            let importer_guard = if importer_index == modules.len() {
                "1".to_owned()
            } else {
                format!(
                    "importer != NULL && strcmp(importer, {}) == 0",
                    native_c_string(&modules[importer_index].1.symbol)
                )
            };
            let attribute_guard = if matches!(node.source, ModuleSource::Asset) {
                " && attribute_type == 1"
            } else {
                " && attribute_type == 0"
            };
            writeln!(
                output,
                "    if ({importer_guard}{attribute_guard} && kiln_c99_string_equals_ascii(specifier, {c_spec}, (size_t){len})) {{"
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
    write_actor_module_not_linked(output, modules);
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

/// The actor worker's answer for a module this build did not link (ADR 0072).
///
/// The worker reaches every linked module by absolute path or file URL (above), so a name that
/// falls through to here is one `spawnActor` was given at run time that the compiler could not
/// see. Say so, naming the module and why, rather than the generic dynamic-import refusal: the
/// spawn rejects with this `TypeError`.
fn write_actor_module_not_linked(
    output: &mut String,
    modules: &[(&kiln_module_graph::ModuleNode, kiln_backend_c::CModule)],
) {
    let Some(actor_worker) = modules.iter().position(|(candidate, _)| {
        candidate.source == ModuleSource::NodeBuiltin(NodeBuiltin::ActorsWorker)
    }) else {
        return;
    };
    const PREFIX: &str = "actor module `";
    const SUFFIX: &str = "` was not linked into this build: a native build links only the actor \
        modules a spawnActor call names at compile time (a string literal, new URL(literal, \
        import.meta.url), or a const holding one); this name was computed at run time";
    writeln!(
        output,
        "    if (importer != NULL && strcmp(importer, {importer}) == 0) {{\n        kiln_value message = kiln_c99_concat(kiln_c99_concat(kiln_c99_copy_text({prefix}, (size_t){prefix_len}), specifier), kiln_c99_copy_text({suffix}, (size_t){suffix_len}));\n        kiln_value err = kiln_c99_undefined();\n        (void)kiln_c99_new_type_error(&err, \"actor module was not linked into this build\");\n        (void)kiln_c99_put(host, err, \"message\", 7, message, depth, thrown);\n        return kiln_c99_invoke_method(host, promise, KILN_C99_METHOD_PROMISE_REJECT, kiln_c99_undefined(), &err, 1, depth, thrown);\n    }}",
        importer = native_c_string(&modules[actor_worker].1.symbol),
        prefix = native_c_string(PREFIX),
        prefix_len = PREFIX.len(),
        suffix = native_c_string(SUFFIX),
        suffix_len = SUFFIX.len(),
    )
    .unwrap();
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
        if graph.declared_includes().contains(&node.path) {
            resolutions.insert((node.path.to_string_lossy().into_owned(), modules.len()));
            if let Ok(url) = Url::from_file_path(&node.path) {
                resolutions.insert((url.to_string(), modules.len()));
            }
        }
        for (spec, importer_index) in resolutions {
            let c_spec = native_c_string(&spec);
            let len = spec.len();
            let importer_guard = if importer_index == modules.len() {
                "1".to_owned()
            } else {
                format!(
                    "importer != NULL && strcmp(importer, {}) == 0",
                    native_c_string(&modules[importer_index].1.symbol)
                )
            };
            let attribute_guard = if matches!(node.source, ModuleSource::Asset) {
                " && attribute_type == 1"
            } else {
                " && attribute_type == 0"
            };
            writeln!(
                output,
                "        if ({importer_guard}{attribute_guard} && kiln_c99_string_equals_ascii(specifier, {c_spec}, (size_t){len})) {{"
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
        "    status = {module_symbol}_commonjs_wrapper(kiln_imports_{index}, (size_t)5, {}, (size_t){}, {}, (size_t){}, kiln_commonjs_require_{index}, kiln_commonjs_resolve_{index}, graph, kiln_graph_gc_visit_roots, graph);\n    if (status != 0) {{ *result = kiln_graph_diagnostic(\"commonjs-wrapper\", {}); return status; }}\n    kiln_exports_{index}[0] = kiln_imports_{index}[0];\n    kiln_commonjs_{index}.module = {module_symbol};\n    kiln_commonjs_{index}.host = host;\n    kiln_commonjs_{index}.imports = kiln_imports_{index};\n    kiln_commonjs_{index}.import_count = (size_t)5;\n    kiln_commonjs_{index}.exports = kiln_exports_{index};\n    kiln_commonjs_{index}.state = 0;",
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
        "    status = {module_symbol}_commonjs_wrapper(import_arrays[{index}], (size_t)5, {}, (size_t){}, {}, (size_t){}, kiln_commonjs_require_{index}, kiln_commonjs_resolve_{index}, commonjs_graph, commonjs_graph->visit_roots, commonjs_graph->root_context);\n    if (status != KILN_C99_OK) goto cleanup;\n    export_arrays[{index}][0] = import_arrays[{index}][0];\n    commonjs_modules[{index}] = (kiln_commonjs_context){{ {module_symbol}, proxy_host, import_arrays[{index}], (size_t)5, export_arrays[{index}], 0 }};",
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
        if !shared {
            writeln!(output, "    if (status == KILN_C99_OK) status = kiln_c99_object_set_external_owner({namespaces}[{module_index}], graph->host.module_owner);").unwrap();
        }
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
                live_cell: true,
            } if shared => format!("export_arrays[{provider_index}][{export_index}]"),
            NativeNamespaceValue::Binding {
                provider_index,
                export_index,
                live_cell: true,
            } => format!("kiln_cells_{provider_index}[{export_index}]"),
            NativeNamespaceValue::Binding {
                provider_index,
                export_index,
                ..
            } if shared => {
                format!("export_arrays[{provider_index}][{export_index}]")
            }
            NativeNamespaceValue::Binding {
                provider_index,
                export_index,
                ..
            } => {
                format!("kiln_exports_{provider_index}[{export_index}]")
            }
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
    if !shared {
        writeln!(output, "    if (status == KILN_C99_OK) status = kiln_c99_object_set_external_owner({namespaces}[{module_index}], graph->host.module_owner);").unwrap();
    }
}

/// The graph launcher's root function (ADR 0041): the export, import and namespace arrays it
/// links with, and the two result slots `main` reads after the realm is destroyed.
pub(super) fn render_graph_roots(
    output: &mut String,
    modules: &[(&kiln_module_graph::ModuleNode, kiln_backend_c::CModule)],
) {
    output.push_str(
        "static int kiln_graph_gc_visit_roots(void *context, kiln_gc_visit_root_fn visit, void *visit_context) {\n    kiln_graph_context *graph = (kiln_graph_context *)context;\n    size_t index;\n    if (graph == NULL) return 0;\n",
    );
    for (index, (node, _)) in modules.iter().enumerate() {
        let export_count = native_graph_export_count(node);
        if export_count != 0 {
            writeln!(output, "    for (index = 0; index < (size_t){export_count}; ++index) if (visit(visit_context, &kiln_exports_{index}[index], KILN_GC_VISIT_VALUE) != 0) return KILN_C99_HOST_ERROR;").unwrap();
            if modules[index].1.instantiation.is_some() {
                writeln!(output, "    for (index = 0; index < (size_t){export_count}; ++index) if (visit(visit_context, &kiln_cells_{index}[index], KILN_GC_VISIT_VALUE) != 0) return KILN_C99_HOST_ERROR;").unwrap();
            }
        }
        if !node.program.imports.is_empty() {
            writeln!(output, "    for (index = 0; index < (size_t){}; ++index) if (visit(visit_context, &kiln_imports_{index}[index], KILN_GC_VISIT_VALUE) != 0) return KILN_C99_HOST_ERROR;", node.program.imports.len()).unwrap();
        }
    }
    writeln!(output, "    for (index = 0; index < (size_t){}; ++index) if (visit(visit_context, &kiln_namespaces[index], KILN_GC_VISIT_VALUE) != 0) return KILN_C99_HOST_ERROR;", modules.len()).unwrap();
    if modules
        .iter()
        .any(|(node, _)| node.program.program.has_dynamic_import())
    {
        output.push_str("    if (visit(visit_context, &kiln_opentui_namespace, KILN_GC_VISIT_VALUE) != 0 ||\n        visit(visit_context, &kiln_node_ffi_namespace, KILN_GC_VISIT_VALUE) != 0) return KILN_C99_HOST_ERROR;\n");
    }
    writeln!(output, "    if (visit(visit_context, &graph->instance->result, KILN_GC_VISIT_VALUE) != 0 ||\n        visit(visit_context, &graph->instance->entry_result, KILN_GC_VISIT_VALUE) != 0) return KILN_C99_HOST_ERROR;\n    for (index = 0; index < (size_t){}; ++index) {{\n        if (visit(visit_context, &graph->instance->module_states[index].result, KILN_GC_VISIT_VALUE) != 0 ||\n            visit(visit_context, &graph->instance->module_states[index].active_promise, KILN_GC_VISIT_VALUE) != 0 ||\n            visit(visit_context, &graph->instance->module_states[index].completion_promise, KILN_GC_VISIT_VALUE) != 0 ||\n            visit(visit_context, &graph->instance->module_states[index].completion_resolve, KILN_GC_VISIT_VALUE) != 0 ||\n            visit(visit_context, &graph->instance->module_states[index].completion_reject, KILN_GC_VISIT_VALUE) != 0) return KILN_C99_HOST_ERROR;\n    }}\n    return 0;\n}}", modules.len()).unwrap();
    // Collection and ownership inspection enumerate precisely the same root slots.
    output.push_str("\nstatic int kiln_graph_gc_mark_root(void *heap, const void *pointer, uint32_t kind) {\n    if (kind != KILN_GC_VISIT_VALUE) return KILN_C99_INVALID_ARGUMENT;\n    (void)kiln_realm_gc_mark_value(heap, pointer);\n    return KILN_C99_OK;\n}\nstatic void kiln_graph_gc_roots(void *heap, void *context, int phase) {\n    kiln_graph_context *graph = (kiln_graph_context *)context;\n    kiln_value owner_root;\n    if (phase == KILN_GC_PHASE_RESET || phase == KILN_GC_PHASE_LANE_RESET || graph == NULL) return;\n    (void)kiln_graph_gc_visit_roots(graph, kiln_graph_gc_mark_root, heap);\n    if (graph->host.module_owner != NULL) { owner_root = (kiln_value){ .tag = 9 /* KILN_VALUE_OBJECT */, .flags = 0, .len = 0, .bits = (uint64_t)(uintptr_t)graph->host.module_owner }; (void)kiln_realm_gc_mark_value(heap, &owner_root); }\n}\n");
}

#[cfg(test)]
pub(super) fn graph_launcher_source(
    graph: &ModuleGraph,
    modules: &[(&kiln_module_graph::ModuleNode, kiln_backend_c::CModule)],
    policy: &NativeRealmPolicy,
) -> Result<String, NativeError> {
    graph_launcher_source_with_imports(graph, modules, policy, None, None, None, None, None, None)
}

/// Stop the promise checkpoint on a startup rejection. A thrown status would reject the
/// completion handler's derived promise instead, and waiting for quiescence hangs beside a live
/// server. The launcher maps this private status back to an uncaught throw at realm teardown.
pub(super) const FATAL_UNHANDLED_REJECTION: &str =
    r#"{ graph->instance->result = *value; return KILN_GRAPH_FATAL_REJECTION; }"#;

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
    let synchronous_require_async = module_transitive_top_level_await(modules, &indices);
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
/* A worker's standard streams are `fopencookie` streams (`node:worker_threads`), a GNU call. */
#if defined(__linux__) && !defined(_GNU_SOURCE)
#define _GNU_SOURCE
#endif
#include <locale.h>
#include <errno.h>
#include <stddef.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#ifndef _WIN32
#include <fcntl.h>
#include <poll.h>
#include <signal.h>
#include <stdio.h>
#include <time.h>
#include <sys/wait.h>
#include <unistd.h>
#include <pthread.h>
#include <strings.h>
#if defined(__linux__)
#include <sys/prctl.h>
#include <sys/resource.h>
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
    if let Some(workers) = policy.workers {
        // `kiln build --workers`: the count `kiln_cli_worker_count` falls back to. Emitted only
        // when recorded, so a binary built without the flag has the same launcher text as before.
        writeln!(
            output,
            "#define KILN_CLI_RECORDED_WORKERS \"{}\"\n",
            workers.setting()
        )
        .unwrap();
    }
    output.push_str(kiln_native_runtime::kiln_value_abi_declaration!());
    output.push_str(kiln_native_runtime::kiln_string_abi_declaration!());
    output.push_str(kiln_native_runtime::kiln_cli_host_state_declaration!());
    output.push_str(r#"/* The trusted builtin bootstrap installs methods onto built-ins; once it has run, the realm takes
 * those writes back out of the prelude's process-wide summary of modified built-ins, before any
 * user module can observe or mutate the realm (`kiln_c99_builtin_store_epoch`). */
extern void kiln_c99_builtin_bootstrap_done(void) __attribute__((visibility("hidden")));
extern int kiln_realm_private_window(int begin);

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
    void *module_context;
    void *module_owner;
} kiln_host;

static int kiln_c99_file_url_matches_path(const kiln_string *url, const char *path, size_t path_len) {
    static const char scheme[] = "file://";
    if (url == NULL || path == NULL || url->unit_len < sizeof(scheme) - 1) return 0;
    for (size_t i = 0; i < sizeof(scheme) - 1; ++i) {
        if (kiln_string_unit_at(url, i) != (uint8_t)scheme[i]) return 0;
    }
    size_t unit = 7;
    size_t byte = 0;
    while (unit < url->unit_len && byte < path_len) {
        uint16_t current = kiln_string_unit_at(url, unit++);
        unsigned char decoded;
        if (current == '%') {
            if (unit + 1 >= url->unit_len) return 0;
            uint16_t high = kiln_string_unit_at(url, unit++);
            uint16_t low = kiln_string_unit_at(url, unit++);
            int high_value = high >= '0' && high <= '9' ? (int)(high - '0') :
                             high >= 'A' && high <= 'F' ? (int)(high - 'A') + 10 :
                             high >= 'a' && high <= 'f' ? (int)(high - 'a') + 10 : -1;
            int low_value = low >= '0' && low <= '9' ? (int)(low - '0') :
                            low >= 'A' && low <= 'F' ? (int)(low - 'A') + 10 :
                            low >= 'a' && low <= 'f' ? (int)(low - 'a') + 10 : -1;
            if (high_value < 0 || low_value < 0) return 0;
            decoded = (unsigned char)((high_value << 4) | low_value);
        } else {
            if (current > 0x7f) return 0;
            decoded = (unsigned char)current;
        }
        if ((unsigned char)path[byte++] != decoded) return 0;
    }
    return unit == url->unit_len && byte == path_len;
}

static int kiln_c99_string_matches_utf8(const kiln_string *s, const char *path, size_t path_len) {
    if (s == NULL || (path == NULL && path_len != 0)) return 0;
    size_t unit = 0;
    size_t byte = 0;
    while (unit < s->unit_len && byte < path_len) {
        unsigned char b0 = (unsigned char)path[byte++];
        uint32_t cp = 0;
        if (b0 < 0x80) {
            cp = b0;
        } else if ((b0 & 0xe0) == 0xc0) {
            if (byte >= path_len) return 0;
            unsigned char b1 = (unsigned char)path[byte++];
            if ((b1 & 0xc0) != 0x80) return 0;
            cp = ((b0 & 0x1f) << 6) | (b1 & 0x3f);
        } else if ((b0 & 0xf0) == 0xe0) {
            if (byte + 1 >= path_len) return 0;
            unsigned char b1 = (unsigned char)path[byte++];
            unsigned char b2 = (unsigned char)path[byte++];
            if ((b1 & 0xc0) != 0x80 || (b2 & 0xc0) != 0x80) return 0;
            cp = ((b0 & 0x0f) << 12) | ((b1 & 0x3f) << 6) | (b2 & 0x3f);
        } else if ((b0 & 0xf8) == 0xf0) {
            if (byte + 2 >= path_len) return 0;
            unsigned char b1 = (unsigned char)path[byte++];
            unsigned char b2 = (unsigned char)path[byte++];
            unsigned char b3 = (unsigned char)path[byte++];
            if ((b1 & 0xc0) != 0x80 || (b2 & 0xc0) != 0x80 || (b3 & 0xc0) != 0x80) return 0;
            cp = ((b0 & 0x07) << 18) | ((b1 & 0x3f) << 12) | ((b2 & 0x3f) << 6) | (b3 & 0x3f);
        } else {
            return 0;
        }

        if (cp < 0x10000) {
            if (kiln_string_unit_at(s, unit++) != (uint16_t)cp) return 0;
        } else {
            uint32_t sub = cp - 0x10000;
            uint16_t high = (uint16_t)(0xd800 + (sub >> 10));
            uint16_t low = (uint16_t)(0xdc00 + (sub & 0x3ff));
            if (unit + 1 >= s->unit_len) return 0;
            if (kiln_string_unit_at(s, unit++) != high) return 0;
            if (kiln_string_unit_at(s, unit++) != low) return 0;
        }
    }
    return unit == s->unit_len && byte == path_len;
}

static int kiln_c99_filename_matches(kiln_value value, const char *path, size_t path_len) {
    if (path == NULL || value.tag != KILN_VALUE_STRING || (value.len != 0 && value.ptr == NULL)) return 0;
    const kiln_string *s = (const kiln_string *)value.ptr;
    if (s == NULL) return 0;
    if (s->unit_len == path_len && (path_len == 0 || kiln_string_equals_ascii(s, path, path_len))) return 1;
    if (s->unit_len >= 7 && kiln_string_starts_ascii(s, "file://", 7))
        return kiln_c99_file_url_matches_path(s, path, path_len);
    return kiln_string_equals_utf8(s, path, path_len);
}

typedef int (*kiln_graph_module_fn)(kiln_host *host, kiln_value *result,
                                    const kiln_value *imports, size_t import_count,
                                    kiln_value *exports, size_t export_count);
typedef int (*kiln_commonjs_require_fn)(void *context, kiln_host *host, kiln_value specifier,
                                       kiln_value *result, kiln_value *thrown);
typedef int (*kiln_commonjs_resolve_fn)(void *context, kiln_host *host, kiln_value specifier,
                                        kiln_value *result, kiln_value *thrown);
typedef int (*kiln_realm_entry_fn)(void *context);

extern int kiln_cli_host_call(void *context, uint16_t operation, uint16_t argc,
                              const kiln_value *arguments, kiln_value *result);
extern int kiln_cli_write_result(kiln_value value);
extern void kiln_cli_report_failure_in(int status, kiln_value value, const char *phase);
extern void kiln_cli_report_failure_with(int status, kiln_value value, const char *phase, const kiln_cli_host_state *host_state);
extern void kiln_cli_warn(const char *message, const kiln_cli_host_state *host_state);
extern kiln_value kiln_cli_describe_error(kiln_value value);
extern int kiln_c99_get(kiln_host *host, kiln_value target, const char *key, size_t key_len,
                        kiln_value *out, int depth, kiln_value *thrown);
extern int kiln_realm_create(uint64_t *out_realm);
extern int kiln_realm_enter(uint64_t realm, kiln_realm_entry_fn entry, void *context);
extern int kiln_realm_set_argv(uint64_t realm, uint64_t argc, const char *const *argv);
extern int kiln_realm_argv_at(uint64_t index, const uint8_t **bytes, uint64_t *length);
extern int kiln_realm_use_wall_clock(uint64_t realm);
extern int kiln_realm_drive(uint64_t realm, void *host, void *thrown);
extern int kiln_realm_drive_step(uint64_t realm, void *host, void *thrown,
                                  uint32_t *kind, uint64_t *delay_ns);
extern int kiln_realm_destroy(uint64_t realm);
extern int kiln_realm_run_pinned(uint64_t count, int (*run)(void *context, uint64_t index),
                                 void *context, int *statuses);
extern int kiln_realm_run_workers(uint64_t count, int (*run)(void *context, uint64_t index),
                                  void *context, int *statuses);
extern int kiln_realm_run_scheduled(uint64_t count, uint64_t lanes, int deterministic,
                                    void *(*begin)(void *context, uint64_t index),
                                    int (*advance)(void *session, uint64_t *delay_ns),
                                    int (*finish)(void *session), void *context, int *statuses);
extern int kiln_realm_run_migrating(uint64_t count, uint64_t lanes, int deterministic,
                                    void *(*begin)(void *context, uint64_t index),
                                    uint64_t (*identify)(void *session),
                                    int (*advance)(void *session, uint64_t *delay_ns),
                                    int (*finish)(void *session), void *context, int *statuses);
extern int kiln_realm_set_graph_control(int (*control)(uint32_t kind, uint64_t first,
                                                       uint64_t second, uint64_t third));
extern int kiln_c99_graph_paused_control(uint32_t kind, uint64_t first,
                                         uint64_t second, uint64_t third);
/* `KILN_WORKERS` thread mode: the worker index of the realm this thread runs (-1 on any other
 * thread, a `node:worker_threads` worker's included) and the count, which the realm's
 * `process.env` gets as KILN_WORKER_INDEX and KILN_WORKER_COUNT. */
static _Thread_local long kiln_cli_thread_worker = -1;
static long kiln_cli_thread_workers = 0;
extern void kiln_c99_realm_capture_output(FILE *output, FILE *error) __attribute__((visibility("hidden")));
/* `node:worker_threads` (realms issue 06): the runtime starts a worker's thread and calls the
 * launcher back there to run one module of this program as the worker's realm. */
typedef int (*kiln_worker_runner_fn)(const uint8_t *entry, uint64_t entry_len, int32_t eval,
                                     uint64_t argc, const char *const *argv);
extern int kiln_worker_set_runner(kiln_worker_runner_fn runner);
extern int kiln_worker_set_turn_runner(void *(*begin)(const uint8_t *, uint64_t, int32_t,
                                                     uint64_t, const char *const *),
                                        uint64_t (*identify)(void *),
                                        int (*advance)(void *, uint64_t *),
                                        int (*finish)(void *));
extern int kiln_worker_exit_code(int32_t *out);
/* The runtime's scalar port and signal operations; the thread-mode signal thread hands a signal to
 * the realms listening for it through op 20 (`kiln_runtime::signals::deliver`, realms issue 23). */
extern int kiln_worker_op(uint32_t op, uint64_t a, uint64_t *out);
extern int kiln_worker_stdio(uint32_t stream, const uint8_t *bytes, uint64_t len);
typedef struct kiln_worker_token {
    uint32_t kind;
    uint32_t flag;
    double number;
    const void *data;
    uint64_t len;
    void *handle;
} kiln_worker_token;
extern int kiln_worker_post(uint32_t op, uint64_t port, uint32_t kind, kiln_worker_token *tokens, uint64_t count);
extern int kiln_c99_worker_report_error(kiln_value error) __attribute__((visibility("hidden")));
extern int kiln_realm_kiln_extensions_on(void);
typedef void (*kiln_gc_roots_fn)(void *heap, void *context, int phase);
extern int kiln_realm_gc_register_roots(kiln_gc_roots_fn roots, void *context, uint32_t flags);
typedef int (*kiln_gc_visit_root_fn)(void *context, const void *pointer, uint32_t kind);
typedef int (*kiln_gc_visit_roots_fn)(void *context, kiln_gc_visit_root_fn visit, void *visit_context);
extern int kiln_realm_gc_register_roots_with_visitor(kiln_gc_roots_fn roots,
             kiln_gc_visit_roots_fn visit_roots, void *context, uint32_t flags);
extern void *kiln_c99_external_owner_create(void *context,
             kiln_gc_visit_roots_fn visit_roots, void (*release_instance)(void *context));
extern int kiln_c99_object_set_external_owner(kiln_value object_value, void *owner);
#define KILN_GC_VISIT_VALUE 2u
extern int kiln_realm_gc_unregister_roots(kiln_gc_roots_fn roots, void *context);
extern int kiln_realm_gc_mark_value(void *heap, const void *value);
#define KILN_GC_ROOT_SURVIVES_TEARDOWN 1u
#define KILN_GC_PHASE_RESET 2
#define KILN_GC_PHASE_LANE_RESET 3
#define KILN_C99_THROWN (-7)
#define KILN_C99_CONTROL_ERROR (-2)
#define KILN_C99_HOST_ERROR (-3)
/* Private to the generated graph launcher. Promise jobs propagate non-throw statuses, so this
 * ends the checkpoint without turning the failure into another rejected promise. */
#define KILN_GRAPH_FATAL_REJECTION (-1000)
extern int kiln_c99_process_object(kiln_value *result);
extern int kiln_c99_node_incoming_message_prototype(kiln_value *result);
#define KILN_C99_INVALID_ARGUMENT (-1)
#define KILN_C99_HOST_MODULE_CREATE_REQUIRE UINT16_C(0x7FF0)
#define KILN_C99_HINT_STRING 1
extern int kiln_c99_commonjs_require_value(kiln_value *out, kiln_commonjs_require_fn call, void *context);
extern int kiln_c99_commonjs_require_value_with_roots(kiln_value *out, kiln_commonjs_require_fn call, void *context, kiln_gc_visit_roots_fn visit_context, void *root_context);
extern int kiln_c99_commonjs_require_value_with_resolver(kiln_value *out, kiln_commonjs_require_fn call, kiln_commonjs_resolve_fn resolve, void *context, kiln_gc_visit_roots_fn visit_context, void *root_context);
extern int kiln_c99_default_require(void *context, kiln_host *host, kiln_value specifier, kiln_value *result, kiln_value *thrown);
extern int kiln_c99_new_object(kiln_value *result);
extern int kiln_c99_make_cell(kiln_value *slot);
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
static int kiln_graph_async_require_error(kiln_host *host, kiln_value *out, kiln_value *thrown) {
    int status = kiln_c99_new_type_error(out, "require() cannot load an ES module with top-level await");
    if (status != 0 && status != KILN_C99_THROWN) return status;
    return kiln_c99_put(host, *out, "code", 4, kiln_c99_copy_text("ERR_REQUIRE_ASYNC_MODULE", 24), 0, thrown);
}
extern int kiln_c99_invoke_method(kiln_host *host, kiln_value *result, uint64_t code,
                                  kiln_value receiver, const kiln_value *args, size_t argc,
                                  int depth, kiln_value *thrown);
extern int kiln_c99_call(kiln_host *host, kiln_value *result, kiln_value callee,
                         kiln_value receiver, const kiln_value *args, size_t argc,
                         int depth, kiln_value *thrown);
extern int kiln_c99_module_completion(kiln_host *, kiln_value, uint32_t, kiln_value *, int, kiln_value *);
extern int kiln_c99_finish_module_checkpoint(kiln_host *, kiln_value *);
extern void kiln_c99_gc_enter_job(void);
extern void kiln_c99_gc_leave_job(void);
#define KILN_C99_OK 0
#define KILN_C99_OUT_OF_MEMORY (-4)
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
        if let Some(instantiation) = &module.instantiation {
            writeln!(
                output,
                "extern int {}(kiln_value *cells, size_t count);\nextern int {}(kiln_host *host, kiln_value *result, const kiln_value *imports, size_t import_count, kiln_value *exports, size_t export_count);",
                instantiation.prepare_symbol,
                instantiation.symbol,
            )
            .unwrap();
        }
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
                "extern int {}_commonjs_wrapper(kiln_value *imports, size_t import_count, const char *filename, size_t filename_len, const char *dirname, size_t dirname_len, kiln_commonjs_require_fn require_call, kiln_commonjs_resolve_fn require_resolve, void *require_context, kiln_gc_visit_roots_fn require_visit_roots, void *require_root_context);",
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
    for (index, (node, module)) in modules.iter().enumerate() {
        let export_count = native_graph_export_count(node);
        if export_count != 0 {
            writeln!(
                linkage_fields,
                "    kiln_value exports_{index}[{export_count}];"
            )
            .unwrap();
            writeln!(
                linkage_names,
                "#define kiln_exports_{index} (graph->instance->exports_{index})"
            )
            .unwrap();
            if module.instantiation.is_some() {
                writeln!(
                    linkage_fields,
                    "    kiln_value cells_{index}[{export_count}];"
                )
                .unwrap();
                writeln!(
                    linkage_names,
                    "#define kiln_cells_{index} (graph->instance->cells_{index})"
                )
                .unwrap();
            }
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
                "#define kiln_imports_{index} (graph->instance->imports_{index})"
            )
            .unwrap();
        }
    }
    writeln!(
        linkage_fields,
        "    kiln_value namespaces[{}];",
        modules.len()
    )
    .unwrap();
    linkage_names.push_str("#define kiln_namespaces (graph->instance->namespaces)\n");
    if modules
        .iter()
        .any(|(node, _)| node.program.program.has_dynamic_import())
    {
        linkage_fields
            .push_str("    kiln_value opentui_namespace;\n    kiln_value node_ffi_namespace;\n");
        linkage_names.push_str("#define kiln_opentui_namespace (graph->instance->opentui_namespace)\n#define kiln_node_ffi_namespace (graph->instance->node_ffi_namespace)\n");
    }
    for (index, (node, _)) in modules.iter().enumerate() {
        if node.source == ModuleSource::CommonJs {
            writeln!(
                linkage_fields,
                "    kiln_commonjs_context commonjs_{index};"
            )
            .unwrap();
            writeln!(
                linkage_names,
                "#define kiln_commonjs_{index} (graph->instance->commonjs_{index})"
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
        /* `module.exports = value` may have happened before this back-edge, while the module is
         * still evaluating. Read the wrapper's current property so circular `require()` observes
         * that assignment, just as it observes mutations to the initial exports object. */
        if (context->imports == NULL || context->import_count < (size_t)3)
            return KILN_C99_CONTROL_ERROR;
        return kiln_c99_get(context->host, context->imports[2], "exports", 7, result, 0, thrown);
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
         typedef struct kiln_graph_realm_state {{\n    /* CLI-owned host state belongs to the realm shared by all its module instances. */\n    kiln_cli_host_state cli;\n}} kiln_graph_realm_state;\n\n\
         typedef struct kiln_graph_module_instance {{\n    /* Mutable module state and linkage are isolated per generation. */\n    kiln_value result;\n    kiln_value entry_result;\n    kiln_graph_module_state module_states[{}];\n    size_t current_module;\n    int roots_registered;\n    int weak_job_active;\n    int unsettled_entry;\n    size_t worker_entry;\n    struct kiln_graph_require_context require_contexts[{}];\n{linkage_fields}}} kiln_graph_module_instance;\n\n\
         struct kiln_graph_context {{\n    kiln_host host;\n    kiln_graph_realm_state *realm;\n    kiln_graph_module_instance *instance;\n}};\n{linkage_names}",
        modules.len(),
        runtime_importers.len().max(1),
    );
    output = output.replace("/* KILN_GRAPH_CONTEXT */\n", &context);
    output.push_str("static int kiln_graph_declared_require(kiln_graph_context *graph, kiln_value specifier, kiln_value *result, kiln_value *thrown, int *matched) {\n    *matched = 0;\n");
    for path in graph.declared_includes() {
        let Some(&provider_index) = indices.get(path) else {
            continue;
        };
        let matches = std::iter::once(path.to_string_lossy().into_owned())
            .chain(Url::from_file_path(path).ok().map(|url| url.to_string()))
            .map(|specifier| {
                format!(
                    "kiln_c99_string_equals_ascii(specifier, {}, (size_t){})",
                    native_c_string(&specifier),
                    specifier.len()
                )
            })
            .collect::<Vec<_>>()
            .join(" || ");
        writeln!(output, "    if ({matches}) {{ *matched = 1; return kiln_graph_load_commonjs(&kiln_commonjs_{provider_index}, result, thrown); }}").unwrap();
    }
    output.push_str("    (void)graph; (void)specifier; (void)result; (void)thrown;\n    return KILN_C99_OK;\n}\n");
    output.push_str(
        "static int kiln_graph_ensure_module(kiln_graph_context *graph, size_t index);\n",
    );
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
            } else if modules[provider_index].0.source == ModuleSource::Source {
                let has_top_level_await = synchronous_require_async[provider_index];
                if has_top_level_await {
                    writeln!(output, "    if (kiln_c99_string_equals_ascii(specifier, {specifier}, (size_t){specifier_len})) {{ kiln_value error = kiln_c99_undefined(); (void)kiln_graph_async_require_error(host, &error, thrown); if (thrown != NULL) *thrown = error; return KILN_C99_THROWN; }}").unwrap();
                } else {
                    writeln!(output, "    if (kiln_c99_string_equals_ascii(specifier, {specifier}, (size_t){specifier_len})) {{ int status = kiln_graph_ensure_module(graph, (size_t){provider_index}); if (status != KILN_C99_OK) {{ if (thrown != NULL) *thrown = graph->instance->module_states[{provider_index}].result; return status; }} *result = kiln_namespaces[{provider_index}]; return KILN_C99_OK; }}").unwrap();
                }
            } else {
                let Some(commonjs_export) =
                    native_commonjs_projection_index(modules[provider_index].0)
                else {
                    continue;
                };
                writeln!(
                    output,
                    "    if (kiln_c99_string_equals_ascii(specifier, {specifier}, (size_t){specifier_len})) {{ int status = kiln_graph_ensure_module(graph, (size_t){provider_index}); if (status != KILN_C99_OK) {{ if (thrown != NULL) *thrown = graph->instance->module_states[{provider_index}].result; return status; }} *result = kiln_exports_{provider_index}[{commonjs_export}]; return KILN_C99_OK; }}"
                )
                .unwrap();
            }
        }
        // Bounded direct eval can contain literal `require()` calls which are not ordinary
        // syntax-tree dependencies. They remain lazy runtime edges on the owning module, but
        // the wrapper's lexical `require` still needs to dispatch them when eval reaches them.
        for dependency in &node.runtime_dependencies {
            if dependency.resolve_only || dependency.importer != node.path {
                continue;
            }
            let Some(&provider_index) = indices.get(&dependency.target) else {
                continue;
            };
            let specifier = native_c_string(&dependency.specifier);
            let specifier_len = dependency.specifier.len();
            if modules[provider_index].0.source == ModuleSource::CommonJs {
                writeln!(
                    output,
                    "    if (kiln_c99_string_equals_ascii(specifier, {specifier}, (size_t){specifier_len})) return kiln_graph_load_commonjs(&kiln_commonjs_{provider_index}, result, thrown);"
                )
                .unwrap();
            } else if modules[provider_index].0.source == ModuleSource::Source {
                if synchronous_require_async[provider_index] {
                    writeln!(output, "    if (kiln_c99_string_equals_ascii(specifier, {specifier}, (size_t){specifier_len})) {{ kiln_value error = kiln_c99_undefined(); (void)kiln_graph_async_require_error(host, &error, thrown); if (thrown != NULL) *thrown = error; return KILN_C99_THROWN; }}").unwrap();
                } else {
                    writeln!(output, "    if (kiln_c99_string_equals_ascii(specifier, {specifier}, (size_t){specifier_len})) {{ int status = kiln_graph_ensure_module(graph, (size_t){provider_index}); if (status != KILN_C99_OK) {{ if (thrown != NULL) *thrown = graph->instance->module_states[{provider_index}].result; return status; }} *result = kiln_namespaces[{provider_index}]; return KILN_C99_OK; }}").unwrap();
                }
            } else if let Some(projection) =
                native_commonjs_projection_index(modules[provider_index].0)
            {
                writeln!(output, "    if (kiln_c99_string_equals_ascii(specifier, {specifier}, (size_t){specifier_len})) {{ int status = kiln_graph_ensure_module(graph, (size_t){provider_index}); if (status != KILN_C99_OK) {{ if (thrown != NULL) *thrown = graph->instance->module_states[{provider_index}].result; return status; }} *result = kiln_exports_{provider_index}[{projection}]; return KILN_C99_OK; }}").unwrap();
            }
        }
        output.push_str("    { int matched = 0; int status = kiln_graph_declared_require(graph, specifier, result, thrown, &matched); if (matched) return status; }\n    return kiln_c99_default_require(NULL, host, specifier, result, thrown);\n}\n");
        if node.source == ModuleSource::CommonJs {
            writeln!(output, "\nstatic int kiln_commonjs_resolve_{index}(void *opaque, kiln_host *host, kiln_value specifier, kiln_value *result, kiln_value *thrown) {{\n    kiln_graph_context *graph = (kiln_graph_context *)opaque;\n    (void)host;\n    if (graph == NULL || result == NULL || specifier.tag != KILN_VALUE_STRING || (specifier.len != 0 && specifier.ptr == NULL)) return KILN_C99_INVALID_ARGUMENT;").unwrap();
            for dependency in &node.runtime_dependencies {
                if dependency.importer != node.path {
                    continue;
                }
                let resolved = if matches!(dependency.source, ModuleSource::NodeBuiltin(_)) {
                    dependency.specifier.clone()
                } else {
                    dependency.target.to_string_lossy().into_owned()
                };
                writeln!(output, "    if (kiln_c99_string_equals_ascii(specifier, {}, (size_t){})) {{ *result = kiln_c99_copy_text({}, (size_t){}); return result->tag == KILN_VALUE_STRING ? KILN_C99_OK : KILN_C99_OUT_OF_MEMORY; }}", native_c_string(&dependency.specifier), dependency.specifier.len(), native_c_string(&resolved), resolved.len()).unwrap();
            }
            for ((failure_importer, failure_specifier), message) in
                graph.runtime_failures(&node.path)
            {
                if failure_importer != &node.path {
                    continue;
                }
                writeln!(output, "    if (kiln_c99_string_equals_ascii(specifier, {}, (size_t){})) {{ (void)kiln_c99_new_type_error(thrown, {}); return KILN_C99_THROWN; }}", native_c_string(failure_specifier), failure_specifier.len(), native_c_string(message)).unwrap();
            }
            if let Some(native_runtime_failures) = native_runtime_failures {
                for ((owner, failure_importer, failure_specifier), message) in
                    native_runtime_failures
                {
                    if owner != &node.path || failure_importer != &node.path {
                        continue;
                    }
                    writeln!(output, "    if (kiln_c99_string_equals_ascii(specifier, {}, (size_t){})) {{ (void)kiln_c99_new_type_error(thrown, {}); return KILN_C99_THROWN; }}", native_c_string(failure_specifier), failure_specifier.len(), native_c_string(message)).unwrap();
                }
            }
            output.push_str("    if (thrown != NULL) { (void)kiln_c99_new_type_error(thrown, \"Cannot find module\"); return KILN_C99_THROWN; }\n    return KILN_C99_HOST_ERROR;\n}\n");
        }
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
    return &graph->instance->module_states[index].result;
}

static kiln_value *kiln_graph_module_promise(kiln_graph_context *graph, size_t index) {
    static kiln_value undefined = { .tag = KILN_VALUE_UNDEFINED, .flags = 0, .len = 0, .bits = 0 };
    return graph->instance->module_states[index].completion_promise.tag == 16 /* KILN_VALUE_PROMISE */
        ? &graph->instance->module_states[index].completion_promise
        : &undefined;
}

static int kiln_graph_prepare_completion(kiln_graph_context *graph, size_t index) {
    kiln_graph_module_state *state;
    kiln_value deferred;
    int status;
    if (graph == NULL || index >= (size_t)MODULE_COUNT) return KILN_C99_CONTROL_ERROR;
    state = &graph->instance->module_states[index];
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
        "static int kiln_graph_host_call(void *context, uint16_t operation, uint16_t argc, const kiln_value *arguments, kiln_value *result);\nstatic int kiln_graph_gc_visit_roots(void *context, kiln_gc_visit_root_fn visit, void *visit_context);\nstatic int kiln_graph_ensure_module(kiln_graph_context *graph, size_t index);\nstatic int kiln_graph_runtime_resolve(void *opaque, kiln_host *host, kiln_value specifier, kiln_value *result, kiln_value *thrown);\n",
    );
    output.push_str("static int kiln_graph_runtime_require(void *opaque, kiln_host *host, kiln_value specifier, kiln_value *result, kiln_value *thrown) {\n    kiln_graph_require_context *context = (kiln_graph_require_context *)opaque;\n    kiln_graph_context *graph;\n    if (context == NULL || (graph = context->graph) == NULL || result == NULL || specifier.tag != KILN_VALUE_STRING || (specifier.len != 0 && specifier.ptr == NULL)) return KILN_C99_INVALID_ARGUMENT;\n    (void)host;\n    switch (context->base_index) {\n");
    for (base_index, importer) in runtime_importers.keys().enumerate() {
        output.push_str(&format!("        case {base_index}:\n"));
        for node in modules.iter().map(|(node, _)| node) {
            for dependency in &node.runtime_dependencies {
                if dependency.resolve_only {
                    continue;
                }
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
                } else if modules[provider_index].0.source == ModuleSource::Source {
                    let has_top_level_await = synchronous_require_async[provider_index];
                    if has_top_level_await {
                        writeln!(output, "            if (kiln_c99_string_equals_ascii(specifier, {specifier}, (size_t){specifier_len})) {{ kiln_value error = kiln_c99_undefined(); (void)kiln_graph_async_require_error(host, &error, thrown); if (thrown != NULL) *thrown = error; return KILN_C99_THROWN; }}").unwrap();
                    } else {
                        writeln!(output, "            if (kiln_c99_string_equals_ascii(specifier, {specifier}, (size_t){specifier_len})) {{ int status = kiln_graph_ensure_module(graph, (size_t){provider_index}); if (status != KILN_C99_OK) {{ if (thrown != NULL) *thrown = graph->instance->module_states[{provider_index}].result; return status; }} *result = kiln_namespaces[{provider_index}]; return KILN_C99_OK; }}").unwrap();
                    }
                } else if let Some(projection) =
                    native_commonjs_projection_index(modules[provider_index].0)
                {
                    writeln!(output, "            if (kiln_c99_string_equals_ascii(specifier, {specifier}, (size_t){specifier_len})) {{ int status = kiln_graph_ensure_module(graph, (size_t){provider_index}); if (status != KILN_C99_OK) {{ if (thrown != NULL) *thrown = graph->instance->module_states[{provider_index}].result; return status; }} *result = kiln_exports_{provider_index}[{projection}]; return KILN_C99_OK; }}").unwrap();
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
    output.push_str("        default: break;\n    }\n");
    for path in graph.declared_includes() {
        let Some(&provider_index) = indices.get(path) else {
            continue;
        };
        let matches = std::iter::once(path.to_string_lossy().into_owned())
            .chain(Url::from_file_path(path).ok().map(|url| url.to_string()))
            .map(|specifier| {
                format!(
                    "kiln_c99_string_equals_ascii(specifier, {}, (size_t){})",
                    native_c_string(&specifier),
                    specifier.len()
                )
            })
            .collect::<Vec<_>>()
            .join(" || ");
        if modules[provider_index].0.source == ModuleSource::CommonJs {
            writeln!(
                output,
                "    if ({matches}) return kiln_graph_load_commonjs(&kiln_commonjs_{provider_index}, result, thrown);"
            )
            .unwrap();
        } else if modules[provider_index].0.source == ModuleSource::Source {
            let has_top_level_await = synchronous_require_async[provider_index];
            if has_top_level_await {
                writeln!(output, "    if ({matches}) {{ kiln_value error = kiln_c99_undefined(); (void)kiln_graph_async_require_error(host, &error, thrown); if (thrown != NULL) *thrown = error; return KILN_C99_THROWN; }}").unwrap();
            } else {
                writeln!(output, "    if ({matches}) {{ int status = kiln_graph_ensure_module(graph, (size_t){provider_index}); if (status != KILN_C99_OK) {{ if (thrown != NULL) *thrown = graph->instance->module_states[{provider_index}].result; return status; }} *result = kiln_namespaces[{provider_index}]; return KILN_C99_OK; }}").unwrap();
            }
        } else if let Some(projection) = native_commonjs_projection_index(modules[provider_index].0)
        {
            writeln!(output, "    if ({matches}) {{ int status = kiln_graph_ensure_module(graph, (size_t){provider_index}); if (status != KILN_C99_OK) {{ if (thrown != NULL) *thrown = graph->instance->module_states[{provider_index}].result; return status; }} *result = kiln_exports_{provider_index}[{projection}]; return KILN_C99_OK; }}").unwrap();
        }
    }
    output.push_str(
        "    return kiln_c99_default_require(NULL, host, specifier, result, thrown);\n}\n\n",
    );
    output.push_str("static int kiln_graph_runtime_resolve(void *opaque, kiln_host *host, kiln_value specifier, kiln_value *result, kiln_value *thrown) {\n    kiln_graph_require_context *context = (kiln_graph_require_context *)opaque;\n    kiln_graph_context *graph;\n    if (context == NULL || (graph = context->graph) == NULL || result == NULL || specifier.tag != KILN_VALUE_STRING || (specifier.len != 0 && specifier.ptr == NULL)) return KILN_C99_INVALID_ARGUMENT;\n    (void)host;\n    switch (context->base_index) {\n");
    for (base_index, importer) in runtime_importers.keys().enumerate() {
        output.push_str(&format!("        case {base_index}:\n"));
        for node in modules.iter().map(|(node, _)| node) {
            for dependency in &node.runtime_dependencies {
                if dependency.importer != *importer {
                    continue;
                }
                let resolved = if matches!(dependency.source, ModuleSource::NodeBuiltin(_)) {
                    dependency.specifier.clone()
                } else {
                    dependency.target.to_string_lossy().into_owned()
                };
                writeln!(
                    output,
                    "            if (kiln_c99_string_equals_ascii(specifier, {}, (size_t){})) {{ *result = kiln_c99_copy_text({}, (size_t){}); return result->tag == KILN_VALUE_STRING ? KILN_C99_OK : KILN_C99_OUT_OF_MEMORY; }}",
                    native_c_string(&dependency.specifier),
                    dependency.specifier.len(),
                    native_c_string(&resolved),
                    resolved.len(),
                )
                .unwrap();
            }
            for ((failure_importer, failure_specifier), message) in
                graph.runtime_failures(&node.path)
            {
                if failure_importer != importer {
                    continue;
                }
                writeln!(
                    output,
                    "            if (kiln_c99_string_equals_ascii(specifier, {}, (size_t){})) {{ (void)kiln_c99_new_type_error(thrown, {}); return KILN_C99_THROWN; }}",
                    native_c_string(failure_specifier),
                    failure_specifier.len(),
                    native_c_string(message),
                )
                .unwrap();
            }
        }
        output.push_str("            break;\n");
    }
    output.push_str("        default: break;\n    }\n    if (thrown == NULL) return KILN_C99_HOST_ERROR;\n    (void)kiln_c99_new_type_error(thrown, \"Cannot find module\");\n    return KILN_C99_THROWN;\n}\n\n");
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
        let canonical = builtin.specifier();
        // Bootstrap/global require has no importing module's fixed table. Publish the same
        // builtin aliases as importer-scoped require, using the resolver as the authority.
        let aliases = std::iter::once(canonical).chain(
            canonical
                .strip_prefix("node:")
                .filter(|alias| NodeBuiltin::parse(alias) == Some(builtin)),
        );
        for specifier in aliases {
            writeln!(
                output,
                "    if (kiln_c99_string_equals_ascii(specifier, {}, (size_t){})) {{ int status = kiln_graph_ensure_module(graph, (size_t){}); if (status != KILN_C99_OK) {{ if (thrown != NULL) *thrown = graph->instance->module_states[{}].result; return status; }} *result = kiln_exports_{}[{}]; return KILN_C99_OK; }}",
                native_c_string(specifier), specifier.len(), index, index, index, projection
            )
            .unwrap();
        }
    }
    output.push_str("    return kiln_c99_default_require(NULL, host, specifier, result, thrown);\n}\n\nstatic int kiln_graph_create_require(kiln_graph_context *graph, uint16_t argc, const kiln_value *arguments, kiln_value *result) {\n    if (graph == NULL || result == NULL || argc != 1 || arguments == NULL || arguments[0].tag != KILN_VALUE_STRING || (arguments[0].len != 0 && arguments[0].ptr == NULL)) return KILN_C99_INVALID_ARGUMENT;\n");
    for (base_index, importer) in runtime_importers.keys().enumerate() {
        let importer = importer.to_string_lossy();
        let file_url = Url::from_file_path(importer.as_ref())
            .ok()
            .map(|url| url.to_string());
        writeln!(
            output,
            "    if (kiln_c99_filename_matches(arguments[0], {}, (size_t){}){} ) {{ graph->instance->require_contexts[{base_index}].graph = graph; graph->instance->require_contexts[{base_index}].base_index = (size_t){base_index}; return kiln_c99_commonjs_require_value_with_resolver(result, kiln_graph_runtime_require, kiln_graph_runtime_resolve, &graph->instance->require_contexts[{base_index}], kiln_graph_gc_visit_roots, graph); }}",
            native_c_string(importer.as_ref()),
            importer.len(),
            file_url.as_ref().map(|url| format!(" || kiln_c99_filename_matches(arguments[0], {}, (size_t){})", native_c_string(url), url.len())).unwrap_or_default(),
        ).unwrap();
    }
    // The entry's import.meta uses its relocated executable path. Only that module's
    // resolver table can be selected by the executable alias; dependency identities above
    // remain separate even when they request the same relative specifier.
    if let Some((base_index, _)) = runtime_importers
        .keys()
        .enumerate()
        .find(|(_, importer)| importer.as_path() == graph.entry())
    {
        writeln!(output, "    {{ const uint8_t *bytes = NULL; uint64_t length = 0; if (kiln_realm_argv_at(0, &bytes, &length) == KILN_C99_OK && kiln_c99_filename_matches(arguments[0], (const char *)bytes, (size_t)length)) {{ graph->instance->require_contexts[{base_index}].graph = graph; graph->instance->require_contexts[{base_index}].base_index = (size_t){base_index}; return kiln_c99_commonjs_require_value_with_resolver(result, kiln_graph_runtime_require, kiln_graph_runtime_resolve, &graph->instance->require_contexts[{base_index}], kiln_graph_gc_visit_roots, graph); }} }}").unwrap();
    }
    output.push_str(
        "    return kiln_c99_commonjs_require_value_with_roots(result, kiln_graph_require_builtin, graph, kiln_graph_gc_visit_roots, graph);\n}\n\nstatic int kiln_graph_host_call(void *context, uint16_t operation, uint16_t argc, const kiln_value *arguments, kiln_value *result) {\n    kiln_graph_context *graph = (kiln_graph_context *)context;\n    if (operation == KILN_C99_HOST_MODULE_CREATE_REQUIRE) return kiln_graph_create_require(graph, argc, arguments, result);\n    return kiln_cli_host_call(graph != NULL ? &graph->realm->cli : NULL, operation, argc, arguments, result);\n}\n",
    );
    // The module a worker names by its path (`new Worker(path)`, realms issue 06), or -1: every
    // module of the program a realm can run as its entry. Matched by its project-relative path at
    // the end of the one asked for -- the longest that matches -- so the launcher names no
    // absolute path and a sibling checkout reuses it, as it does the rest of the launcher.
    output.push_str("static long kiln_graph_module_index(const uint8_t *path, uint64_t len) {\n    long best = -1;\n    uint64_t best_len = 0;\n");
    for (index, (node, _)) in modules.iter().enumerate() {
        if !matches!(
            node.source,
            ModuleSource::Source
                | ModuleSource::CommonJs
                | ModuleSource::NodeBuiltin(NodeBuiltin::ActorsWorker)
        ) {
            continue;
        }
        if node.source == ModuleSource::NodeBuiltin(NodeBuiltin::ActorsWorker) {
            let path = NodeBuiltin::ActorsWorker.specifier();
            writeln!(output,
                "    if (len == (uint64_t){n} && memcmp(path, {text}, (size_t){n}) == 0) return {index};",
                n = path.len(), text = native_c_string(&path),
            ).unwrap();
            continue;
        }
        match logical_module_path(graph.project_root(), &node.path) {
            Ok(logical) => writeln!(
                output,
                "    if (len > (uint64_t){n} && best_len < (uint64_t){n} && path[len - {n} - 1] == '/' && memcmp(path + len - {n}, {text}, (size_t){n}) == 0) {{ best = {index}; best_len = (uint64_t){n}; }}",
                n = logical.len(),
                text = native_c_string(&logical),
            )
            .unwrap(),
            Err(_) => {
                let absolute = node.path.to_string_lossy();
                writeln!(
                    output,
                    "    if (len == (uint64_t){n} && memcmp(path, {text}, (size_t){n}) == 0) return {index};",
                    n = absolute.len(),
                    text = native_c_string(absolute.as_ref()),
                )
                .unwrap();
            }
        }
    }
    output.push_str("    return best;\n}\n");
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
    /* Extend the realm's existing capability table. Replacing it would discard host loader
     * and crypto hooks installed by process initialization before the TypeScript bootstrap. */
    status = kiln_c99_get(host, proc_obj, "__kilnNativeModules", 19, &modules_obj, 0, NULL);
    if (status != KILN_C99_OK) return status;

    kiln_value net_obj = kiln_c99_undefined();
    status = kiln_c99_new_object(&net_obj);
    if (status != KILN_C99_OK) return status;
    kiln_value net_connect = (kiln_value){ .tag = 11, .flags = 0, .len = 0, .bits = 338 /* KILN_C99_METHOD_NODE_NET_CONNECT */ };
    kiln_value net_create_server = (kiln_value){ .tag = 11, .flags = 0, .len = 0, .bits = 268 /* KILN_C99_METHOD_NODE_NET_CREATE_SERVER */ };
    kiln_value net_cork_socket = (kiln_value){ .tag = 11, .flags = 0, .len = 0, .bits = 565 /* KILN_C99_METHOD_NODE_SOCKET_CORK_KERNEL */ };
    kiln_value net_uncork_socket = (kiln_value){ .tag = 11, .flags = 0, .len = 0, .bits = 566 /* KILN_C99_METHOD_NODE_SOCKET_UNCORK_KERNEL */ };
    kiln_value net_pause_socket = (kiln_value){ .tag = 11, .flags = 0, .len = 0, .bits = 567 /* KILN_C99_METHOD_NODE_SOCKET_PAUSE_KERNEL */ };
    kiln_value net_resume_socket = (kiln_value){ .tag = 11, .flags = 0, .len = 0, .bits = 568 /* KILN_C99_METHOD_NODE_SOCKET_RESUME_KERNEL */ };
    kiln_value net_read_socket = (kiln_value){ .tag = 11, .flags = 0, .len = 0, .bits = 569 /* KILN_C99_METHOD_NODE_SOCKET_READ_KERNEL */ };
    status = kiln_c99_put(host, net_obj, "connect", 7, net_connect, 0, NULL);
    if (status != KILN_C99_OK) return status;
    status = kiln_c99_put(host, net_obj, "createConnection", 16, net_connect, 0, NULL);
    if (status != KILN_C99_OK) return status;
    status = kiln_c99_put(host, net_obj, "createServer", 12, net_create_server, 0, NULL);
    if (status != KILN_C99_OK) return status;
    status = kiln_c99_put(host, net_obj, "__corkSocket", 12, net_cork_socket, 0, NULL);
    if (status != KILN_C99_OK) return status;
    status = kiln_c99_put(host, net_obj, "__uncorkSocket", 14, net_uncork_socket, 0, NULL);
    if (status != KILN_C99_OK) return status;
    status = kiln_c99_put(host, net_obj, "__pauseSocket", 13, net_pause_socket, 0, NULL);
    if (status != KILN_C99_OK) return status;
    status = kiln_c99_put(host, net_obj, "__resumeSocket", 14, net_resume_socket, 0, NULL);
    if (status != KILN_C99_OK) return status;
    status = kiln_c99_put(host, net_obj, "__readSocket", 12, net_read_socket, 0, NULL);
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
    kiln_value http_incoming_ended = (kiln_value){ .tag = 11, .flags = 0, .len = 0, .bits = 571 /* KILN_C99_METHOD_NODE_HTTP_INCOMING_ENDED */ };
    status = kiln_c99_put(host, http_obj, "createFetchServer", 17, http_create_fetch_server, 0, NULL);
    if (status != KILN_C99_OK) return status;
    status = kiln_c99_put(host, http_obj, "fetchRespond", 12, http_fetch_respond, 0, NULL);
    if (status != KILN_C99_OK) return status;
    status = kiln_c99_put(host, http_obj, "__incomingMessageEnded", 22,
                          http_incoming_ended, 0, NULL);
    if (status != KILN_C99_OK) return status;
    kiln_value incoming_message_prototype = kiln_c99_undefined();
    status = kiln_c99_node_incoming_message_prototype(&incoming_message_prototype);
    if (status != KILN_C99_OK) return status;
    status = kiln_c99_put(host, http_obj, "__incomingMessagePrototype",
                          sizeof("__incomingMessagePrototype") - 1,
                          incoming_message_prototype, 0, NULL);
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

    return KILN_C99_OK;
}

/* A thread-mode worker realm's own `process.env` view: the process environment every realm
 * copies when it builds `process` (as a node worker thread gets a copy of its parent's), plus its
 * index and the count, which differ per realm and so never go into the shared `environ`. */
static int kiln_cli_put_worker_env(kiln_host *host) {
    kiln_value process = kiln_c99_undefined();
    kiln_value env = kiln_c99_undefined();
    char text[32];
    int length;
    int status = kiln_c99_process_object(&process);
    if (status == KILN_C99_OK) status = kiln_c99_get(host, process, "env", 3, &env, 0, NULL);
    if (status != KILN_C99_OK) return status;
    length = snprintf(text, sizeof text, "%ld", kiln_cli_thread_worker);
    status = kiln_c99_put(host, env, "KILN_WORKER_INDEX", 17, kiln_c99_copy_text(text, (size_t)length), 0, NULL);
    if (status != KILN_C99_OK) return status;
    length = snprintf(text, sizeof text, "%ld", kiln_cli_thread_workers);
    return kiln_c99_put(host, env, "KILN_WORKER_COUNT", 17, kiln_c99_copy_text(text, (size_t)length), 0, NULL);
}

static int kiln_graph_enter_inner(void *opaque) {
    kiln_graph_context *graph = (kiln_graph_context *)opaque;
    kiln_graph_entry_context context;
    kiln_host *host;
    kiln_value *result;
    int status = 0;
    if (graph == NULL) return -1;
    host = &graph->host;
    result = &graph->instance->result;
    (void)context;
"#,
    );
    for base_index in 0..runtime_importers.len() {
        writeln!(
            output,
            "    graph->instance->require_contexts[{base_index}].graph = graph; graph->instance->require_contexts[{base_index}].base_index = (size_t){base_index};"
        )
        .unwrap();
    }
    output.push_str(
        r#"
    if (!graph->instance->roots_registered) {
        status = kiln_realm_gc_register_roots_with_visitor(kiln_graph_gc_roots,
                         kiln_graph_gc_visit_roots, graph, KILN_GC_ROOT_SURVIVES_TEARDOWN);
        if (status != 0) return status;
        graph->instance->roots_registered = 1;
        graph->host.module_owner = kiln_c99_external_owner_create(
            graph, kiln_graph_gc_visit_roots, NULL);
        if (graph->host.module_owner == NULL) return KILN_C99_OUT_OF_MEMORY;
        /* A realm hosted beside others writes its standard streams where its host said. */
        if (graph->realm->cli.output != NULL)
            kiln_c99_realm_capture_output(graph->realm->cli.output, graph->realm->cli.error_output);
        status = kiln_install_native_modules(host);
        if (status != 0) return status;
        if (kiln_cli_thread_worker >= 0 && graph->instance->worker_entry == 0) {
            status = kiln_cli_put_worker_env(host);
            if (status != 0) return status;
        }
"#,
    );
    if let Some(bootstrap) = bootstrap {
        writeln!(
            output,
            "        status = {}(host, result, NULL, 0, NULL, 0);\n        if (status != 0) {{ if (result->tag == 9 /* KILN_VALUE_OBJECT */) *result = kiln_cli_describe_error(*result); if (result->tag != KILN_VALUE_STRING || result->len == 0) *result = kiln_graph_diagnostic(\"builtin-bootstrap\", \"@kiln/runtime-bootstrap\"); return status; }}\n        kiln_c99_builtin_bootstrap_done();",
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
    // Node exposes the entry module through every CommonJS `require.main`. The wrapper creates
    // one require function per source module, so publish the same entry module object onto each
    // require function after all wrappers exist and before any module body runs.
    if modules[entry_index].0.source == ModuleSource::CommonJs {
        for (index, (node, _)) in modules.iter().enumerate() {
            if node.source == ModuleSource::CommonJs {
                writeln!(
                    output,
                    "        status = kiln_c99_put(NULL, kiln_imports_{index}[1], \"main\", (size_t)4, kiln_imports_{entry_index}[2], 0, NULL);\n        if (status != KILN_C99_OK) {{ *result = kiln_graph_diagnostic(\"commonjs-main\", \"require.main property setup failed\"); return status; }}"
                )
                .unwrap();
            }
        }
    }
    // ModuleDeclarationInstantiation is a graph phase: allocate every binding cell first, then
    // wire imports to those cells, and only then create any function closures. No module body is
    // evaluated in this block.
    for (index, (node, module)) in modules.iter().enumerate() {
        let Some(instantiation) = &module.instantiation else {
            continue;
        };
        let export_count = native_graph_export_count(node);
        let cells = if export_count == 0 {
            "NULL"
        } else {
            &format!("kiln_cells_{index}")
        };
        writeln!(output, "        status = {}({cells}, (size_t){export_count}); if (status != KILN_C99_OK) return status;", instantiation.prepare_symbol).unwrap();
    }
    for (index, (_, module)) in modules.iter().enumerate() {
        if module.instantiation.is_some() {
            if namespace_plans[index].is_some() {
                write_graph_namespace_build(&mut output, index, &module.symbol, &[], false);
                writeln!(output, "        if (status != KILN_C99_OK) return status;").unwrap();
            }
        }
    }
    for (index, (_, module)) in modules.iter().enumerate() {
        if module.instantiation.is_some() {
            if let Some(plan) = &namespace_plans[index] {
                write_graph_namespace_build(&mut output, index, &module.symbol, plan, false);
                writeln!(output, "        if (status != KILN_C99_OK) return status;").unwrap();
            }
        }
    }
    for (index, (node, _)) in modules.iter().enumerate() {
        if !modules[index].1.instantiation.is_some() {
            continue;
        }
        for (import_index, import) in node.program.imports.iter().enumerate() {
            if live_imports
                .is_some_and(|imports| !imports.contains(&(node.path.clone(), import_index)))
            {
                continue;
            }
            if let NativeGraphImport::Binding {
                provider_index,
                export_index,
                live_cell: true,
            } = graph_import_value(graph, modules, &indices, node, import)?
            {
                writeln!(output, "        kiln_imports_{index}[{import_index}] = kiln_cells_{provider_index}[{export_index}];").unwrap();
            }
            if let NativeGraphImport::Namespace { provider_index } =
                graph_import_value(graph, modules, &indices, node, import)?
            {
                if modules[provider_index].1.instantiation.is_some() {
                    writeln!(output, "        kiln_imports_{index}[{import_index}] = kiln_namespaces[{provider_index}];").unwrap();
                } else {
                    writeln!(output, "        status = kiln_c99_make_cell(&kiln_imports_{index}[{import_index}]); if (status != KILN_C99_OK) return status;").unwrap();
                }
            }
            match graph_import_value(graph, modules, &indices, node, import)? {
                NativeGraphImport::Binding {
                    live_cell: false, ..
                }
                | NativeGraphImport::CommonJsNamed { .. } => {
                    writeln!(output, "        status = kiln_c99_make_cell(&kiln_imports_{index}[{import_index}]); if (status != KILN_C99_OK) return status;").unwrap();
                }
                _ => {}
            }
        }
    }
    for (index, (node, module)) in modules.iter().enumerate() {
        let Some(instantiation) = &module.instantiation else {
            continue;
        };
        let import_count = node.program.imports.len();
        let imports = if import_count == 0 {
            "NULL"
        } else {
            &format!("kiln_imports_{index}")
        };
        let export_count = native_graph_export_count(node);
        let exports = if export_count == 0 {
            "NULL"
        } else {
            &format!("kiln_exports_{index}")
        };
        writeln!(output, "        status = {}(host, result, {imports}, (size_t){import_count}, {exports}, (size_t){export_count}); if (status != KILN_C99_OK) return status;", instantiation.symbol).unwrap();
    }
    output.push_str("    }\n");
    // A worker's realm (`node:worker_threads`, realms issue 06) runs one module of the program as
    // its entry -- with what that module imports, first -- rather than the program's startup list.
    writeln!(
        evaluation,
        "    if (graph->instance->worker_entry != 0) {{\n        size_t index = graph->instance->worker_entry - 1;\n        graph->instance->current_module = (size_t){};\n        status = kiln_graph_ensure_module(graph, index);\n        graph->instance->result = graph->instance->module_states[index].result;\n        return status;\n    }}",
        startup_indices.len()
    )
    .unwrap();
    writeln!(
        evaluation,
        "    while (graph->instance->current_module < (size_t){}) {{\n        switch (graph->instance->current_module) {{",
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
            "static int kiln_graph_eval_module_{index}(kiln_graph_context *graph) {{\n    kiln_graph_entry_context context;\n    kiln_graph_module_state *state = &graph->instance->module_states[{index}];\n    kiln_host *host = &graph->host;\n    kiln_value *result = &state->result;\n    kiln_value continuation_promise = kiln_c99_undefined();\n    int status = 0;"
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
                "            status = kiln_graph_load_commonjs(&kiln_commonjs_{index}, result, result);\n            if (status != 0) {{ if (status != KILN_C99_THROWN && result->tag == KILN_VALUE_UNDEFINED) {{ if (status == KILN_C99_HOST_ERROR && graph->realm->cli.last_error.tag == KILN_VALUE_STRING && graph->realm->cli.last_error.len != 0) *result = graph->realm->cli.last_error; else *result = kiln_graph_diagnostic(\"commonjs-evaluate\", {}); }} return status; }}",
                native_c_string(&logical_paths[index])
            )
            .unwrap();
            if node.path == graph.entry() {
                evaluation.push_str("            graph->instance->entry_result = *result;\n");
            }
            writeln!(
                evaluation,
                "            kiln_namespaces[{index}] = kiln_exports_{index}[0];"
            )
            .unwrap();
            if module.instantiation.is_none() {
                if let Some(plan) = &namespace_plans[index] {
                    write_graph_namespace_build(
                        &mut evaluation,
                        index,
                        &module.symbol,
                        plan,
                        false,
                    );
                    writeln!(
                    evaluation,
                    "            if (status != 0) {{ *result = kiln_graph_diagnostic(\"namespace\", {}); return status; }}",
                    native_c_string(&logical_paths[index])
                )
                .unwrap();
                }
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
                    "graph->instance->unsettled_entry = 1;\n                        "
                } else {
                    ""
                }
            )
            .unwrap();
            if node.path == graph.entry() {
                evaluation.push_str("                graph->instance->entry_result = (promise != NULL) ? promise->value : *result;\n");
            }
            if module.instantiation.is_none() {
                if let Some(plan) = &namespace_plans[index] {
                    write_graph_namespace_build(
                        &mut evaluation,
                        index,
                        &module.symbol,
                        plan,
                        false,
                    );
                    writeln!(
                    evaluation,
                    "                if (status != 0) {{ *result = kiln_graph_diagnostic(\"namespace\", {}); return status; }}",
                    native_c_string(&logical_paths[index])
                )
                .unwrap();
                }
            }
            evaluation.push_str(
                r#"                state->module_settling = 0;
                return 0;
            }
"#,
            );
        }

        // ESM imports are evaluated before the module body. CommonJS `require()` calls
        // are expressions: a literal inside a function or an untaken branch must wait
        // until that call actually runs. The CommonJS loader below ensures the target
        // at the call site.
        let static_dependencies: Vec<usize> = if node.source == ModuleSource::CommonJs {
            Vec::new()
        } else {
            node.dependencies
                .iter()
                .filter(|dependency| {
                    node.program
                        .dependencies
                        .iter()
                        .any(|specifier| specifier == &dependency.specifier)
                })
                .filter_map(|dependency| indices.get(&dependency.target).copied())
                .collect()
        };
        for (dependency_cursor, provider_index) in static_dependencies.iter().enumerate() {
            writeln!(
                evaluation,
                "            if (state->dependency_cursor == (size_t){dependency_cursor}) {{ status = kiln_graph_ensure_module(graph, (size_t){provider_index}); if (status != KILN_C99_OK) {{ *result = graph->instance->module_states[{provider_index}].result; return status; }} state->dependency_cursor++; if (kiln_graph_module_promise(graph, (size_t){provider_index})->tag == 16 /* KILN_VALUE_PROMISE */) {{ status = kiln_graph_prepare_completion(graph, (size_t){index}); if (status != KILN_C99_OK) return status; status = kiln_c99_module_completion(host, *kiln_graph_module_promise(graph, (size_t){provider_index}), (uint32_t){index}, &continuation_promise, 1, NULL); if (status != KILN_C99_OK) return status; state->module_settling = 1; return 0; }} }}"
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
                    live_cell,
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
                        "            kiln_imports_{index}[{import_index}] = {}[{export_index}];",
                        if live_cell {
                            format!("kiln_cells_{provider_index}")
                        } else {
                            format!("kiln_exports_{provider_index}")
                        }
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
        // Every engine builtin module's top-level code runs inside the engine-private window:
        // `globalThis.__kilnPrivate` reads the realm's private object there and nowhere else.
        let engine_module = matches!(node.source, ModuleSource::NodeBuiltin(_));
        if engine_module {
            evaluation.push_str("            status = kiln_realm_private_window(1);\n            if (status != 0) return status;\n");
        }
        evaluation.push_str(
            "            state->body_started = 1;\n            status = context.module(context.host, context.result, context.imports, context.import_count, context.exports, context.export_count);\n",
        );
        if engine_module {
            evaluation.push_str("            { int hook_status = kiln_realm_private_window(0); if (status == 0) status = hook_status; }\n");
        }
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
            "            if (status != 0) {{ if (status != KILN_C99_THROWN && result->tag == KILN_VALUE_UNDEFINED) {{ if (status == KILN_C99_HOST_ERROR && graph->realm->cli.last_error.tag == KILN_VALUE_STRING && graph->realm->cli.last_error.len != 0) *result = graph->realm->cli.last_error; else *result = kiln_graph_diagnostic(\"module-enter\", {}); }} return status; }}",
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
                graph->instance->entry_result = (promise != NULL && promise->state == 1) ? promise->value : *result;
            } else {
                graph->instance->entry_result = *result;
            }
"#,
                );
            } else {
                evaluation.push_str("            graph->instance->entry_result = *result;\n");
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
            "        case {index}: {{\n            kiln_graph_module_state *state = &graph->instance->module_states[{index}];\n            kiln_value settled = kiln_c99_undefined();\n            if (rejected) {{ state->result = *value; state->phase = KILN_MODULE_FAILED; state->failure_status = KILN_C99_THROWN; state->module_settling = 0; *result = *value; if (state->completion_reject.tag != KILN_VALUE_UNDEFINED) {{ status = kiln_c99_call(host, &settled, state->completion_reject, kiln_c99_undefined(), value, 1, depth, thrown); if (status != KILN_C99_OK) return status; }} {fatal} return KILN_C99_OK; }}\n            if (!state->body_started) {{ state->module_settling = 0; status = kiln_graph_eval_module_{index}(graph); if (status != KILN_C99_OK) {{ state->phase = KILN_MODULE_FAILED; state->failure_status = status; *result = state->result; if (state->completion_reject.tag != KILN_VALUE_UNDEFINED) {{ status = kiln_c99_call(host, &settled, state->completion_reject, kiln_c99_undefined(), &state->result, 1, depth, thrown); if (status != KILN_C99_OK) return status; }} return KILN_C99_OK; }} *result = kiln_c99_undefined(); return KILN_C99_OK; }}\n            if (state->phase == KILN_MODULE_EVALUATED) {{ *result = kiln_namespaces[{index}]; return KILN_C99_OK; }}\n",
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
        if module.instantiation.is_none() {
            if let Some(plan) = &namespace_plans[index] {
                write_graph_namespace_build(&mut helper_defs, index, &module.symbol, plan, false);
            }
        }
        helper_defs.push_str(&format!("            state->result = kiln_namespaces[{index}];\n            state->phase = KILN_MODULE_EVALUATED;\n            state->module_settling = 0;\n            *result = state->result;\n            if (state->completion_resolve.tag != KILN_VALUE_UNDEFINED) {{ status = kiln_c99_call(host, &settled, state->completion_resolve, kiln_c99_undefined(), &state->result, 1, depth, thrown); if (status != KILN_C99_OK) return status; }}\n", index = index));
        if node.path == graph.entry() {
            helper_defs.push_str("            graph->instance->entry_result = *value;\n");
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
            writeln!(evaluation, "        case {startup_order}: {{ status = kiln_graph_eval_module_{index}(graph); graph->instance->result = graph->instance->module_states[{index}].result; if (status != 0) return status; if (!graph->instance->module_states[{index}].module_settling) graph->instance->current_module++; return 0; }}").unwrap();
        } else {
            writeln!(evaluation, "        case {startup_order}: {{ status = kiln_graph_ensure_module(graph, (size_t){index}); graph->instance->result = graph->instance->module_states[{index}].result; if (status != 0) return status; graph->instance->current_module++; return 0; }}").unwrap();
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
    writeln!(
        output,
        "static int kiln_graph_enter(void *opaque) {{\n    kiln_graph_context *graph = (kiln_graph_context *)opaque;\n    int status;\n    if (graph == NULL) return -1;\n    if (!graph->instance->weak_job_active) {{ kiln_c99_gc_enter_job(); graph->instance->weak_job_active = 1; }}\n    status = kiln_graph_enter_inner(opaque);\n    if (status != 0 || graph->instance->current_module >= (size_t){} ||\n        graph->instance->module_states[kiln_graph_startup_indices[graph->instance->current_module]].module_settling) {{\n        kiln_c99_gc_leave_job();\n        graph->instance->weak_job_active = 0;\n    }}\n    return status;\n}}",
        startup_indices.len()
    )
    .unwrap();
    output.push_str(&helper_defs);
    output.push_str(
        "static int kiln_graph_ensure_module(kiln_graph_context *graph, size_t index) {\n",
    );
    output.push_str("    int status;\n");
    output.push_str("    if (graph == NULL || index >= (size_t)");
    output.push_str(&modules.len().to_string());
    output.push_str(
        ") return KILN_C99_CONTROL_ERROR;\n    if (graph->instance->module_states[index].phase == KILN_MODULE_EVALUATED) return 0;\n    if (graph->instance->module_states[index].phase == KILN_MODULE_FAILED) return graph->instance->module_states[index].failure_status;\n    if (graph->instance->module_states[index].phase == KILN_MODULE_EVALUATING) return 0;\n    graph->instance->module_states[index].phase = KILN_MODULE_EVALUATING;\n    switch (index) {\n",
    );
    for index in 0..modules.len() {
        writeln!(output, "        case {index}: {{").unwrap();
        let dependencies = if modules[index].0.source == ModuleSource::CommonJs {
            &[][..]
        } else {
            &modules[index].0.dependencies[..]
        };
        for (dependency_index, dependency) in dependencies.iter().enumerate() {
            if let Some(&provider) = indices.get(&dependency.target) {
                writeln!(
                    output,
                    "            {}status = kiln_graph_ensure_module(graph, (size_t){provider}); if (status != KILN_C99_OK) {{ graph->instance->module_states[index].result = graph->instance->module_states[{provider}].result; graph->instance->module_states[index].phase = KILN_MODULE_FAILED; graph->instance->module_states[index].failure_status = status; return status; }}",
                    if dependency_index == 0 { "int " } else { "" }
                )
                .unwrap();
            }
        }
        writeln!(
            output,
            "            status = kiln_graph_eval_module_{index}(graph);\n            if (status != KILN_C99_OK) {{ graph->instance->module_states[index].phase = KILN_MODULE_FAILED; graph->instance->module_states[index].failure_status = status; return status; }}\n            return 0;\n        }}"
        )
        .unwrap();
    }
    output.push_str("        default: return KILN_C99_CONTROL_ERROR;\n    }\n}\n");
    output.push_str(
        r#"/* argc/argv reach the realm before it is entered, so the first `process` a module builds
 * already has the command line. The strings are copied on the other side of this call. `output`
 * is where the realm's standard streams go (NULL: the process's). Returns the status the process
 * exits with for this realm: 0, 1, or node's 13 for a top-level await that never settled. */
static int kiln_cli_run_realm_as(int argc, char **argv, int writes_result, FILE *output,
                                 FILE *error_output, size_t worker_entry);
static int kiln_cli_run_realm(int argc, char **argv, int writes_result, FILE *output) {
    return kiln_cli_run_realm_as(argc, argv, writes_result, output, NULL, 0);
}

/* A worker's realm is over: its code is the one its end decided, whatever teardown says. */
static int kiln_cli_end_worker_realm(uint64_t worker_realm, kiln_graph_context *graph, int code) {
    (void)kiln_realm_destroy(worker_realm);
    free(graph);
    return code;
}

/* In a worker's realm, report the exception that ended it to the parent's `error` event. */
static int kiln_cli_report_worker_error(void *opaque) {
    kiln_graph_context *graph = (kiln_graph_context *)opaque;
    return kiln_c99_worker_report_error(graph->instance->result);
}

/* A scheduled turn saves its worker and process-exit state in the realm scope before the
 * launcher regains control. Enter that scope to consume the exit request: reading the lane's
 * thread-local slot here would see whichever realm used the lane outside this turn. */
typedef struct kiln_cli_exit_request {
    int found;
    int32_t code;
} kiln_cli_exit_request;

static int kiln_cli_read_exit_request(void *opaque) {
    kiln_cli_exit_request *request = (kiln_cli_exit_request *)opaque;
    request->found = kiln_worker_exit_code(&request->code);
    return request->found < 0 ? request->found : KILN_C99_OK;
}

static int kiln_cli_take_realm_exit_code(uint64_t realm, int32_t *out) {
    kiln_cli_exit_request request = {0};
    int status = kiln_realm_enter(realm, kiln_cli_read_exit_request, &request);
    if (status != KILN_C99_OK) return -1;
    if (request.found == 1) *out = request.code;
    return request.found;
}

/* The graph and result slots outlive every generated C frame. A scheduler can run one step,
 * release its lane, and later resume this record without retaining a frame pointer. */
typedef struct kiln_cli_realm_run {
    kiln_graph_context *graph;
    /* Realm services and module-instance values have separate owners. */
    kiln_graph_realm_state realm_state;
    kiln_graph_module_instance module_instance;
    uint64_t realm;
    const char *phase;
    int status;
    int final_drive;
    int settling_driven;
    int scheduled;
    uint64_t park_delay_ns;
} kiln_cli_realm_run;

/* `worker_entry` is one plus the index of the module a worker's realm runs (0 for the program's
 * own realm); a worker's standard streams are `output` and `error_output`. */
static void kiln_cli_realm_begin(kiln_cli_realm_run *run, int argc, char **argv, FILE *output,
                                  FILE *error_output, size_t worker_entry) {
    /* The realm's graph: its module states and every module's linkage, one per realm run and freed
     * after the realm is destroyed (a result slot is read after teardown). */
    kiln_graph_context *graph = (kiln_graph_context *)calloc(1, sizeof *graph);
    uint64_t realm = 0;
    int status = 1;
    memset(run, 0, sizeof *run);
    run->graph = graph;
    run->phase = "realm creation";
    run->status = 1;
    if (graph == NULL) {
        kiln_cli_warn("kiln: out of memory for the module graph", NULL);
        return;
    }
    graph->realm = &run->realm_state;
    graph->instance = &run->module_instance;
    graph->realm->cli.output = output;
    graph->realm->cli.error_output = error_output;
    graph->instance->worker_entry = worker_entry;
    graph->instance->result.tag = KILN_VALUE_UNDEFINED;
    graph->instance->entry_result.tag = KILN_VALUE_UNDEFINED;
    graph->host.call = kiln_graph_host_call;
    graph->host.context = graph;
    graph->host.module_context = graph;
    graph->host.module_owner = NULL;
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
    output.push_str("    run->realm = realm;\n    run->status = status;\n}\n");
    writeln!(
        output,
        "static const int kiln_cli_virtual_clock = {};",
        i32::from(!policy.host_clock)
    )
    .unwrap();
    writeln!(
        output,
        r#"
/* One provider drive answers 4 when quiescent, 1 for another ready turn, 2 for an I/O wake,
 * 3 for a pure timer deadline, and 0 for failure. The continuous launcher uses the same state
 * machine but lets its provider call wait until quiescence. */
static int kiln_cli_realm_drive_step(kiln_cli_realm_run *run) {{
    kiln_graph_context *graph = run->graph;
    if (!run->scheduled) {{
        run->status = kiln_realm_drive(run->realm, &graph->host, &graph->instance->result);
        return run->status == 0 ? 4 : 0;
    }}
    uint32_t kind = 3;
    uint64_t delay_ns = 0;
    run->status = kiln_realm_drive_step(run->realm, &graph->host, &graph->instance->result,
                                        &kind, &delay_ns);
    if (run->status != 0) return 0;
    run->park_delay_ns = delay_ns;
    return kind == 0 ? 1 : kind == 1 ? 2 : kind == 2 ? 3 : kind == 3 ? 4 : 0;
}}

/* Advance at most one generated module body or one provider language turn. */
static int kiln_cli_realm_step(kiln_cli_realm_run *run) {{
    kiln_graph_context *graph = run->graph;
    uint64_t realm = run->realm;
    if (run->status != 0 || graph == NULL) return 0;
    if (graph->instance->current_module < (size_t){} || !graph->instance->roots_registered) {{
        if (!run->settling_driven && graph->instance->current_module < (size_t){} &&
            graph->instance->module_states[kiln_graph_startup_indices[graph->instance->current_module]].module_settling) {{
            run->phase = "the turn loop";
            int progress = kiln_cli_realm_drive_step(run);
            if (progress == 4) {{ run->settling_driven = 1; return 1; }}
            return progress;
        }} else {{
            run->settling_driven = 0;
            run->phase = "module body";
            run->status = kiln_realm_enter(realm, kiln_graph_enter, graph);
        }}
        return run->status == 0;
    }}
    if (!run->final_drive) {{
        run->phase = "the turn loop";
        int progress = kiln_cli_realm_drive_step(run);
        if (progress == 4) {{ run->final_drive = 1; return 0; }}
        return progress;
    }}
    return 0;
}}

static int kiln_cli_realm_finish(kiln_cli_realm_run *run, int writes_result,
                                  size_t worker_entry) {{
    kiln_graph_context *graph = run->graph;
    uint64_t realm = run->realm;
    const char *phase = run->phase;
    int status = run->status;
    int destroy_status;
    int unsettled_entry = 0;
    /* `process.exit(code)` in a thread-mode realm (realms issue 23): the code, and whether it
     * was asked for. */
    int32_t exit_requested = 0;
    int exit_asked = 0;
    if (graph == NULL) {{ free(run); return 1; }}"#,
        startup_indices.len(),
        startup_indices.len()
    )
    .unwrap();
    output.push_str(
        r##"
    if (status == KILN_GRAPH_FATAL_REJECTION) {
        status = KILN_C99_THROWN;
        phase = "module body";
    }
    if (status == 0 && graph->instance->entry_result.tag == 16 /* KILN_VALUE_PROMISE */) {
        typedef struct kiln_launcher_promise_view {
            int state;
            kiln_value value;
        } kiln_launcher_promise_view;
        kiln_launcher_promise_view *promise = (kiln_launcher_promise_view *)(uintptr_t)graph->instance->entry_result.bits;
        if (promise != NULL) {
            if (promise->state == 2 /* KILN_C99_REJECTED */) {
                status = KILN_C99_THROWN;
                phase = "module body";
                graph->instance->result = kiln_cli_describe_error(promise->value);
            } else if (promise->state == 0 /* KILN_C99_PENDING */) {
                status = KILN_C99_CONTROL_ERROR;
                phase = "the turn loop";
                unsettled_entry = 1;
                graph->instance->result = kiln_graph_diagnostic("top-level await", "unsettled");
            } else if (promise->state == 1 /* KILN_C99_FULFILLED */) {
                graph->instance->entry_result = promise->value;
            }
        }
    }
    /* A worker's realm ends the way node ends a worker: an exit it asked for (`process.exit`,
     * its parent's `terminate`) is its code with nothing reported, and an exception nothing
     * caught goes to the parent's `error` event -- a copy made inside the realm, before the value
     * is turned into the text a main realm prints -- with 1 for its code. */
    if (worker_entry != 0) {
        int32_t requested = 0;
        int code = 0;
        int exit_request = kiln_cli_take_realm_exit_code(realm, &requested);
        if (exit_request < 0) {
            code = 1;
        } else if (exit_request == 1) {
            code = requested;
        } else if (status == KILN_C99_THROWN) {
            (void)kiln_realm_enter(realm, kiln_cli_report_worker_error, graph);
            code = 1;
        } else if (status != 0) {
            if (graph->instance->result.tag != KILN_VALUE_STRING) graph->instance->result = kiln_c99_text(graph->instance->result);
            (void)kiln_realm_enter(realm, kiln_cli_report_worker_error, graph);
            code = 1;
        }
        int completed = kiln_cli_end_worker_realm(realm, graph, code);
        free(run);
        return completed;
    }
    /* `process.exit(code)` in a realm of `KILN_WORKERS` thread mode ends that realm, not the
     * process (realms issue 23): the runtime recorded the code and the realm unwound here. The
     * launcher decides what the code means for the other realms, as the supervisor does for a
     * process-mode worker's. */
    int exit_request = kiln_cli_take_realm_exit_code(realm, &exit_requested);
    if (exit_request < 0 && status == 0) status = KILN_C99_CONTROL_ERROR;
    if (exit_request == 1) {
        exit_asked = 1;
        status = 0;
        writes_result = 0;
    }
    /* Graph module entries carry the original thrown value across the module ABI so dynamic
     * import and catch preserve language identity. Only now, after the graph and its turn loop
     * have established that the value is uncaught, attach the native refusal provenance. A
     * handled throw resets the realm diagnostic bridge, so this cannot leak an earlier refusal
     * into a later ordinary error. */
    if (status == KILN_C99_THROWN) {
        kiln_value message = kiln_cli_describe_error(graph->instance->result);
        if (message.tag != KILN_VALUE_STRING) {
            message = kiln_c99_text(message);
        }
        kiln_value site = kiln_c99_control_diagnostic();
        if (kiln_c99_diagnostic_available() && message.tag == KILN_VALUE_STRING &&
            site.tag == KILN_VALUE_STRING && site.len != 0) {
            kiln_value separator = kiln_c99_copy_text(" at ", 4);
            graph->instance->result = kiln_c99_concat(kiln_c99_concat(message, separator), site);
        } else {
            graph->instance->result = message;
        }
    }
    /* An entry whose top-level await never settled, with the loop out of work: node's answer is a
     * warning and status 13 rather than a refusal, and both engines give that one. */
    unsettled_entry = status == KILN_C99_CONTROL_ERROR && (unsettled_entry || graph->instance->unsettled_entry);
    if (unsettled_entry) kiln_cli_warn("Warning: Detected unsettled top-level await", &graph->realm->cli);
    else if (status != 0) kiln_cli_report_failure_with(status, graph->instance->result, phase, &graph->realm->cli);
    if (writes_result && status == 0 && kiln_cli_write_result(graph->instance->entry_result) != 0) {
        static const kiln_string refusal_str = { 17, 0, "filesystem access" };
        kiln_cli_report_failure_with(KILN_C99_HOST_ERROR, (kiln_value){ .tag = KILN_VALUE_STRING, .flags = 0, .len = 17, .ptr = (const char *)&refusal_str }, "result write", &graph->realm->cli);
        status = 1;
    }
    destroy_status = kiln_realm_destroy(realm);
    free(graph);
    free(run);
    if (exit_asked) return (int)exit_requested;
    /* Every failure folds to 1, the way a refusal does; 13 is the one status a program's caller
     * is told apart by, as `process.exit(code)` is. */
    if (unsettled_entry) return 13;
    if (status == 0) status = destroy_status;
    return status == 0 ? 0 : 1;
}

static int kiln_cli_run_realm_as(int argc, char **argv, int writes_result, FILE *output,
                                 FILE *error_output, size_t worker_entry) {
    kiln_cli_realm_run *run = (kiln_cli_realm_run *)calloc(1, sizeof *run);
    if (run == NULL) {
        kiln_cli_warn("kiln: out of memory for the realm session", NULL);
        return 1;
    }
    kiln_cli_realm_begin(run, argc, argv, output, error_output, worker_entry);
    while (kiln_cli_realm_step(run)) {}
    return kiln_cli_realm_finish(run, writes_result, worker_entry);
}

/* A worker's standard streams (`node:worker_threads`): each write goes to the parent as a message,
 * which the parent's `Worker` writes to its own stream, as node pipes a worker's. */
#if defined(__APPLE__)
static int kiln_cli_worker_write(void *cookie, const char *bytes, int length) {
    (void)kiln_worker_stdio((uint32_t)(uintptr_t)cookie, (const uint8_t *)bytes, (uint64_t)length);
    return length;
}
static FILE *kiln_cli_worker_stream(uint32_t stream) {
    FILE *file = funopen((void *)(uintptr_t)stream, NULL, kiln_cli_worker_write, NULL, NULL);
    if (file != NULL) setvbuf(file, NULL, _IONBF, 0);
    return file;
}
#elif !defined(_WIN32)
static ssize_t kiln_cli_worker_write(void *cookie, const char *bytes, size_t length) {
    (void)kiln_worker_stdio((uint32_t)(uintptr_t)cookie, (const uint8_t *)bytes, (uint64_t)length);
    return (ssize_t)length;
}
static FILE *kiln_cli_worker_stream(uint32_t stream) {
    cookie_io_functions_t functions = {NULL, kiln_cli_worker_write, NULL, NULL};
    FILE *file = fopencookie((void *)(uintptr_t)stream, "w", functions);
    if (file != NULL) setvbuf(file, NULL, _IONBF, 0);
    return file;
}
#else
static FILE *kiln_cli_worker_stream(uint32_t stream) {
    (void)stream;
    return NULL;
}
#endif

/* A worker graph is a heap-owned session just like a top-level scheduled realm. Its streams and
 * result slots survive every one-turn callback; no generated C frame survives an advance. */
typedef struct kiln_cli_worker_session {
    kiln_cli_realm_run *run;
    FILE *out;
    FILE *err;
    size_t worker_entry;
    int failed_begin;
} kiln_cli_worker_session;

/* A worker made from source text needs a compiler at run time, which a native program does not
 * carry. Report an invalid entry on the parent's error channel while the worker context is live. */
static void kiln_cli_worker_bad_entry(int32_t eval, const uint8_t *entry, uint64_t entry_len) {
    static const char no_eval[] = "kiln: a worker made from source text ({ eval: true }) needs a compiler at run time, which a natively built program does not have: put the worker's code in a module of the program";
    /* Name the module and why: a target the compiler could not see named (ADR 0072's rule for
     * actor modules, which workers share). The worker then exits 1 after this 'error'. */
    static const char missing_suffix[] = "' was not linked into this build: a native build links only the worker modules a new Worker call names at compile time (a string literal, new URL(literal, import.meta.url), path.join(__dirname, literal) or __filename); this name was computed at run time";
    static const char error_name[] = "Error";
    char missing[1024 + sizeof missing_suffix];
    const char *text = no_eval;
    size_t length = sizeof no_eval - 1;
    if (!eval) {
        int shown = entry_len > 1024 ? 1024 : (int)entry_len;
        int written = snprintf(missing, sizeof missing, "Cannot find module '%.*s%s", shown,
                               entry != NULL ? (const char *)entry : "", missing_suffix);
        text = missing;
        length = written < 0 ? 0 : (size_t)written < sizeof missing ? (size_t)written : sizeof missing - 1;
    }
    kiln_worker_token tokens[6];
    memset(tokens, 0, sizeof tokens);
    tokens[0].kind = 3; tokens[0].number = 0;
    tokens[1].kind = 3; tokens[1].number = 19;
    tokens[2].kind = 4; tokens[2].data = error_name; tokens[2].len = sizeof error_name - 1;
    tokens[3].kind = 4; tokens[3].data = text; tokens[3].len = (uint64_t)length;
    tokens[4].kind = 3; tokens[4].number = 0;
    tokens[5].kind = 3; tokens[5].number = 0;
    (void)kiln_worker_post(15, 0, 3, tokens, 6);
}

/* The caller installs the worker protocol scope before begin and on each later turn. `scheduled`
 * selects the provider's one-turn drive; a continuous worker still uses the same session shape. */
static void *kiln_cli_worker_begin(const uint8_t *entry, uint64_t entry_len, int32_t eval,
                                    uint64_t argc, const char *const *argv, int scheduled) {
    kiln_cli_worker_session *session = (kiln_cli_worker_session *)calloc(1, sizeof *session);
    long index = eval ? -1 : kiln_graph_module_index(entry, entry_len);
    if (session == NULL) return NULL;
    if (index < 0) {
        kiln_cli_worker_bad_entry(eval, entry, entry_len);
        session->failed_begin = 1;
        return session;
    }
    session->worker_entry = (size_t)index + 1;
    session->run = (kiln_cli_realm_run *)calloc(1, sizeof *session->run);
    if (session->run == NULL) { session->failed_begin = 1; return session; }
    session->out = kiln_cli_worker_stream(1);
    session->err = kiln_cli_worker_stream(2);
    /* The worker's `process.argv` as node spells it names its script second; a native realm's
     * names the executable in both leading slots (the single-executable convention), so the
     * script's slot is left out and the realm fills it the way it fills a main realm's. */
    if (argc >= 2) {
        const char **realm_argv = (const char **)calloc((size_t)argc, sizeof *realm_argv);
        uint64_t at;
        if (realm_argv == NULL) { session->failed_begin = 1; return session; }
        realm_argv[0] = argv[0];
        for (at = 2; at < argc; ++at) realm_argv[at - 1] = argv[at];
        kiln_cli_realm_begin(session->run, (int)(argc - 1), (char **)(uintptr_t)realm_argv,
                             session->out, session->err, session->worker_entry);
        free(realm_argv);
    } else {
        kiln_cli_realm_begin(session->run, (int)argc, (char **)(uintptr_t)argv,
                             session->out, session->err, session->worker_entry);
    }
    session->run->scheduled = scheduled;
    return session;
}

static void *kiln_cli_worker_begin_scheduled(const uint8_t *entry, uint64_t entry_len,
                                              int32_t eval, uint64_t argc,
                                              const char *const *argv) {
    return kiln_cli_worker_begin(entry, entry_len, eval, argc, argv, 1);
}

static uint64_t kiln_cli_worker_identify(void *opaque) {
    kiln_cli_worker_session *session = (kiln_cli_worker_session *)opaque;
    return session->run == NULL ? 0 : session->run->realm;
}

static int kiln_cli_worker_advance(void *opaque, uint64_t *delay_ns) {
    kiln_cli_worker_session *session = (kiln_cli_worker_session *)opaque;
    if (session->failed_begin) { *delay_ns = 0; return 0; }
    int progress = kiln_cli_realm_step(session->run);
    *delay_ns = session->run->park_delay_ns;
    return progress;
}

static int kiln_cli_worker_finish(void *opaque) {
    kiln_cli_worker_session *session = (kiln_cli_worker_session *)opaque;
    int status = session->failed_begin ? 1 : kiln_cli_realm_finish(session->run, 0,
                                                                    session->worker_entry);
    if (session->failed_begin) free(session->run);
    if (session->out != NULL) fclose(session->out);
    if (session->err != NULL) fclose(session->err);
    free(session);
    return status;
}

/* The continuous worker path drives the same session to completion on its dedicated thread.
 * The scheduler will call begin, advance and finish on separate turns. */
static int kiln_cli_run_worker(const uint8_t *entry, uint64_t entry_len, int32_t eval,
                               uint64_t argc, const char *const *argv) {
    void *session = kiln_cli_worker_begin(entry, entry_len, eval, argc, argv, 0);
    uint64_t delay_ns = 0;
    if (session == NULL) return 1;
    while (kiln_cli_worker_advance(session, &delay_ns)) {}
    return kiln_cli_worker_finish(session);
}

/* `KILN_REALMS=N`: run the program as N realms at once in this one process, each pinned to its
 * own thread of the runtime's worker pool (`kiln_realm_run_pinned`, realms issue 05) -- the
 * harness that checks realms running side by side each print exactly what the program prints
 * alone (`scripts/native-realms.sh`). Each realm's standard streams, both into one file as a
 * shell's `2>&1` would, go to `$KILN_REALMS_OUT/<index>.out`, and its exit status to
 * `<index>.status`. A realm calling `process.exit` still ends the whole process. */
typedef struct kiln_cli_realms {
    int argc;
    char **argv;
    const char *directory;
} kiln_cli_realms;

static int kiln_cli_run_hosted_realm(void *context, uint64_t index) {
    const kiln_cli_realms *realms = (const kiln_cli_realms *)context;
    char path[4096];
    FILE *output, *written;
    int status;
    snprintf(path, sizeof path, "%s/%llu.out", realms->directory, (unsigned long long)index);
    output = fopen(path, "wb");
    if (output == NULL) return 1;
    status = kiln_cli_run_realm(realms->argc, realms->argv, 0, output);
    if (fclose(output) != 0 && status == 0) status = 1;
    snprintf(path, sizeof path, "%s/%llu.status", realms->directory, (unsigned long long)index);
    written = fopen(path, "wb");
    if (written == NULL || fprintf(written, "%d\n", status) < 0 || fclose(written) != 0) return 1;
    return status;
}

typedef struct kiln_cli_scheduled_realm {
    kiln_cli_realm_run *run;
    const kiln_cli_realms *realms;
    FILE *output;
    char *captured;
    size_t captured_len;
    size_t captured_cap;
    int capture_failed;
    int memory_output;
    uint64_t index;
#if !defined(_WIN32)
    char stream_buffer[256];
#endif
} kiln_cli_scheduled_realm;

/* An idle scheduled realm should retain only the bytes it has printed. `open_memstream` reserves
 * an 8-KiB buffer for even a three-byte "hi\n", and the parity harness can park thousands of
 * realms at once. Keep FILE* as the host ABI, give stdio a small buffer owned by the session,
 * and append flushed writes into a growable capture. The session outlives fclose. */
static int kiln_cli_scheduled_append(kiln_cli_scheduled_realm *session,
                                     const char *bytes, size_t length) {
    size_t needed, cap;
    char *grown;
    if (length == 0) return 1;
    if (length > SIZE_MAX - session->captured_len) goto failed;
    needed = session->captured_len + length;
    if (needed > session->captured_cap) {
        cap = session->captured_cap == 0 ? 64 : session->captured_cap;
        while (cap < needed) {
            if (cap > SIZE_MAX / 2) { cap = needed; break; }
            cap *= 2;
        }
        grown = (char *)realloc(session->captured, cap);
        if (grown == NULL) goto failed;
        session->captured = grown;
        session->captured_cap = cap;
    }
    memcpy(session->captured + session->captured_len, bytes, length);
    session->captured_len = needed;
    return 1;
failed:
    session->capture_failed = 1;
    errno = ENOMEM;
    return 0;
}

#if defined(__APPLE__)
static int kiln_cli_scheduled_write(void *cookie, const char *bytes, int length) {
    if (length < 0 || !kiln_cli_scheduled_append((kiln_cli_scheduled_realm *)cookie,
                                                 bytes, (size_t)length)) return -1;
    return length;
}
static FILE *kiln_cli_scheduled_stream(kiln_cli_scheduled_realm *session) {
    FILE *file = funopen(session, NULL, kiln_cli_scheduled_write, NULL, NULL);
    if (file != NULL && setvbuf(file, session->stream_buffer, _IOFBF,
                               sizeof session->stream_buffer) != 0) { fclose(file); return NULL; }
    return file;
}
#elif !defined(_WIN32)
static ssize_t kiln_cli_scheduled_write(void *cookie, const char *bytes, size_t length) {
    if (!kiln_cli_scheduled_append((kiln_cli_scheduled_realm *)cookie, bytes, length)) return -1;
    return (ssize_t)length;
}
static FILE *kiln_cli_scheduled_stream(kiln_cli_scheduled_realm *session) {
    cookie_io_functions_t functions = {NULL, kiln_cli_scheduled_write, NULL, NULL};
    FILE *file = fopencookie(session, "w", functions);
    if (file != NULL && setvbuf(file, session->stream_buffer, _IOFBF,
                               sizeof session->stream_buffer) != 0) { fclose(file); return NULL; }
    return file;
}
#endif

static void *kiln_cli_scheduled_begin(void *context, uint64_t index) {
    const kiln_cli_realms *realms = (const kiln_cli_realms *)context;
    kiln_cli_scheduled_realm *session = (kiln_cli_scheduled_realm *)calloc(1, sizeof *session);
    char path[4096];
    if (session == NULL) return NULL;
    session->run = (kiln_cli_realm_run *)calloc(1, sizeof *session->run);
    if (session->run == NULL) { free(session); return NULL; }
    snprintf(path, sizeof path, "%s/%llu.out", realms->directory, (unsigned long long)index);
    /* `KILN_REALMS` is a parity harness: keep each realm's output distinct, but do not hold
     * one file descriptor per idle realm. A hundred thousand parked realms must use the scheduler's
     * fixed lane count rather than spending their descriptor budget on capture files. */
#if defined(_WIN32)
    session->output = fopen(path, "wb");
#else
    session->output = kiln_cli_scheduled_stream(session);
    session->memory_output = session->output != NULL;
#endif
    if (session->output == NULL) {
        free(session->captured);
        free(session->run);
        free(session);
        return NULL;
    }
    session->realms = realms;
    session->index = index;
    kiln_cli_realm_begin(session->run, realms->argc, realms->argv, session->output, NULL, 0);
    session->run->scheduled = 1;
    return session;
}

static uint64_t kiln_cli_scheduled_identify(void *opaque) {
    kiln_cli_scheduled_realm *session = (kiln_cli_scheduled_realm *)opaque;
    return session->run->realm;
}

static int kiln_cli_scheduled_advance(void *opaque, uint64_t *delay_ns) {
    kiln_cli_scheduled_realm *session = (kiln_cli_scheduled_realm *)opaque;
    int progress = kiln_cli_realm_step(session->run);
    *delay_ns = session->run->park_delay_ns;
    return progress;
}

static int kiln_cli_scheduled_finish(void *opaque) {
    kiln_cli_scheduled_realm *session = (kiln_cli_scheduled_realm *)opaque;
    char path[4096];
    FILE *written;
    int status = kiln_cli_realm_finish(session->run, 0, 0);
    if (fclose(session->output) != 0 && status == 0) status = 1;
    if (session->capture_failed) status = 1;
    if (session->memory_output) {
        snprintf(path, sizeof path, "%s/%llu.out", session->realms->directory,
                 (unsigned long long)session->index);
        written = fopen(path, "wb");
        if (written == NULL) {
            status = 1;
        } else {
            if (session->captured_len != 0 &&
                fwrite(session->captured, 1, session->captured_len, written) != session->captured_len)
                status = 1;
            if (fclose(written) != 0) status = 1;
        }
    }
    free(session->captured);
    snprintf(path, sizeof path, "%s/%llu.status", session->realms->directory,
             (unsigned long long)session->index);
    written = fopen(path, "wb");
    if (written == NULL) {
        status = 1;
    } else {
        if (fprintf(written, "%d\n", status) < 0) status = 1;
        if (fclose(written) != 0) status = 1;
    }
    free(session);
    return status;
}

static long kiln_cli_scheduler_lanes(void) {
    /* One budget for realm turns and parallel chunks. The explicit parallelism setting also
     * selects a single deterministic lane for sequential comparisons. */
    const char *setting = getenv("KILN_PARALLELISM");
    char *end = NULL;
    long lanes = 0;
    if (setting != NULL && *setting != '\0') {
        errno = 0;
        lanes = strtol(setting, &end, 10);
        if (errno == ERANGE || end == setting || *end != '\0' || lanes < 1) lanes = 1;
    } else {
        setting = getenv("KILN_SCHEDULER_LANES");
        if (setting != NULL && *setting != '\0') {
            errno = 0;
            lanes = strtol(setting, &end, 10);
            if (errno == ERANGE || end == setting || *end != '\0' || lanes < 1) lanes = 0;
        }
    }
#ifndef _WIN32
    if (lanes == 0) {
        long cores = sysconf(_SC_NPROCESSORS_ONLN);
        const char *workers_setting = getenv("KILN_WORKER_COUNT");
        long workers = 1;
        if (workers_setting != NULL && *workers_setting != '\0') {
            end = NULL;
            errno = 0;
            workers = strtol(workers_setting, &end, 10);
            if (errno == ERANGE || end == workers_setting || *end != '\0' || workers < 1) workers = 1;
        }
        lanes = cores > 0 ? cores / workers : 1;
    }
#endif
    if (lanes < 1) lanes = 1;
    return lanes > 1024 ? 1024 : lanes;
}

static int kiln_cli_scheduler_enabled(void) {
    const char *setting = getenv("KILN_REALM_SCHEDULER");
    return setting == NULL || *setting == '\0' || strcmp(setting, "0") != 0;
}

static long kiln_cli_realm_count(void) {
    const char *setting = getenv("KILN_REALMS");
    char *end = NULL;
    long count;
    if (setting == NULL || *setting == '\0') return 0;
    count = strtol(setting, &end, 10);
    if (end == setting || *end != '\0' || count < 1) return 0;
    /* The dedicated-thread launcher retains its bounded thread count. Scheduled realms share
     * fixed lanes, so its acceptance workload can hold 100,000 idle realms. */
    long limit = kiln_cli_scheduler_enabled() ? 100000 : 1024;
    if (count > limit) {
        fprintf(stderr, "kiln: KILN_REALMS=%ld exceeds %ld-realm %s limit\n",
                count, limit, kiln_cli_scheduler_enabled() ? "scheduled" : "pinned");
        return -1;
    }
    return count;
}

static int kiln_cli_run_realms(int argc, char **argv, long count) {
    kiln_cli_realms realms;
    int *statuses;
    int status;
    long index;
    realms.argc = argc;
    realms.argv = argv;
    realms.directory = getenv("KILN_REALMS_OUT");
    if (realms.directory == NULL || *realms.directory == '\0') {
        fputs("kiln: KILN_REALMS needs KILN_REALMS_OUT, a directory for each realm's output\n", stderr);
        return 1;
    }
    statuses = (int *)calloc((size_t)count, sizeof *statuses);
    if (statuses == NULL) return 1;
    if (kiln_cli_scheduler_enabled()) {
        const char *migrate = getenv("KILN_REALM_MIGRATE");
        if (migrate == NULL || *migrate == '\0' || strcmp(migrate, "0") != 0)
            status = kiln_realm_run_migrating((uint64_t)count,
                        (uint64_t)kiln_cli_scheduler_lanes(), kiln_cli_virtual_clock,
                        kiln_cli_scheduled_begin, kiln_cli_scheduled_identify,
                        kiln_cli_scheduled_advance, kiln_cli_scheduled_finish, &realms, statuses);
        else
            status = kiln_realm_run_scheduled((uint64_t)count,
                        (uint64_t)kiln_cli_scheduler_lanes(), kiln_cli_virtual_clock,
                        kiln_cli_scheduled_begin, kiln_cli_scheduled_advance,
                        kiln_cli_scheduled_finish, &realms, statuses);
    }
    else
        status = kiln_realm_run_pinned((uint64_t)count, kiln_cli_run_hosted_realm, &realms, statuses);
    for (index = 0; index < count; ++index)
        if (statuses[index] != 0) status = 1;
    free(statuses);
    return status == 0 ? 0 : 1;
}

/* How many realms to run, read from the environment at startup.
 *
 * In process mode a server binds with SO_REUSEPORT, so N of these share a port and the kernel
 * spreads the accepts across them; in thread mode (KILN_WORKERS_MODE=thread, `kiln_threads_run`)
 * the realms share one socket and the runtime balances the accepts. Reading the count at startup keeps one artifact correct on every machine it is
 * copied to, which is the point of shipping a binary. `kiln build --workers=<n|auto>` records a
 * default in KILN_CLI_RECORDED_WORKERS, which applies only when KILN_WORKERS is unset or empty;
 * `auto` is still resolved here, on the machine that runs the binary, and `kiln build` warns when a
 * number is recorded because that ties the artifact to the box that built it.
 *
 * "auto" (case-insensitive) queries online CPU cores via sysconf(_SC_NPROCESSORS_ONLN).
 * Absent, unparseable or below one means one realm -- exactly the behaviour before this existed. */
static long kiln_cli_worker_count(void) {
    const char *setting = getenv("KILN_WORKERS");
    char *end = NULL;
    long count;
#ifdef KILN_CLI_RECORDED_WORKERS
    if (setting == NULL || *setting == '\0') setting = KILN_CLI_RECORDED_WORKERS;
#endif
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
 * exist (issue 05): KILN_WORKERS_MODE=thread runs the workers that way (`kiln_threads_run`,
 * below). Process mode is still the default, for the crash isolation and restarts below.
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
 *    a default action never fires) -- unless every worker handled it (`process.on('SIGTERM')`,
 *    realms issue 23) and exited by itself, when the supervisor exits as they did: 0 for a clean
 *    drain. A worker receives exactly the signal a single-process server would, so it behaves
 *    exactly as one does. `docker stop` therefore drains every
 *    worker before PID 1 exits and the namespace is torn down. SIGUSR1 (the introspection dump)
 *    is forwarded without stopping anything.
 *  - gives the drain a deadline: a worker still alive KILN_WORKERS_GRACE_MS (default
 *    KILN_SUP_GRACE_MS) after the stop began gets SIGKILL, sent to its process group so whatever
 *    it spawned goes with it -- `docker stop`'s own escalation, one level down.
 *  - tells a crash from an answer. An exit status is the program's answer -- `process.exit(n)`,
 *    an uncaught error's 1, an unsettled top-level await's 13 -- and restarting cannot change it:
 *    0 finishes that worker and leaves the others serving; anything else stops every worker and
 *    becomes the supervisor's own status, the way the same program run alone would have exited.
 *    A death by a signal the supervisor did not send (a segfault, an abort, the OOM killer, an
 *    operator's `kill` of one worker) is a crash, and the worker is restarted with backoff. A
 *    worker that crashes more than KILN_SUP_MAX_STREAK times in a row, each within
 *    KILN_SUP_STABLE_MS of starting, is a crash loop: the supervisor stops the others and exits
 *    128+signal. Nothing is restarted once a stop has begun.
 *  - puts each worker in a process group of its own, so a signal reaches a worker exactly once:
 *    from the supervisor. Sharing the supervisor's group, Ctrl-C at a terminal signals the whole
 *    foreground group and the forward then delivered SIGINT a second time; telling the two
 *    copies apart needs the terminal's foreground group and siginfo codes that differ between
 *    kernels, and a `kill -INT -<group>` from a process manager is indistinguishable from the
 *    terminal. With one path there is nothing to tell apart, and the SIGKILL above can take a
 *    worker's children by naming its group. The cost is job control: a worker is not in the
 *    terminal's foreground group, so reading the terminal stops it (SIGTTIN) and Ctrl-Z stops
 *    only the supervisor -- neither is something N realms sharing one terminal could use.
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
 * the binary by `kiln build --workers` is a default there, and nothing below cares. */
#ifndef _WIN32
#define KILN_SUP_MAX_STREAK 5
#define KILN_SUP_STABLE_MS 5000LL
#define KILN_SUP_BACKOFF_MS 100LL
#define KILN_SUP_BACKOFF_CAP_MS 2000LL
#define KILN_SUP_GRACE_MS 10000LL
/* Which mechanism `KILN_WORKERS` > 1 uses when KILN_WORKERS_MODE is unset: 1 threads, 0 processes.
 * Processes, as measured in realms issue 07 (2026-09-27, 16 threads, bench/server/serve.mjs):
 * thread mode matched process mode's throughput at 32 connections and above, was 8% slower at 16
 * and had a higher p99 at 64, and saved 11 MB of PSS over 16 workers (0.7 MB each) -- not enough
 * to give up a supervisor that restarts a crashed worker. Revisit once the realms share their
 * built-ins (issue 08), which is where thread mode's memory argument is. */
#define KILN_CLI_DEFAULT_WORKERS_THREADS 0

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

/* How long a stopping worker has before SIGKILL. Zero kills with the forwarded signal. */
static long long kiln_sup_grace_ms(void) {
    const char *setting = getenv("KILN_WORKERS_GRACE_MS");
    char *end = NULL;
    long long grace;
    if (setting == NULL || *setting == '\0') return KILN_SUP_GRACE_MS;
    errno = 0;
    grace = strtoll(setting, &end, 10);
    if (end == setting || *end != '\0' || errno != 0 || grace < 0) {
        fprintf(stderr, "kiln: ignoring KILN_WORKERS_GRACE_MS=%s (expected milliseconds); using %lld\n",
                setting, KILN_SUP_GRACE_MS);
        return KILN_SUP_GRACE_MS;
    }
    /* A day is "never" for a drain, and keeps the deadline arithmetic far from overflow. */
    return grace > 86400000LL ? 86400000LL : grace;
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
    /* Its own process group, before anything can signal it; the supervisor makes the same call
     * from its side of the fork so neither order leaves a window. */
    (void)setpgid(0, 0);
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
    exit(kiln_cli_run_realm(argc, argv, index == 0, NULL));
}

static void kiln_sup_forward(const pid_t *pids, long workers, int sig) {
    long index;
    for (index = 0; index < workers; ++index)
        if (pids[index] > 0) kill(pids[index], sig);
}

/* Begin (or continue) a stop: forward `sig`, and arm the SIGKILL deadline the first time. A
 * deadline of -1 is unarmed and -2 already spent. */
static void kiln_sup_stop(const pid_t *pids, long workers, int sig, long long grace_ms,
                          long long *kill_at) {
    kiln_sup_forward(pids, workers, sig);
    if (*kill_at == -1) *kill_at = kiln_sup_now_ms() + grace_ms;
}

static void kiln_sup_kill_stragglers(const pid_t *pids, long workers, long long grace_ms) {
    long index;
    for (index = 0; index < workers; ++index) {
        if (pids[index] <= 0) continue;
        fprintf(stderr, "kiln: worker %ld (pid %ld) did not stop within %lld ms; sending SIGKILL\n",
                index, (long)pids[index], grace_ms);
        if (kill(-pids[index], SIGKILL) != 0) kill(pids[index], SIGKILL);
    }
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
    /* After a forwarded stop signal: whether any worker died of a signal (it had no listener, or
     * the grace deadline killed it), and the first non-zero status of one that exited on its own
     * (a listener drained it, realms issue 23). */
    int drained_by_signal = 0, drained_status = 0;
    long long grace_ms = kiln_sup_grace_ms(), kill_at = -1;
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
                if (pid > 0) (void)setpgid(pid, pid);
                sigprocmask(SIG_SETMASK, &original, NULL);
                if (pid < 0) {
                    long long delay;
                    int fork_error = errno;
                    streak[index] += 1;
                    if (streak[index] > KILN_SUP_MAX_STREAK) {
                        fprintf(stderr, "kiln: cannot fork worker %ld: %s; giving up\n", index, strerror(fork_error));
                        exit_status = 1;
                        stopping = 1;
                        kiln_sup_stop(pids, workers, SIGTERM, grace_ms, &kill_at);
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
        if (stopping && live > 0 && kill_at >= 0) {
            if (now >= kill_at) {
                kiln_sup_kill_stragglers(pids, workers, grace_ms);
                kill_at = -2;
            } else {
                wait_ms = kill_at - now;
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
                    kiln_sup_stop(pids, workers, sig, grace_ms, &kill_at);
                }
            }
        }
        if (!stopping && kiln_sup_stop_signal != 0) {
            stop_signal = (int)kiln_sup_stop_signal;
            stopping = 1;
            kiln_sup_stop(pids, workers, stop_signal, grace_ms, &kill_at);
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
            /* The drain: whatever a worker dies of now, it was asked to stop. */
            if (stopping) {
                if (WIFSIGNALED(status)) drained_by_signal = 1;
                else if (WIFEXITED(status) && WEXITSTATUS(status) != 0 && drained_status == 0)
                    drained_status = WEXITSTATUS(status);
                continue;
            }
            if (WIFEXITED(status)) {
                if (WEXITSTATUS(status) == 0) continue;
                fprintf(stderr, "kiln: worker %ld (pid %ld) exited with status %d; stopping the other workers\n",
                        index, (long)pid, WEXITSTATUS(status));
                exit_status = WEXITSTATUS(status);
                stopping = 1;
                kiln_sup_stop(pids, workers, SIGTERM, grace_ms, &kill_at);
                continue;
            }
            lived = now - started[index];
            if (lived >= KILN_SUP_STABLE_MS) streak[index] = 0;
            streak[index] += 1;
            if (streak[index] > KILN_SUP_MAX_STREAK) {
                fprintf(stderr, "kiln: worker %ld (pid %ld) was killed by signal %d, crash %d in a row within %lld ms of starting; giving up\n",
                        index, (long)pid, WTERMSIG(status), streak[index], KILN_SUP_STABLE_MS);
                exit_status = 128 + WTERMSIG(status);
                stopping = 1;
                kiln_sup_stop(pids, workers, SIGTERM, grace_ms, &kill_at);
                continue;
            }
            delay = KILN_SUP_BACKOFF_MS << (streak[index] - 1);
            if (delay > KILN_SUP_BACKOFF_CAP_MS) delay = KILN_SUP_BACKOFF_CAP_MS;
            fprintf(stderr, "kiln: worker %ld (pid %ld) was killed by signal %d after %lld ms; restarting in %lld ms\n",
                    index, (long)pid, WTERMSIG(status), lived, delay);
            restart_at[index] = now + delay;
        }
    }

    free(pids);
    free(started);
    free(restart_at);
    free(streak);
    /* Every worker handled the signal (`process.on('SIGTERM')`) and exited by itself: the
     * supervisor ends as they did -- 0 for a clean drain -- as one process that handled it
     * would. */
    if (stop_signal != 0 && !drained_by_signal) return drained_status;
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

/* `KILN_WORKERS` thread mode (realms issue 07): the N workers are N realms of the program in
 * this one process, each pinned to its own thread of the runtime's worker pool
 * (`kiln_realm_run_workers`), instead of N forked processes under a supervisor. What each part
 * of the supervisor becomes:
 *
 *  - Accepts. The realm threads join the runtime's listen group, so the first realm to listen
 *    on an address binds the one socket and every realm listening there is a member of it; the
 *    runtime hands each accepted connection to the member with the fewest open connections
 *    (round-robin among ties) rather than the kernel hashing it over N SO_REUSEPORT sockets.
 *  - Signals. SIGTERM, SIGINT, SIGHUP and SIGQUIT are blocked in every realm thread and taken by
 *    one thread here. A realm listening for the signal (`process.on('SIGTERM')`, realms issue
 *    23) gets it as an event, delivered to every realm that listens, and drains; the process
 *    ends when all of them have finished, or KILN_WORKERS_GRACE_MS after the first such signal
 *    by that signal's default action -- the status the supervisor exits with once its deadline
 *    has killed the workers (143 for SIGTERM). With no listener the thread ends the
 *    process by the same signal -- every realm at once, which is what forwarding the signal to
 *    N workers that each die of it amounts to -- or with 128+signal as PID 1, where a default
 *    action never fires. A realm without a listener beside realms with one keeps running until
 *    they finish or the deadline passes. Other signals go to listening realms through the
 *    runtime's own handler.
 *  - Exit statuses, by the supervisor's rule: a realm that finishes with 0 is done and the
 *    others keep serving; one that finishes with anything else (an uncaught error's 1, an
 *    unsettled top-level await's 13) ends the process with that status at once.
 *    `process.exit(n)` ends its realm with n (the runtime records it and the realm unwinds to
 *    `kiln_cli_run_realm_as`), and the same rule applies to n, as it would to one worker there.
 *  - Crashes. There is nothing to restart: a realm that crashes (a segfault, an abort, the OOM
 *    killer) takes every realm with it, because they are one process. That is what
 *    KILN_WORKERS_MODE=process is for.
 *  - Index. Each realm's own `process.env` has KILN_WORKER_INDEX and KILN_WORKER_COUNT, set
 *    before any module runs; the process environment is not changed. Worker 0 writes the
 *    result file. */
static sigset_t kiln_threads_stop_signals;

/* End the process the way the stop signal `sig` would have: its default action, or 128+sig as
 * PID 1, where a default action never fires. */
static void kiln_threads_die_of(int sig) {
    fflush(stdout);
    fflush(stderr);
    if (getpid() == 1) _exit(128 + sig);
    {
        struct sigaction action;
        sigset_t only;
        memset(&action, 0, sizeof action);
        action.sa_handler = SIG_DFL;
        sigemptyset(&action.sa_mask);
        sigaction(sig, &action, NULL);
        sigemptyset(&only);
        sigaddset(&only, sig);
        pthread_sigmask(SIG_UNBLOCK, &only, NULL);
        raise(sig);
    }
    _exit(128 + sig);
}

/* The stop signal the realms took, for the grace deadline below. */
static volatile sig_atomic_t kiln_threads_grace_signal = SIGTERM;

/* The grace deadline after a stop signal the realms took (realms issue 23): as process mode's
 * supervisor does for its workers, a process whose realms have not all finished by then is
 * ended, says so, and exits as the signal would have made it (143 for SIGTERM). */
static void *kiln_threads_grace(void *argument) {
    long long grace_ms = (long long)(intptr_t)argument;
    struct timespec rest;
    rest.tv_sec = (time_t)(grace_ms / 1000);
    rest.tv_nsec = (long)(grace_ms % 1000) * 1000000L;
    while (nanosleep(&rest, &rest) != 0 && errno == EINTR) {
    }
    fprintf(stderr, "kiln: workers did not stop within %lld ms; ending the process\n", grace_ms);
    kiln_threads_die_of((int)kiln_threads_grace_signal);
    return NULL;
}

static void *kiln_threads_wait_for_stop(void *argument) {
    int sig = 0;
    int grace_armed = 0;
    (void)argument;
    for (;;) {
        uint64_t taken[2] = {0, 0};
        while (sigwait(&kiln_threads_stop_signals, &sig) != 0) {
        }
        /* A realm listening for the signal (`process.on('SIGTERM')`) takes it as an event and
         * drains; the process then ends when every realm has finished, as N process-mode workers
         * that each handled it would, or at the grace deadline. With no listener anywhere, the
         * signal does what it does to one process. */
        if (kiln_worker_op(20, (uint64_t)sig, taken) == 0 && taken[0] > 0) {
            if (!grace_armed) {
                pthread_t timer;
                pthread_attr_t attributes;
                long long grace_ms = kiln_sup_grace_ms();
                grace_armed = 1;
                kiln_threads_grace_signal = sig;
                if (grace_ms == 0) {
                    fprintf(stderr, "kiln: workers did not stop within 0 ms; ending the process\n");
                    kiln_threads_die_of(sig);
                }
                if (pthread_attr_init(&attributes) == 0) {
                    pthread_attr_setdetachstate(&attributes, PTHREAD_CREATE_DETACHED);
                    if (pthread_create(&timer, &attributes, kiln_threads_grace,
                                       (void *)(intptr_t)grace_ms) != 0)
                        fputs("kiln: cannot start the grace timer; workers may outlive it\n", stderr);
                    pthread_attr_destroy(&attributes);
                }
            }
            continue;
        }
        break;
    }
    kiln_threads_die_of(sig);
    return NULL;
}

typedef struct kiln_threads {
    int argc;
    char **argv;
} kiln_threads;

static int kiln_threads_run_worker(void *context, uint64_t index) {
    const kiln_threads *threads = (const kiln_threads *)context;
    int status;
    kiln_cli_thread_worker = (long)index;
    status = kiln_cli_run_realm(threads->argc, threads->argv, index == 0, NULL);
    if (status != 0) {
        fprintf(stderr, "kiln: worker %llu exited with status %d; stopping the other workers\n",
                (unsigned long long)index, status);
        fflush(stdout);
        fflush(stderr);
        _exit(status);
    }
    return 0;
}

static int kiln_threads_run(int argc, char **argv, long workers) {
    kiln_threads threads;
    pthread_t waiter;
    pthread_attr_t attributes;
    int *statuses;
    int status;
    long index;
    threads.argc = argc;
    threads.argv = argv;
    kiln_cli_thread_workers = workers;
    sigemptyset(&kiln_threads_stop_signals);
    sigaddset(&kiln_threads_stop_signals, SIGTERM);
    sigaddset(&kiln_threads_stop_signals, SIGINT);
    sigaddset(&kiln_threads_stop_signals, SIGHUP);
    sigaddset(&kiln_threads_stop_signals, SIGQUIT);
    /* Blocked before any realm thread exists, so every one of them inherits the mask and the
     * waiter below is the only thread a stop signal can be delivered to. */
    pthread_sigmask(SIG_BLOCK, &kiln_threads_stop_signals, NULL);
    if (pthread_attr_init(&attributes) != 0) return 1;
    pthread_attr_setdetachstate(&attributes, PTHREAD_CREATE_DETACHED);
    if (pthread_create(&waiter, &attributes, kiln_threads_wait_for_stop, NULL) != 0) {
        pthread_attr_destroy(&attributes);
        fputs("kiln: cannot start the signal thread for KILN_WORKERS thread mode\n", stderr);
        return 1;
    }
    pthread_attr_destroy(&attributes);
    statuses = (int *)calloc((size_t)workers, sizeof *statuses);
    if (statuses == NULL) return 1;
    status = kiln_realm_run_workers((uint64_t)workers, kiln_threads_run_worker, &threads, statuses);
    for (index = 0; index < workers; ++index)
        if (statuses[index] != 0) status = statuses[index];
    free(statuses);
    return status == 0 ? 0 : 1;
}

/* Which mechanism N workers use: KILN_WORKERS_MODE=thread or process (case-insensitive). Unset
 * or empty is the default; anything else is reported once and the default used. */
static int kiln_cli_workers_use_threads(void) {
    const char *setting = getenv("KILN_WORKERS_MODE");
    if (setting == NULL || *setting == '\0') return KILN_CLI_DEFAULT_WORKERS_THREADS;
    if (strcasecmp(setting, "thread") == 0 || strcasecmp(setting, "threads") == 0) return 1;
    if (strcasecmp(setting, "process") == 0 || strcasecmp(setting, "processes") == 0) return 0;
    fprintf(stderr, "kiln: ignoring KILN_WORKERS_MODE=%s (expected thread or process); using %s\n",
            setting, KILN_CLI_DEFAULT_WORKERS_THREADS ? "thread" : "process");
    return KILN_CLI_DEFAULT_WORKERS_THREADS;
}
#endif

/* Parallel issue 30: compiled code recurses on the C stack, as deeply as the interpreter's
 * 10,000 calls, so the main thread gets the 64 MiB a `node:worker_threads` realm's thread gets.
 * Linux grows the main stack on demand up to the soft `RLIMIT_STACK` read at each fault, so
 * raising the soft limit (never past the hard one, never lowering it) is enough; macOS fixes the
 * main stack at link time instead (`main_stack_flags` in kiln-native-toolchain). */
static void kiln_cli_grow_main_stack(void) {
#if defined(__linux__)
    const rlim_t wanted = (rlim_t)64 * 1024 * 1024;
    struct rlimit limit;
    if (getrlimit(RLIMIT_STACK, &limit) != 0) return;
    if (limit.rlim_cur == RLIM_INFINITY || limit.rlim_cur >= wanted) return;
    limit.rlim_cur = limit.rlim_max == RLIM_INFINITY || limit.rlim_max >= wanted ? wanted : limit.rlim_max;
    (void)setrlimit(RLIMIT_STACK, &limit);
#endif
}

int main(int argc, char **argv) {
    long workers, realms;
    kiln_cli_grow_main_stack();
    if (setlocale(LC_NUMERIC, "C") == NULL) return 1;
    if (kiln_realm_set_graph_control(kiln_c99_graph_paused_control) != 0) return 1;
    if (kiln_cli_http3_enabled(argc, argv)) {
#ifndef _WIN32
        setenv("KILN_HTTP3", "1", 1);
#else
        _putenv("KILN_HTTP3=1");
#endif
    }
    (void)kiln_worker_set_runner(kiln_cli_run_worker);
    (void)kiln_worker_set_turn_runner(kiln_cli_worker_begin_scheduled,
                                      kiln_cli_worker_identify, kiln_cli_worker_advance,
                                      kiln_cli_worker_finish);
    realms = kiln_cli_realm_count();
    if (realms < 0) return 1;
    if (realms > 0) return kiln_cli_run_realms(argc, argv, realms);
    workers = kiln_cli_worker_count();
#ifdef _WIN32
    (void)workers;
    return kiln_cli_run_realm(argc, argv, 1, NULL);
#else
    if (workers <= 1) return kiln_cli_run_realm(argc, argv, 1, NULL);
    if (kiln_cli_workers_use_threads()) return kiln_threads_run(argc, argv, workers);
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
