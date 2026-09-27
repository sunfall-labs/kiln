// ECMAScript Object and Reflect extensions (ADR 0005, ADR 0063 Rule 4).
// Includes:
// - Object.seal(o)
// - Object.isSealed(o)
// - Reflect.isExtensible(target)
// - Reflect.preventExtensions(target)
// - Reflect.setPrototypeOf(target, proto)

// `[[SetPrototypeOf]]` and `[[PreventExtensions]]` with their boolean result (ADR 0065,
// representation): `Object.setPrototypeOf` and `Object.preventExtensions` report `false` as a
// TypeError, which cannot be told apart from a TypeError a proxy trap threw or an invariant check
// raised -- and those must propagate. An engine that publishes neither leaves the fallback below.
const __kilnSetPrototypeOf: ((target: any, proto: any) => boolean) | undefined =
    (globalThis as any).__kilnSetPrototypeOf;
const __kilnPreventExtensions: ((target: any) => boolean) | undefined =
    (globalThis as any).__kilnPreventExtensions;
delete (globalThis as any).__kilnSetPrototypeOf;
delete (globalThis as any).__kilnPreventExtensions;

(function installObjectReflectBuiltins() {
    // Built-in methods are not constructors, so each is a method definition (`{ m() {} }.m`)
    // rather than a function declaration, which would have [[Construct]].
    const defineMethod = (target: any, name: string | symbol, fn: any) => {
        Object.defineProperty(target, name, {
            value: fn,
            writable: true,
            enumerable: false,
            configurable: true,
        });
    };

    if (typeof Reflect !== "undefined") {
        const isExtensible = { isExtensible(target: any) {
            if (target === null || (typeof target !== "object" && typeof target !== "function")) {
                throw new TypeError("Reflect.isExtensible called on non-object");
            }
            return Object.isExtensible(target);
        } }.isExtensible;

        const preventExtensions = { preventExtensions(target: any) {
            if (target === null || (typeof target !== "object" && typeof target !== "function")) {
                throw new TypeError("Reflect.preventExtensions called on non-object");
            }
            if (__kilnPreventExtensions !== undefined) return __kilnPreventExtensions(target);
            // Without the primitive: `Object.preventExtensions` reports a refusal as a TypeError,
            // and anything else -- a proxy trap's own throw -- is the program's and propagates.
            // A trap's own TypeError is misread as a refusal here; the primitive has no such gap.
            try {
                Object.preventExtensions(target);
                return !Object.isExtensible(target);
            } catch (error) {
                if (error instanceof TypeError) return false;
                throw error;
            }
        } }.preventExtensions;

        const setPrototypeOf = { setPrototypeOf(target: any, proto: any) {
            if (target === null || (typeof target !== "object" && typeof target !== "function")) {
                throw new TypeError("Reflect.setPrototypeOf called on non-object");
            }
            if (proto !== null && typeof proto !== "object" && typeof proto !== "function") {
                throw new TypeError("Object prototype may only be an Object or null: " + String(proto));
            }
            if (__kilnSetPrototypeOf !== undefined) return __kilnSetPrototypeOf(target, proto);
            // As above: a refusal is a TypeError, and any other throw is the program's.
            try {
                Object.setPrototypeOf(target, proto);
                return true;
            } catch (error) {
                if (error instanceof TypeError) return false;
                throw error;
            }
        } }.setPrototypeOf;

        defineMethod(Reflect, "isExtensible", isExtensible);
        defineMethod(Reflect, "preventExtensions", preventExtensions);
        defineMethod(Reflect, "setPrototypeOf", setPrototypeOf);
    }

    if (typeof Object !== "undefined") {
        const seal = { seal(o: any) {
            if (o === null || (typeof o !== "object" && typeof o !== "function")) {
                return o;
            }
            Object.preventExtensions(o);
            const keys = Object.getOwnPropertyNames(o);
            for (let i = 0; i < keys.length; i++) {
                const key = keys[i];
                const desc = Object.getOwnPropertyDescriptor(o, key);
                if (desc && desc.configurable) {
                    Object.defineProperty(o, key, { configurable: false });
                }
            }
            if (typeof Object.getOwnPropertySymbols === "function") {
                const symbols = Object.getOwnPropertySymbols(o);
                for (let i = 0; i < symbols.length; i++) {
                    const sym = symbols[i];
                    const desc = Object.getOwnPropertyDescriptor(o, sym);
                    if (desc && desc.configurable) {
                        Object.defineProperty(o, sym, { configurable: false });
                    }
                }
            }
            return o;
        } }.seal;

        const isSealed = { isSealed(o: any) {
            if (o === null || (typeof o !== "object" && typeof o !== "function")) {
                return true;
            }
            if (Object.isExtensible(o)) {
                return false;
            }
            const keys = Object.getOwnPropertyNames(o);
            for (let i = 0; i < keys.length; i++) {
                const desc = Object.getOwnPropertyDescriptor(o, keys[i]);
                if (desc && desc.configurable) {
                    return false;
                }
            }
            if (typeof Object.getOwnPropertySymbols === "function") {
                const symbols = Object.getOwnPropertySymbols(o);
                for (let i = 0; i < symbols.length; i++) {
                    const desc = Object.getOwnPropertyDescriptor(o, symbols[i]);
                    if (desc && desc.configurable) {
                        return false;
                    }
                }
            }
            return true;
        } }.isSealed;

        defineMethod(Object, "seal", seal);
        defineMethod(Object, "isSealed", isSealed);
    }
})();
