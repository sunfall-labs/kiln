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

    const defineMethod = (target: any, name: string | symbol, fn: any) => {
        Object.defineProperty(target, name, {
            value: fn,
            writable: true,
            enumerable: false,
            configurable: true,
        });
    };

    function getIterator(obj: any): { iterator: any; nextMethod: any } {
        if (obj === null || (typeof obj !== "object" && typeof obj !== "function")) {
            throw new TypeError("Value is not iterable");
        }
        let method = typeof Symbol !== "undefined" && Symbol.iterator ? obj[Symbol.iterator] : undefined;
        if (typeof method !== "function") {
            method = obj["@@iterator"];
        }
        if (typeof method !== "function") {
            throw new TypeError("Value is not iterable");
        }
        const iterator = method.call(obj);
        if (iterator === null || (typeof iterator !== "object" && typeof iterator !== "function")) {
            throw new TypeError("Iterator result is not an object");
        }
        const nextMethod = iterator.next;
        if (typeof nextMethod !== "function") {
            throw new TypeError("Iterator has no next method");
        }
        return { iterator, nextMethod };
    }

    function closeIterator(iter: any) {
        if (iter && typeof iter.return === "function") {
            try {
                iter.return();
            } catch {
                // ignore errors during cleanup
            }
        }
    }

    function createHelper(nextFn: () => { done: boolean; value: any }, returnFn?: () => void) {
        const helper: any = {
            next: nextFn,
            return: () => {
                if (returnFn) returnFn();
                return { done: true, value: undefined };
            },
        };
        if (typeof Symbol !== "undefined" && Symbol.iterator) {
            helper[Symbol.iterator] = function () {
                return this;
            };
        }
        helper["@@iterator"] = function () {
            return this;
        };
        try {
            Object.setPrototypeOf(helper, Proto);
        } catch {
            // ignore if setPrototypeOf fails
        }
        return helper;
    }

    // Iterator.concat(...iterables)
    function concat(...iterables: any[]) {
        let currentIndex = 0;
        let currentIter: any = null;
        let done = false;

        return createHelper(
            () => {
                if (done) return { done: true, value: undefined };
                while (currentIndex < iterables.length || currentIter !== null) {
                    if (currentIter === null) {
                        if (currentIndex >= iterables.length) {
                            done = true;
                            return { done: true, value: undefined };
                        }
                        const item = iterables[currentIndex++];
                        const opened = getIterator(item);
                        currentIter = opened;
                    }
                    const res = currentIter.nextMethod.call(currentIter.iterator);
                    if (res && res.done) {
                        currentIter = null;
                        continue;
                    }
                    return res;
                }
                done = true;
                return { done: true, value: undefined };
            },
            () => {
                done = true;
                if (currentIter) {
                    closeIterator(currentIter.iterator);
                    currentIter = null;
                }
            }
        );
    }

    // Iterator.zip(iterables, options?)
    function zip(iterables: any, options?: any) {
        if (iterables === null || (typeof iterables !== "object" && typeof iterables !== "function")) {
            throw new TypeError("Iterator.zip: iterables must be an object");
        }
        if (options !== undefined && (options === null || (typeof options !== "object" && typeof options !== "function"))) {
            throw new TypeError("Iterator.zip: options must be an object");
        }
        const mode = options?.mode ?? "shortest";
        if (mode !== "shortest" && mode !== "longest" && mode !== "strict") {
            throw new TypeError('Iterator.zip: mode must be "shortest", "longest" or "strict"');
        }
        let paddingOption = options?.padding;
        if (mode === "longest") {
            if (paddingOption !== undefined && (paddingOption === null || (typeof paddingOption !== "object" && typeof paddingOption !== "function"))) {
                throw new TypeError("Iterator.zip: padding must be an object");
            }
        }

        // Open positional sources
        const sources: { iterator: any; nextMethod: any }[] = [];
        try {
            const outer = getIterator(iterables);
            while (true) {
                const step = outer.nextMethod.call(outer.iterator);
                if (step && step.done) break;
                const inner = getIterator(step.value);
                sources.push(inner);
            }
        } catch (err) {
            for (const s of sources) closeIterator(s.iterator);
            throw err;
        }

        const count = sources.length;
        const padding: any[] = [];
        if (mode === "longest") {
            if (paddingOption !== undefined) {
                try {
                    const padIter = getIterator(paddingOption);
                    for (let i = 0; i < count; i++) {
                        const step = padIter.nextMethod.call(padIter.iterator);
                        if (step && !step.done) {
                            padding.push(step.value);
                        } else {
                            padding.push(undefined);
                        }
                    }
                    closeIterator(padIter.iterator);
                } catch (err) {
                    for (const s of sources) closeIterator(s.iterator);
                    throw err;
                }
            } else {
                for (let i = 0; i < count; i++) padding.push(undefined);
            }
        }

        let done = count === 0;
        const ended: boolean[] = new Array(count).fill(false);
        let endedCount = 0;

        return createHelper(
            () => {
                if (done) return { done: true, value: undefined };
                const result = new Array(count);
                let firstEndedThisRound = false;

                for (let i = 0; i < count; i++) {
                    if (ended[i]) {
                        result[i] = padding[i];
                        continue;
                    }
                    let step;
                    try {
                        step = sources[i].nextMethod.call(sources[i].iterator);
                    } catch (err) {
                        done = true;
                        for (let j = 0; j < count; j++) {
                            if (!ended[j]) closeIterator(sources[j].iterator);
                        }
                        throw err;
                    }
                    if (step && step.done) {
                        ended[i] = true;
                        endedCount++;
                        if (mode === "shortest") {
                            done = true;
                            for (let j = 0; j < count; j++) {
                                if (!ended[j]) closeIterator(sources[j].iterator);
                            }
                            return { done: true, value: undefined };
                        }
                        if (mode === "strict") {
                            done = true;
                            for (let j = 0; j < count; j++) {
                                if (!ended[j]) closeIterator(sources[j].iterator);
                            }
                            if (i > 0 || firstEndedThisRound) {
                                throw new RangeError("Iterator.zip: iterators have different lengths");
                            }
                            firstEndedThisRound = true;
                        }
                        result[i] = padding[i];
                    } else {
                        if (mode === "strict" && firstEndedThisRound) {
                            done = true;
                            for (let j = 0; j < count; j++) {
                                if (!ended[j]) closeIterator(sources[j].iterator);
                            }
                            throw new RangeError("Iterator.zip: iterators have different lengths");
                        }
                        result[i] = step ? step.value : undefined;
                    }
                }

                if (mode === "strict" && firstEndedThisRound) {
                    return { done: true, value: undefined };
                }

                if (mode === "longest" && endedCount === count) {
                    done = true;
                    return { done: true, value: undefined };
                }

                return { done: false, value: result };
            },
            () => {
                done = true;
                for (let i = 0; i < count; i++) {
                    if (!ended[i]) closeIterator(sources[i].iterator);
                }
            }
        );
    }

    // Iterator.zipKeyed(obj, options?)
    function zipKeyed(obj: any, options?: any) {
        if (obj === null || (typeof obj !== "object" && typeof obj !== "function")) {
            throw new TypeError("Iterator.zip: iterables must be an object");
        }
        if (options !== undefined && (options === null || (typeof options !== "object" && typeof options !== "function"))) {
            throw new TypeError("Iterator.zip: options must be an object");
        }
        const mode = options?.mode ?? "shortest";
        if (mode !== "shortest" && mode !== "longest" && mode !== "strict") {
            throw new TypeError('Iterator.zip: mode must be "shortest", "longest" or "strict"');
        }
        let paddingOption = options?.padding;
        if (mode === "longest") {
            if (paddingOption !== undefined && (paddingOption === null || (typeof paddingOption !== "object" && typeof paddingOption !== "function"))) {
                throw new TypeError("Iterator.zip: padding must be an object");
            }
        }

        const keys: (string | symbol)[] = [];
        const sources: { iterator: any; nextMethod: any }[] = [];
        try {
            const ownKeys = Reflect.ownKeys(obj);
            for (const key of ownKeys) {
                const desc = Object.getOwnPropertyDescriptor(obj, key);
                if (!desc || !desc.enumerable) continue;
                const val = obj[key];
                if (val === undefined) continue;
                const inner = getIterator(val);
                keys.push(key);
                sources.push(inner);
            }
        } catch (err) {
            for (const s of sources) closeIterator(s.iterator);
            throw err;
        }

        const count = sources.length;
        const padding: any[] = [];
        if (mode === "longest") {
            for (let i = 0; i < count; i++) {
                padding.push(paddingOption !== undefined ? paddingOption[keys[i]] : undefined);
            }
        }

        let done = count === 0;
        const ended: boolean[] = new Array(count).fill(false);
        let endedCount = 0;

        return createHelper(
            () => {
                if (done) return { done: true, value: undefined };
                const result = Object.create(null);
                let firstEndedThisRound = false;

                for (let i = 0; i < count; i++) {
                    const key = keys[i];
                    if (ended[i]) {
                        result[key] = padding[i];
                        continue;
                    }
                    let step;
                    try {
                        step = sources[i].nextMethod.call(sources[i].iterator);
                    } catch (err) {
                        done = true;
                        for (let j = 0; j < count; j++) {
                            if (!ended[j]) closeIterator(sources[j].iterator);
                        }
                        throw err;
                    }
                    if (step && step.done) {
                        ended[i] = true;
                        endedCount++;
                        if (mode === "shortest") {
                            done = true;
                            for (let j = 0; j < count; j++) {
                                if (!ended[j]) closeIterator(sources[j].iterator);
                            }
                            return { done: true, value: undefined };
                        }
                        if (mode === "strict") {
                            done = true;
                            for (let j = 0; j < count; j++) {
                                if (!ended[j]) closeIterator(sources[j].iterator);
                            }
                            if (i > 0 || firstEndedThisRound) {
                                throw new RangeError("Iterator.zip: iterators have different lengths");
                            }
                            firstEndedThisRound = true;
                        }
                        result[key] = padding[i];
                    } else {
                        if (mode === "strict" && firstEndedThisRound) {
                            done = true;
                            for (let j = 0; j < count; j++) {
                                if (!ended[j]) closeIterator(sources[j].iterator);
                            }
                            throw new RangeError("Iterator.zip: iterators have different lengths");
                        }
                        result[key] = step ? step.value : undefined;
                    }
                }

                if (mode === "strict" && firstEndedThisRound) {
                    return { done: true, value: undefined };
                }

                if (mode === "longest" && endedCount === count) {
                    done = true;
                    return { done: true, value: undefined };
                }

                return { done: false, value: result };
            },
            () => {
                done = true;
                for (let i = 0; i < count; i++) {
                    if (!ended[i]) closeIterator(sources[i].iterator);
                }
            }
        );
    }

    // Iterator.prototype.chunks(size)
    function chunks(this: any, size: any) {
        if (typeof size !== "number") {
            closeIterator(this);
            throw new TypeError("chunks size must be a Number");
        }
        if (!Number.isFinite(size) || Math.trunc(size) !== size) {
            closeIterator(this);
            throw new TypeError("chunks size must be an integral Number");
        }
        if (size < 1 || size > 4294967295) {
            closeIterator(this);
            throw new RangeError("Iterator limit out of range");
        }

        const self = this;
        let done = false;

        return createHelper(
            () => {
                if (done) return { done: true, value: undefined };
                const chunk: any[] = [];
                for (let i = 0; i < size; i++) {
                    const step = self.next();
                    if (step && step.done) {
                        done = true;
                        if (chunk.length > 0) {
                            return { done: false, value: chunk };
                        }
                        return { done: true, value: undefined };
                    }
                    chunk.push(step ? step.value : undefined);
                }
                return { done: false, value: chunk };
            },
            () => {
                done = true;
                closeIterator(self);
            }
        );
    }

    // Iterator.prototype.windows(size, undersized?)
    function windows(this: any, size: any, undersized?: any) {
        if (typeof size !== "number") {
            closeIterator(this);
            throw new TypeError("windows size must be a Number");
        }
        if (!Number.isFinite(size) || Math.trunc(size) !== size) {
            closeIterator(this);
            throw new TypeError("windows size must be an integral Number");
        }
        if (size < 1 || size > 4294967295) {
            closeIterator(this);
            throw new RangeError("Iterator limit out of range");
        }

        let allowPartial = false;
        if (undersized !== undefined) {
            if (undersized === "allow-partial") {
                allowPartial = true;
            } else if (undersized !== "only-full") {
                closeIterator(this);
                throw new TypeError('windows undersized must be "only-full" or "allow-partial"');
            }
        }

        const self = this;
        const buffer: any[] = [];
        let done = false;
        let filledOnce = false;

        return createHelper(
            () => {
                if (done) return { done: true, value: undefined };

                if (!filledOnce) {
                    while (buffer.length < size) {
                        const step = self.next();
                        if (step && step.done) {
                            done = true;
                            if (allowPartial && buffer.length > 0 && buffer.length < size) {
                                const result = buffer.slice();
                                buffer.length = 0;
                                return { done: false, value: result };
                            }
                            return { done: true, value: undefined };
                        }
                        buffer.push(step ? step.value : undefined);
                    }
                    filledOnce = true;
                    return { done: false, value: buffer.slice() };
                }

                const step = self.next();
                if (step && step.done) {
                    done = true;
                    return { done: true, value: undefined };
                }
                buffer.shift();
                buffer.push(step ? step.value : undefined);
                return { done: false, value: buffer.slice() };
            },
            () => {
                done = true;
                closeIterator(self);
            }
        );
    }

    // Iterator.prototype.join(separator?)
    function join(this: any, separator?: any) {
        const sep = separator === undefined ? "," : String(separator);
        const parts: string[] = [];
        while (true) {
            const step = this.next();
            if (step && step.done) break;
            const val = step ? step.value : undefined;
            if (val === null || val === undefined) {
                parts.push("");
            } else {
                parts.push(String(val));
            }
        }
        return parts.join(sep);
    }

    // Iterator.prototype[Symbol.dispose]()
    function dispose(this: any) {
        closeIterator(this);
    }

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
