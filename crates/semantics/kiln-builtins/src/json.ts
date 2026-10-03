// Public JSON replacer traversal over the existing native serialization primitive (ADR 0005).
// The interpreter already supplies the complete JSON algorithm; this hook is native-only.
(function installJSONReplacers() {
  const G = globalThis as any;
  const serialize = G.__kilnJsonSerialize;
  const parse = G.__kilnJsonParse;
  const unboxPrimitive = G.__kilnJsonUnbox;
  const rawCreate = G.__kilnJsonRawCreate;
  const isRaw = G.__kilnJsonIsRaw;
  delete G.__kilnJsonSerialize;
  delete G.__kilnJsonParse;
  delete G.__kilnJsonUnbox;
  delete G.__kilnJsonRawCreate;
  delete G.__kilnJsonIsRaw;
  if (typeof rawCreate === "function" && typeof isRaw === "function") {
    Object.defineProperties(G.JSON, {
      rawJSON: {
        value: function rawJSON(text: any): object { return rawCreate(text); },
        writable: true, enumerable: false, configurable: true,
      },
      isRawJSON: {
        value: function isRawJSON(value: any): boolean { return isRaw(value); },
        writable: true, enumerable: false, configurable: true,
      },
    });
  }
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
      if (typeof text === "symbol") throw new TypeError("Cannot convert a Symbol value to a string");
      const input = String(text);
      const value = parse(input);
      if (typeof reviver !== "function") return value;
      // Keep original primitive token text for the reviver context. The validated document is
      // scanned before callbacks run, so subsequent property mutations cannot change its proof.
      let position = 0;
      function whitespace(): void {
        while (position < input.length && (input[position] === ' ' || input[position] === '\n' ||
          input[position] === '\r' || input[position] === '\t')) position++;
      }
      function stringEnd(): void {
        position++;
        while (position < input.length) {
          const character = input[position++];
          if (character === '\\') position++;
          else if (character === '"') return;
        }
      }
      function sourceRecord(value: any): any {
        whitespace();
        const start = position;
        const record: any = { value, source: undefined, children: undefined };
        if (input[position] === '{' || input[position] === '[') {
          const array = input[position] === '[';
          const end = array ? ']' : '}';
          record.children = Object.create(null);
          position++;
          whitespace();
          let index = 0;
          while (input[position] !== end) {
            let key: string;
            if (array) key = String(index++);
            else {
              const keyStart = position;
              stringEnd();
              key = parse(input.slice(keyStart, position));
              whitespace();
              position++; // colon: the primitive parser has already validated it
            }
            record.children[key] = sourceRecord(value !== null && typeof value === "object"
              ? value[key] : undefined);
            whitespace();
            if (input[position] !== ',') break;
            position++;
            whitespace();
          }
          position++;
        } else {
          if (input[position] === '"') stringEnd();
          else {
            while (position < input.length && input[position] !== ',' && input[position] !== '}' &&
              input[position] !== ']' && input[position] !== ' ' && input[position] !== '\n' &&
              input[position] !== '\r' && input[position] !== '\t') position++;
          }
          record.source = input.slice(start, position);
        }
        return record;
      }
      const source = sourceRecord(value);
      function visit(holder: any, key: string, record: any): any {
        const value = holder[key];
        if (record !== undefined && !Object.is(record.value, value)) record = undefined;
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
            const child = visit(value, childKey,
              record !== undefined && record.children !== undefined ? record.children[childKey] : undefined);
            if (child === undefined) delete value[childKey];
            else Object.defineProperty(value, childKey, {
              value: child, writable: true, enumerable: true, configurable: true,
            });
          }
        }
        const context: any = {};
        if (record !== undefined && record.source !== undefined && Object.is(record.value, value))
          context.source = record.source;
        return reviver.call(holder, key, value, context);
      }
      return visit({ "": value }, "", source);
    },
  }.parse;
  Object.defineProperty(G.JSON, "parse", {
    value: parseWithReviver, writable: true, configurable: true,
  });
})();
