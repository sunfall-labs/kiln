// Public JSON replacer traversal over the existing native serialization primitive (ADR 0005).
// The interpreter already supplies the complete JSON algorithm; this hook is native-only.
(function installJSONReplacers() {
  const G = globalThis as any;
  const serialize = G.__kilnJsonSerialize;
  delete G.__kilnJsonSerialize;
  if (typeof serialize !== "function") return;

  function unbox(value: any): any {
    if (value === null || typeof value !== "object") return value;
    const tag = Object.prototype.toString.call(value);
    // The tag only selects a brand check; a forged @@toStringTag must not unbox an object.
    try {
      if (tag === "[object Number]") return Number.prototype.valueOf.call(value);
      if (tag === "[object String]") return String.prototype.valueOf.call(value);
      if (tag === "[object Boolean]") return Boolean.prototype.valueOf.call(value);
      if (tag === "[object BigInt]") return BigInt.prototype.valueOf.call(value);
    } catch (_) {}
    return value;
  }

  const stringify = {
    stringify(value: any, replacer?: any, space?: any): any {
      space = unbox(space);
      const callback = typeof replacer === "function" ? replacer : undefined;
      let propertyList: string[] | undefined;
      if (Array.isArray(replacer)) {
        propertyList = [];
        const length = replacer.length;
        for (let i = 0; i < length; i++) {
          const item = unbox(replacer[i]);
          if (typeof item === "string" || typeof item === "number") {
            const key = String(item);
            if (propertyList.indexOf(key) === -1) propertyList.push(key);
          }
        }
      }
      if (callback === undefined && propertyList === undefined)
        return serialize(value, undefined, space);

      const ancestors: any[] = [];
      function visit(holder: any, key: string): any {
        let current = holder[key];
        if (current !== null && (typeof current === "object" ||
            typeof current === "function" || typeof current === "bigint")) {
          const toJSON = current.toJSON;
          if (typeof toJSON === "function") current = toJSON.call(current, key);
        }
        if (callback !== undefined) current = callback.call(holder, key, current);
        current = unbox(current);
        if (current === null || typeof current !== "object") return current;
        if (ancestors.indexOf(current) !== -1)
          throw new TypeError("Converting circular structure to JSON");
        ancestors.push(current);
        let result: any;
        if (Array.isArray(current)) {
          result = [];
          const length = current.length;
          for (let i = 0; i < length; i++) result.push(visit(current, String(i)));
        } else {
          result = {};
          const keys = propertyList === undefined ? Object.keys(current) : propertyList;
          for (let i = 0; i < keys.length; i++) {
            const key = keys[i];
            const item = visit(current, key);
            if (item !== undefined && typeof item !== "function" && typeof item !== "symbol") {
              Object.defineProperty(result, key, {
                value: item, writable: true, enumerable: true, configurable: true,
              });
            }
          }
        }
        ancestors.pop();
        return result;
      }
      return serialize(visit({ "": value }, ""), undefined, space);
    },
  }.stringify;
  Object.defineProperty(G.JSON, "stringify", {
    value: stringify, writable: true, configurable: true,
  });
})();
