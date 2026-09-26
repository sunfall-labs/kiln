// ECMAScript Object and Reflect extensions (ADR 0005, ADR 0063 Rule 4).
// Includes:
// - Object.seal(o)
// - Object.isSealed(o)
// - Reflect.isExtensible(target)
// - Reflect.preventExtensions(target)
// - Reflect.setPrototypeOf(target, proto)

(function installObjectReflectBuiltins() {
    const defineMethod = (target: any, name: string | symbol, fn: any) => {
        Object.defineProperty(target, name, {
            value: fn,
            writable: true,
            enumerable: false,
            configurable: true,
        });
    };

    if (typeof Reflect !== "undefined") {
        function isExtensible(target: any) {
            if (target === null || (typeof target !== "object" && typeof target !== "function")) {
                throw new TypeError("Reflect.isExtensible called on non-object");
            }
            return Object.isExtensible(target);
        }

        function preventExtensions(target: any) {
            if (target === null || (typeof target !== "object" && typeof target !== "function")) {
                throw new TypeError("Reflect.preventExtensions called on non-object");
            }
            try {
                Object.preventExtensions(target);
                return !Object.isExtensible(target);
            } catch {
                return false;
            }
        }

        function setPrototypeOf(target: any, proto: any) {
            if (target === null || (typeof target !== "object" && typeof target !== "function")) {
                throw new TypeError("Reflect.setPrototypeOf called on non-object");
            }
            if (proto !== null && typeof proto !== "object" && typeof proto !== "function") {
                throw new TypeError("Object prototype may only be an Object or null: " + String(proto));
            }
            try {
                Object.setPrototypeOf(target, proto);
                return true;
            } catch {
                return false;
            }
        }

        defineMethod(Reflect, "isExtensible", isExtensible);
        defineMethod(Reflect, "preventExtensions", preventExtensions);
        defineMethod(Reflect, "setPrototypeOf", setPrototypeOf);
    }

    if (typeof Object !== "undefined") {
        function seal(o: any) {
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
        }

        function isSealed(o: any) {
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
        }

        defineMethod(Object, "seal", seal);
        defineMethod(Object, "isSealed", isSealed);
    }
})();
