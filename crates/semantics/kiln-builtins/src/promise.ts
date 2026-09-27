// ECMAScript Promise extensions (ADR 0005, ADR 0063 Rule 4).
// Includes:
// - Promise.try(callbackfn, ...args) (ES2025)
// - Promise.allKeyed(obj) (Stage 2/3 / Kiln extension)
// - Promise.allSettledKeyed(obj) (Stage 2/3 / Kiln extension)

(function installPromiseBuiltins() {
    if (typeof Promise === "undefined") {
        return;
    }

    // Built-in methods are not constructors, so each is a method definition (`{ m() {} }.m`)
    // rather than a function declaration, which would have [[Construct]].
    // One descriptor for every member, not a literal per call: `defineProperty` reads its fields
    // and keeps nothing, and a literal per member was the largest share of the garbage every
    // realm's bootstrap made (realms issue 08).
    const methodDescriptor: PropertyDescriptor = { value: undefined, writable: true, enumerable: false, configurable: true };
    const defineMethod = (target: any, name: string | symbol, fn: any) => {
        methodDescriptor.value = fn;
        Object.defineProperty(target, name, methodDescriptor);
        methodDescriptor.value = undefined;
    };

    // Promise.try(callbackfn, ...args)
    const promiseTry = { try(this: any, callbackfn: any, ...args: any[]) {
        const C = this;
        if (C === null || (typeof C !== "object" && typeof C !== "function")) {
            throw new TypeError("Promise.try called on a non-object");
        }
        return new C((resolve: any) => {
            resolve(callbackfn(...args));
        });
    } }.try;

    // `NewPromiseCapability(C)`: the executor records the two functions `C` hands it, refuses a
    // second pair, and both must be callable once `C` returns.
    function newPromiseCapability(C: any): { promise: any; resolve: any; reject: any } {
        if (typeof C !== "function") {
            throw new TypeError("Promise combinator called on a non-constructor");
        }
        let resolve: any = undefined;
        let reject: any = undefined;
        const executor = (res: any, rej: any) => {
            if (resolve !== undefined) throw new TypeError("Promise executor already has a resolve function");
            if (reject !== undefined) throw new TypeError("Promise executor already has a reject function");
            resolve = res;
            reject = rej;
        };
        const promise = new C(executor);
        if (typeof resolve !== "function") throw new TypeError("Promise resolve function is not callable");
        if (typeof reject !== "function") throw new TypeError("Promise reject function is not callable");
        return { promise, resolve, reject };
    }

    // `Promise.allKeyed` and `Promise.allSettledKeyed`: `PerformPromiseAllKeyed` over the
    // object's own enumerable keys, in `[[OwnPropertyKeys]]` order, with the result object built
    // in that order once every element has settled. Every abrupt completion after the capability
    // exists rejects its promise (`IfAbruptRejectPromise`) rather than throwing.
    function performKeyed(C: any, promises: any, settled: boolean): any {
        const capability = newPromiseCapability(C);
        try {
            const promiseResolve = C.resolve;
            if (typeof promiseResolve !== "function") {
                throw new TypeError("Promise resolve is not a function");
            }
            if (promises === null || (typeof promises !== "object" && typeof promises !== "function")) {
                throw new TypeError(String(promises) + " is not an object");
            }
            const keys: any[] = [];
            const values: any[] = [];
            let remaining = 1;
            const finish = () => {
                const result = Object.create(null);
                for (let i = 0; i < keys.length; i++) {
                    Object.defineProperty(result, keys[i], {
                        value: values[i],
                        writable: true,
                        enumerable: true,
                        configurable: true,
                    });
                }
                capability.resolve.call(undefined, result);
            };
            const allKeys = Reflect.ownKeys(promises);
            for (let k = 0; k < allKeys.length; k++) {
                const key = allKeys[k];
                const desc = Reflect.getOwnPropertyDescriptor(promises, key);
                if (desc === undefined || !desc.enumerable) continue;
                const value = promises[key];
                const index = keys.length;
                keys.push(key);
                values.push(undefined);
                const nextPromise = promiseResolve.call(C, value);
                let alreadyCalled = false;
                const record = (entry: any) => {
                    if (alreadyCalled) return undefined;
                    alreadyCalled = true;
                    values[index] = entry;
                    remaining--;
                    if (remaining === 0) finish();
                    return undefined;
                };
                const onFulfilled = (x: any) =>
                    record(settled ? { status: "fulfilled", value: x } : x);
                const onRejected = settled
                    ? (y: any) => record({ status: "rejected", reason: y })
                    : capability.reject;
                remaining++;
                const then = nextPromise.then;
                if (typeof then !== "function") {
                    throw new TypeError("then is not a function");
                }
                then.call(nextPromise, onFulfilled, onRejected);
            }
            remaining--;
            if (remaining === 0) finish();
        } catch (error) {
            capability.reject.call(undefined, error);
        }
        return capability.promise;
    }

    // Promise.allKeyed(obj)
    const allKeyed = { allKeyed(this: any, promises: any) {
        return performKeyed(this, promises, false);
    } }.allKeyed;

    // Promise.allSettledKeyed(obj)
    const allSettledKeyed = { allSettledKeyed(this: any, promises: any) {
        return performKeyed(this, promises, true);
    } }.allSettledKeyed;

    defineMethod(Promise, "try", promiseTry);
    defineMethod(Promise, "allKeyed", allKeyed);
    defineMethod(Promise, "allSettledKeyed", allSettledKeyed);
})();
