// Public JSON replacer traversal over the existing native serialization primitive (ADR 0005).
// The interpreter already supplies the complete JSON algorithm; this hook is native-only.
(function installJSONReplacers() {
  const G = globalThis as any;
  const serialize = G.__kilnJsonSerialize;
  const parse = G.__kilnJsonParse;
  const unboxPrimitive = G.__kilnJsonUnbox;
  delete G.__kilnJsonSerialize;
  delete G.__kilnJsonParse;
  delete G.__kilnJsonUnbox;
  if (typeof serialize !== "function") return;

  function unbox(value: any): any {
    if (value === null || typeof value !== "object") return value;
    const primitive = unboxPrimitive(value);
    if (primitive === value) return value;
    // Number and String wrapper conversion can invoke user coercion hooks. Boolean and BigInt
    // serialization reads their internal slots instead; classification never reads a public tag.
    if (typeof primitive === "number") return Number(value);
    if (typeof primitive === "string") return String(value);
    return primitive;
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
        if (typeof current === "bigint") throw new TypeError("Do not know how to serialize a BigInt");
        if (typeof current === "function" || typeof current === "symbol") return undefined;
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
          result = Object.create(null);
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
  const parseWithReviver = {
    parse(text: any, reviver?: any): any {
      const value = parse(text);
      if (typeof reviver !== "function") return value;
      function visit(holder: any, key: string): any {
        const value = holder[key];
        if (value !== null && typeof value === "object") {
          const keys: string[] = [];
          if (Array.isArray(value)) {
            const length = value.length;
            for (let i = 0; i < length; i++) keys.push(String(i));
          } else {
            const own = Object.keys(value);
            for (let i = 0; i < own.length; i++) keys.push(own[i]);
          }
          for (let i = 0; i < keys.length; i++) {
            const childKey = keys[i];
            const child = visit(value, childKey);
            if (child === undefined) delete value[childKey];
            else Object.defineProperty(value, childKey, {
              value: child, writable: true, enumerable: true, configurable: true,
            });
          }
        }
        return reviver.call(holder, key, value);
      }
      return visit({ "": value }, "");
    },
  }.parse;
  Object.defineProperty(G.JSON, "parse", {
    value: parseWithReviver, writable: true, configurable: true,
  });
})();
