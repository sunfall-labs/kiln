// ECMAScript Iterator helpers and combinators (ADR 0005, ADR 0063 Rule 4).
// Includes:
// - Iterator.concat(...iterables)
// - Iterator.zip(iterables, options?)
// - Iterator.zipKeyed(obj, options?)
// - Iterator.prototype.chunks(size)
// - Iterator.prototype.windows(size, undersized?)
// - Iterator.prototype.join(separator?)
// - Iterator.prototype[Symbol.dispose]()

(function installIteratorBuiltins() {
    if (typeof Iterator === "undefined" || !Iterator.prototype) {
        return;
    }

    const Proto = Iterator.prototype;

    // Built-in methods are not constructors, so each is a method definition (`{ m() {} }.m`); an
    // optional parameter has a default (`x: any = undefined`) so `length` does not count it.
    const defineMethod = (target: any, name: string | symbol, fn: any) => {
        Object.defineProperty(target, name, {
            value: fn,
            writable: true,
            enumerable: false,
            configurable: true,
        });
    };

    // --- The iterator protocol, as the specification spells it ---
    //
    // `step` reads a record's `next` result (`IteratorStepValue`): a throw from `next`, a result
    // that is not an object, or a throw from its `done` or `value` getter marks the source done
    // and propagates -- the source failed on its own terms and is never closed for it.
    // `closeIterator` is `IteratorClose` with a normal completion (its errors are the caller's),
    // `closeDiscarding` the one with a throw already in flight (the in-flight throw wins).
    const DONE: any = {};

    function isObject(value: any): boolean {
        return value !== null && (typeof value === "object" || typeof value === "function");
    }

    // `GetIteratorDirect`: `next` is read once, here, and never again.
    function iteratorDirect(obj: any): { iterator: any; next: any; done: boolean } {
        return { iterator: obj, next: obj.next, done: false };
    }

    function step(record: { iterator: any; next: any; done: boolean }): any {
        let result: any;
        try {
            result = record.next.call(record.iterator);
        } catch (error) {
            record.done = true;
            throw error;
        }
        if (!isObject(result)) {
            record.done = true;
            throw new TypeError("Iterator result " + String(result) + " is not an object");
        }
        let done: any;
        let value: any;
        try {
            done = result.done;
            if (done) {
                record.done = true;
                return DONE;
            }
            value = result.value;
        } catch (error) {
            record.done = true;
            throw error;
        }
        return value;
    }

    function closeIterator(iterator: any): void {
        const method = iterator.return;
        if (method === undefined || method === null) return;
        if (typeof method !== "function") {
            throw new TypeError("Iterator return method is not callable");
        }
        const result = method.call(iterator);
        if (!isObject(result)) {
            throw new TypeError("Iterator return result " + String(result) + " is not an object");
        }
    }

    function closeDiscarding(iterator: any): void {
        try {
            const method = iterator.return;
            if (method !== undefined && method !== null) method.call(iterator);
        } catch {
            // The throw already in flight is the one the caller sees.
        }
    }

    // An iterator helper object: `resume` runs the helper's closure to its next yield and
    // answers the value or `DONE`; `close` releases what the helper holds when `return` is
    // called before the closure finished. The four states are a generator's, which is what the
    // specification builds a helper from: a `next` re-entered while the closure is running is a
    // TypeError, and a helper that has completed answers done without touching anything.
    function makeHelper(resume: () => any, close: () => void): any {
        const SUSPENDED = 0;
        const RUNNING = 1;
        const COMPLETED = 2;
        let state = SUSPENDED;
        const methods = {
            next(): any {
                if (state === RUNNING) throw new TypeError("Iterator helper is already running");
                if (state === COMPLETED) return { value: undefined, done: true };
                state = RUNNING;
                let value: any;
                try {
                    value = resume();
                } catch (error) {
                    state = COMPLETED;
                    throw error;
                }
                if (value === DONE) {
                    state = COMPLETED;
                    return { value: undefined, done: true };
                }
                state = SUSPENDED;
                return { value, done: false };
            },
            return(): any {
                if (state === RUNNING) throw new TypeError("Iterator helper is already running");
                if (state === COMPLETED) return { value: undefined, done: true };
                state = COMPLETED;
                close();
                return { value: undefined, done: true };
            },
        };
        const helper = Object.create(Proto);
        Object.defineProperty(helper, "next", { value: methods.next, writable: true, configurable: true });
        Object.defineProperty(helper, "return", { value: methods.return, writable: true, configurable: true });
        return helper;
    }

    // Iterator.concat(...iterables): each argument is checked -- an object with a callable
    // `@@iterator` -- before the next is looked at, and each source is opened only when the one
    // before it is exhausted. A value is handed on in a fresh result object, never the source's.
    const concat = { concat(...items: any[]) {
        const sources: { iterable: any; method: any }[] = [];
        for (let i = 0; i < items.length; i++) {
            const item = items[i];
            if (!isObject(item)) {
                throw new TypeError("Iterator.concat: every argument must be an object");
            }
            const method = item[Symbol.iterator];
            if (typeof method !== "function") {
                throw new TypeError("Iterator.concat: every argument must be iterable");
            }
            sources.push({ iterable: item, method });
        }
        let index = 0;
        let open: { iterator: any; next: any; done: boolean } | null = null;
        return makeHelper(
            () => {
                while (true) {
                    if (open === null) {
                        if (index >= sources.length) return DONE;
                        const source = sources[index++];
                        const iterator = source.method.call(source.iterable);
                        if (!isObject(iterator)) {
                            throw new TypeError("Iterator.concat: @@iterator did not return an object");
                        }
                        open = iteratorDirect(iterator);
                    }
                    const value = step(open);
                    if (value !== DONE) return value;
                    open = null;
                }
            },
            () => {
                const current = open;
                open = null;
                if (current !== null) closeIterator(current.iterator);
            }
        );
    } }.concat;

    // `GetIteratorFlattenable(value, reject-strings)`: an iterable or a bare iterator, never a
    // string primitive.
    function flattenable(value: any): { iterator: any; next: any; done: boolean } {
        if (!isObject(value)) throw new TypeError("Iterator.zip: every iterable must be an object");
        const method = value[Symbol.iterator];
        let iterator: any;
        if (method === undefined || method === null) {
            iterator = value;
        } else if (typeof method === "function") {
            iterator = method.call(value);
        } else {
            throw new TypeError("Iterator.zip: @@iterator is not callable");
        }
        if (!isObject(iterator)) throw new TypeError("Iterator.zip: iterator is not an object");
        return iteratorDirect(iterator);
    }

    // `GetIterator(value, sync)`: the stricter opening, which requires a callable `@@iterator`.
    function opened(value: any): { iterator: any; next: any; done: boolean } {
        const method = value[Symbol.iterator];
        if (typeof method !== "function") throw new TypeError("Iterator.zip: value is not iterable");
        const iterator = method.call(value);
        if (!isObject(iterator)) throw new TypeError("Iterator.zip: iterator is not an object");
        return iteratorDirect(iterator);
    }

    // `IteratorStep` without reading `value`: only whether there is another element.
    function stepDone(record: { iterator: any; next: any; done: boolean }): boolean {
        let result: any;
        try {
            result = record.next.call(record.iterator);
        } catch (error) {
            record.done = true;
            throw error;
        }
        if (!isObject(result)) {
            record.done = true;
            throw new TypeError("Iterator result " + String(result) + " is not an object");
        }
        let done: any;
        try {
            done = result.done;
        } catch (error) {
            record.done = true;
            throw error;
        }
        if (done) record.done = true;
        return !!done;
    }

    // `IteratorCloseAll`: last opened first; with a normal completion the first error is the
    // one reported after every source has been told, with a throw in flight none is.
    function closeAll(records: { iterator: any }[], discarding: boolean): void {
        let failed = false;
        let first: any = undefined;
        for (let i = records.length - 1; i >= 0; i--) {
            try {
                closeIterator(records[i].iterator);
            } catch (error) {
                if (!failed) {
                    failed = true;
                    first = error;
                }
            }
        }
        if (failed && !discarding) throw first;
    }

    // `Iterator.zip` and `Iterator.zipKeyed`: every source is opened by the call -- a zipped step
    // needs one element from each -- so `return()` before the first `next()` closes them all.
    // The two differ only in where the sources come from and what a step is shaped like.
    function zipWith(keyed: boolean, iterables: any, options: any): any {
        if (!isObject(iterables)) throw new TypeError("Iterator.zip: iterables must be an object");
        if (options !== undefined && !isObject(options)) {
            throw new TypeError("Iterator.zip: options must be an object");
        }
        const modeOption = options === undefined ? undefined : options.mode;
        let mode: string;
        if (modeOption === undefined) {
            mode = "shortest";
        } else if (modeOption === "shortest" || modeOption === "longest" || modeOption === "strict") {
            mode = modeOption;
        } else {
            throw new TypeError('Iterator.zip: mode must be "shortest", "longest" or "strict"');
        }
        // Read only in the one mode that pads.
        let paddingOption: any = undefined;
        if (mode === "longest") {
            paddingOption = options === undefined ? undefined : options.padding;
            if (paddingOption !== undefined && !isObject(paddingOption)) {
                throw new TypeError("Iterator.zip: padding must be an object");
            }
        }

        const sources: { iterator: any; next: any; done: boolean }[] = [];
        const keys: any[] = [];
        if (keyed) {
            const allKeys = Reflect.ownKeys(iterables);
            for (let i = 0; i < allKeys.length; i++) {
                const key = allKeys[i];
                let value: any;
                try {
                    const desc = Reflect.getOwnPropertyDescriptor(iterables, key);
                    if (desc === undefined || !desc.enumerable) continue;
                    value = iterables[key];
                    // `undefined` reads as "not one of the sources"; anything else must iterate.
                    if (value === undefined) continue;
                    sources.push(flattenable(value));
                } catch (error) {
                    closeAll(sources, true);
                    throw error;
                }
                keys.push(key);
            }
        } else {
            const outer = opened(iterables);
            while (true) {
                let element: any;
                try {
                    element = step(outer);
                } catch (error) {
                    // The outer iterator failed on its own terms; only what this walk opened
                    // is closed.
                    closeAll(sources, true);
                    throw error;
                }
                if (element === DONE) break;
                try {
                    sources.push(flattenable(element));
                } catch (error) {
                    // A bad element is this walk's fault, so the outer iterator is closed too,
                    // last, after everything it produced.
                    closeAll([outer].concat(sources), true);
                    throw error;
                }
            }
        }

        const count = sources.length;
        const padding: any[] = [];
        try {
            if (paddingOption === undefined) {
                for (let i = 0; i < count; i++) padding.push(undefined);
            } else if (keyed) {
                for (let i = 0; i < count; i++) padding.push(paddingOption[keys[i]]);
            } else {
                const pad = opened(paddingOption);
                let alive = true;
                for (let i = 0; i < count; i++) {
                    if (alive) {
                        const value = step(pad);
                        if (value !== DONE) {
                            padding.push(value);
                            continue;
                        }
                        alive = false;
                    }
                    padding.push(undefined);
                }
                // Closed only if it had more to give: one that ran out closed itself.
                if (alive) closeIterator(pad.iterator);
            }
        } catch (error) {
            closeAll(sources, true);
            throw error;
        }

        // `iters` is indexed by position and holds null once a source has ended; `open` is what
        // a close sweeps.
        const iters: ({ iterator: any; next: any; done: boolean } | null)[] = sources.slice();
        let open = sources.slice();
        const removeOpen = (record: any) => {
            const at = open.indexOf(record);
            if (at >= 0) open.splice(at, 1);
        };
        const mismatch = () => new TypeError("Iterator.zip: strict mode requires every iterator to have the same length");
        // Strict mode's check that every other source ended on this round too.
        const strictTail = (): any => {
            for (let i = 1; i < count; i++) {
                const record = iters[i];
                if (record === null) continue;
                let ended: boolean;
                try {
                    ended = stepDone(record);
                } catch (error) {
                    removeOpen(record);
                    closeAll(open, true);
                    throw error;
                }
                if (!ended) {
                    closeAll(open, true);
                    throw mismatch();
                }
                removeOpen(record);
            }
            return DONE;
        };
        return makeHelper(
            () => {
                if (count === 0) return DONE;
                const results: any[] = [];
                for (let i = 0; i < count; i++) {
                    const record = iters[i];
                    if (record === null) {
                        results.push(padding[i]);
                        continue;
                    }
                    let value: any;
                    try {
                        value = step(record);
                    } catch (error) {
                        // The failing source is not told to close: it failed on its own terms.
                        removeOpen(record);
                        closeAll(open, true);
                        throw error;
                    }
                    if (value === DONE) {
                        removeOpen(record);
                        if (mode === "shortest") {
                            closeAll(open, false);
                            return DONE;
                        }
                        if (mode === "strict") {
                            if (i !== 0) {
                                closeAll(open, true);
                                throw mismatch();
                            }
                            return strictTail();
                        }
                        if (open.length === 0) return DONE;
                        iters[i] = null;
                        results.push(padding[i]);
                        continue;
                    }
                    results.push(value);
                }
                if (!keyed) return results;
                // A null prototype: a key named `toString` is the source's value and nothing else.
                const out = Object.create(null);
                for (let i = 0; i < count; i++) {
                    Object.defineProperty(out, keys[i], { value: results[i], writable: true, enumerable: true, configurable: true });
                }
                return out;
            },
            () => {
                const remaining = open;
                open = [];
                closeAll(remaining, false);
            }
        );
    }

    // Iterator.zip(iterables, options?)
    const zip = { zip(iterables: any, options: any = undefined) {
        return zipWith(false, iterables, options);
    } }.zip;

    // Iterator.zipKeyed(iterables, options?)
    const zipKeyed = { zipKeyed(iterables: any, options: any = undefined) {
        return zipWith(true, iterables, options);
    } }.zipKeyed;

    // The size `chunks` and `windows` take: not coerced -- `chunks("2")` is a TypeError, not a
    // size of two -- an integral Number, then in range. A bad one closes the receiver.
    function sizeArgument(receiver: any, size: any, name: string): number {
        if (typeof size !== "number") {
            closeDiscarding(receiver);
            throw new TypeError(name + " size must be a Number");
        }
        if (!Number.isFinite(size) || size % 1 !== 0) {
            closeDiscarding(receiver);
            throw new TypeError(name + " size must be an integral Number");
        }
        if (size < 1 || size > 4294967295) {
            closeDiscarding(receiver);
            throw new RangeError(name + " size out of range");
        }
        return size;
    }

    // Iterator.prototype.chunks(size)
    const chunks = { chunks(this: any, size: any) {
        if (!isObject(this)) throw new TypeError("Iterator.prototype.chunks called on a non-object");
        const chunkSize = sizeArgument(this, size, "chunks");
        const record = iteratorDirect(this);
        return makeHelper(
            () => {
                if (record.done) return DONE;
                const buffer: any[] = [];
                while (true) {
                    const value = step(record);
                    // A source that runs out mid-chunk still hands on what it produced.
                    if (value === DONE) return buffer.length > 0 ? buffer : DONE;
                    buffer.push(value);
                    if (buffer.length === chunkSize) return buffer;
                }
            },
            () => closeIterator(record.iterator)
        );
    } }.chunks;

    // Iterator.prototype.windows(size, undersized?)
    const windows = { windows(this: any, size: any, undersized: any = undefined) {
        if (!isObject(this)) throw new TypeError("Iterator.prototype.windows called on a non-object");
        const windowSize = sizeArgument(this, size, "windows");
        // Two spellings and nothing else -- not a value that would stringify to one of them.
        let allowPartial = false;
        if (undersized === "allow-partial") {
            allowPartial = true;
        } else if (undersized !== undefined && undersized !== "only-full") {
            closeDiscarding(this);
            throw new TypeError('windows undersized must be "only-full" or "allow-partial"');
        }
        const record = iteratorDirect(this);
        const buffer: any[] = [];
        return makeHelper(
            () => {
                while (true) {
                    if (record.done) return DONE;
                    const value = step(record);
                    if (value === DONE) {
                        // A short window only when the whole source was shorter than one window:
                        // a buffer that ever filled is never drained.
                        if (allowPartial && buffer.length > 0 && buffer.length < windowSize) {
                            const partial = buffer.slice();
                            buffer.length = 0;
                            return partial;
                        }
                        return DONE;
                    }
                    if (buffer.length === windowSize) buffer.shift();
                    buffer.push(value);
                    if (buffer.length === windowSize) return buffer.slice();
                }
            },
            () => closeIterator(record.iterator)
        );
    } }.windows;

    // Iterator.prototype.join(separator): the separator is converted before `next` is read, and
    // the source is closed for the errors `join` causes itself -- a throwing `toString` on the
    // separator or an element -- and for nothing else.
    const join = { join(this: any, separator: any) {
        if (!isObject(this)) throw new TypeError("Iterator.prototype.join called on a non-object");
        let sep = ",";
        if (separator !== undefined) {
            try {
                sep = `${separator}`;
            } catch (error) {
                closeDiscarding(this);
                throw error;
            }
        }
        const record = iteratorDirect(this);
        let joined = "";
        let first = true;
        while (true) {
            const value = step(record);
            if (value === DONE) return joined;
            if (!first) joined += sep;
            first = false;
            // A nullish element contributes nothing but still gets its separator.
            if (value === null || value === undefined) continue;
            try {
                joined += `${value}`;
            } catch (error) {
                closeDiscarding(this);
                throw error;
            }
        }
    } }.join;

    // Iterator.prototype[Symbol.dispose](): `return`, if there is one, and its errors are the
    // caller's.
    const dispose = { [Symbol.dispose](this: any) {
        const method = this.return;
        if (method !== undefined && method !== null) {
            if (typeof method !== "function") throw new TypeError("Iterator return method is not callable");
            method.call(this);
        }
    } }[Symbol.dispose];

    defineMethod(Iterator, "concat", concat);
    defineMethod(Iterator, "zip", zip);
    defineMethod(Iterator, "zipKeyed", zipKeyed);

    defineMethod(Proto, "chunks", chunks);
    defineMethod(Proto, "windows", windows);
    defineMethod(Proto, "join", join);
    if (typeof Symbol !== "undefined" && Symbol.dispose) {
        defineMethod(Proto, Symbol.dispose, dispose);
    }
    defineMethod(Proto, "@@dispose", dispose);
})();
