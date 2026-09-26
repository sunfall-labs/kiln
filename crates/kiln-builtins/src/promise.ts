// ECMAScript Promise extensions (ADR 0005, ADR 0063 Rule 4).
// Includes:
// - Promise.try(callbackfn, ...args) (ES2025)
// - Promise.allKeyed(obj) (Stage 2/3 / Kiln extension)
// - Promise.allSettledKeyed(obj) (Stage 2/3 / Kiln extension)

(function installPromiseBuiltins() {
    if (typeof Promise === "undefined") {
        return;
    }

    const defineMethod = (target: any, name: string | symbol, fn: any) => {
        Object.defineProperty(target, name, {
            value: fn,
            writable: true,
            enumerable: false,
            configurable: true,
        });
    };

    // Promise.try(callbackfn, ...args)
    function promiseTry(this: any, callbackfn: any, ...args: any[]) {
        const C = this;
        if (C === null || (typeof C !== "object" && typeof C !== "function")) {
            throw new TypeError("Promise.try called on a non-object");
        }
        return new C((resolve: any) => {
            resolve(callbackfn(...args));
        });
    }

    // Promise.allKeyed(obj)
    function allKeyed(this: any, obj: any) {
        const C = this;
        if (C === null || (typeof C !== "object" && typeof C !== "function")) {
            throw new TypeError("Promise.allKeyed called on a non-object");
        }
        const promiseResolve = C.resolve;
        if (typeof promiseResolve !== "function") {
            throw new TypeError("Promise resolve is not a function");
        }
        if (obj === null || (typeof obj !== "object" && typeof obj !== "function")) {
            return new C((_: any, reject: any) => {
                reject(new TypeError(String(obj) + " is not an object"));
            });
        }
        return new C((resolve: any, reject: any) => {
            let keys: (string | symbol)[];
            try {
                keys = Reflect.ownKeys(obj);
            } catch (err) {
                reject(err);
                return;
            }
            const activeKeys: (string | symbol)[] = [];
            const values: any[] = [];
            for (let i = 0; i < keys.length; i++) {
                const key = keys[i];
                let desc;
                try {
                    desc = Object.getOwnPropertyDescriptor(obj, key);
                } catch (err) {
                    reject(err);
                    return;
                }
                if (desc && desc.enumerable) {
                    let val;
                    try {
                        val = obj[key];
                    } catch (err) {
                        reject(err);
                        return;
                    }
                    activeKeys.push(key);
                    values.push(val);
                }
            }

            if (activeKeys.length === 0) {
                resolve(Object.create(null));
                return;
            }

            const result = Object.create(null);
            let remaining = activeKeys.length;
            let settled = false;

            for (let i = 0; i < activeKeys.length; i++) {
                const key = activeKeys[i];
                const val = values[i];
                let p;
                try {
                    p = promiseResolve.call(C, val);
                } catch (err) {
                    if (!settled) {
                        settled = true;
                        reject(err);
                    }
                    return;
                }
                p.then(
                    (resolvedVal: any) => {
                        if (settled) return;
                        result[key] = resolvedVal;
                        remaining--;
                        if (remaining === 0) {
                            settled = true;
                            resolve(result);
                        }
                    },
                    (err: any) => {
                        if (!settled) {
                            settled = true;
                            reject(err);
                        }
                    }
                );
            }
        });
    }

    // Promise.allSettledKeyed(obj)
    function allSettledKeyed(this: any, obj: any) {
        const C = this;
        if (C === null || (typeof C !== "object" && typeof C !== "function")) {
            throw new TypeError("Promise.allSettledKeyed called on a non-object");
        }
        const promiseResolve = C.resolve;
        if (typeof promiseResolve !== "function") {
            throw new TypeError("Promise resolve is not a function");
        }
        if (obj === null || (typeof obj !== "object" && typeof obj !== "function")) {
            return new C((_: any, reject: any) => {
                reject(new TypeError(String(obj) + " is not an object"));
            });
        }
        return new C((resolve: any, reject: any) => {
            let keys: (string | symbol)[];
            try {
                keys = Reflect.ownKeys(obj);
            } catch (err) {
                reject(err);
                return;
            }
            const activeKeys: (string | symbol)[] = [];
            const values: any[] = [];
            for (let i = 0; i < keys.length; i++) {
                const key = keys[i];
                let desc;
                try {
                    desc = Object.getOwnPropertyDescriptor(obj, key);
                } catch (err) {
                    reject(err);
                    return;
                }
                if (desc && desc.enumerable) {
                    let val;
                    try {
                        val = obj[key];
                    } catch (err) {
                        reject(err);
                        return;
                    }
                    activeKeys.push(key);
                    values.push(val);
                }
            }

            if (activeKeys.length === 0) {
                resolve(Object.create(null));
                return;
            }

            const result = Object.create(null);
            let remaining = activeKeys.length;
            let settled = false;

            for (let i = 0; i < activeKeys.length; i++) {
                const key = activeKeys[i];
                const val = values[i];
                let p;
                try {
                    p = promiseResolve.call(C, val);
                } catch (err) {
                    if (!settled) {
                        settled = true;
                        reject(err);
                    }
                    return;
                }
                p.then(
                    (resolvedVal: any) => {
                        if (settled) return;
                        result[key] = { status: "fulfilled", value: resolvedVal };
                        remaining--;
                        if (remaining === 0) {
                            settled = true;
                            resolve(result);
                        }
                    },
                    (err: any) => {
                        if (settled) return;
                        result[key] = { status: "rejected", reason: err };
                        remaining--;
                        if (remaining === 0) {
                            settled = true;
                            resolve(result);
                        }
                    }
                );
            }
        });
    }

    defineMethod(Promise, "try", promiseTry);
    defineMethod(Promise, "allKeyed", allKeyed);
    defineMethod(Promise, "allSettledKeyed", allSettledKeyed);
})();
