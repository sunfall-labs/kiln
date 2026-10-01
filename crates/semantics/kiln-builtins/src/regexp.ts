// kiln-builtins: `RegExp.prototype` methods and `RegExp.escape` (ECMAScript specification / ES2025)
// ADR 0005, ADR 0063 rule 4

(function installRegExp() {
  if (typeof RegExp === "undefined" || !RegExp.prototype) return;
  const Proto = RegExp.prototype as any;
  const G = globalThis as any;
  const rawString = G.__kilnRawStringProtocols || {
    match: String.prototype.match,
    matchAll: String.prototype.matchAll,
    replace: String.prototype.replace,
    replaceAll: String.prototype.replaceAll,
    search: String.prototype.search,
    split: String.prototype.split,
  };
  delete G.__kilnRawStringProtocols;
  const IntrinsicRegExp = RegExp;
  const isObject = (value: any): boolean => value !== null &&
    (typeof value === "object" || typeof value === "function");
  const requireReceiver = (receiver: any, name: string): void => {
    if (receiver === null || receiver === undefined) {
      throw new TypeError(`String.prototype.${name} called on null or undefined`);
    }
  };
  const invokeRegExpProtocol = (regexp: any, key: symbol, args: any[]): any => {
    const method = regexp[key];
    return Reflect.apply(method, regexp, args);
  };
  const match = { match(this: any, regexp: any): any {
    "use strict";
    requireReceiver(this, "match");
    if (isObject(regexp)) {
      const matcher = regexp[Symbol.match];
      if (matcher !== null && matcher !== undefined) return Reflect.apply(matcher, regexp, [this]);
    }
    const subject = `${this}`;
    return invokeRegExpProtocol(new IntrinsicRegExp(regexp), Symbol.match, [subject]);
  } }.match;
  const matchAll = { matchAll(this: any, regexp: any): any {
    "use strict";
    requireReceiver(this, "matchAll");
    let isRegExp = false;
    if (isObject(regexp)) {
      const matcher = regexp[Symbol.match];
      isRegExp = matcher === undefined ? regexp instanceof IntrinsicRegExp : !!matcher;
      if (isRegExp) {
        const flags = regexp.flags;
        if (flags === null || flags === undefined || `${flags}`.indexOf("g") < 0) {
          throw new TypeError("String.prototype.matchAll called with a non-global RegExp");
        }
      }
    }
    if (isObject(regexp)) {
      const matcher = regexp[Symbol.matchAll];
      if (matcher !== null && matcher !== undefined) return Reflect.apply(matcher, regexp, [this]);
    }
    const subject = `${this}`;
    const created = new IntrinsicRegExp(regexp, "g");
    return invokeRegExpProtocol(created, Symbol.matchAll, [subject]);
  } }.matchAll;
  const replace = { replace(this: any, searchValue: any, replaceValue: any): any {
    "use strict";
    requireReceiver(this, "replace");
    if (isObject(searchValue)) {
      const replacer = searchValue[Symbol.replace];
      if (replacer !== null && replacer !== undefined) return Reflect.apply(replacer, searchValue, [this, replaceValue]);
    }
    return Reflect.apply(rawString.replace, this, [searchValue, replaceValue]);
  } }.replace;
  const replaceAll = { replaceAll(this: any, searchValue: any, replaceValue: any): any {
    "use strict";
    requireReceiver(this, "replaceAll");
    if (isObject(searchValue)) {
      const matcher = searchValue[Symbol.match];
      const isRegExp = matcher === undefined ? searchValue instanceof IntrinsicRegExp : !!matcher;
      if (isRegExp) {
        const flags = searchValue.flags;
        if (flags === null || flags === undefined || `${flags}`.indexOf("g") < 0) {
          throw new TypeError("String.prototype.replaceAll must be called with a global RegExp");
        }
      }
      const replacer = searchValue[Symbol.replace];
      if (replacer !== null && replacer !== undefined) return Reflect.apply(replacer, searchValue, [this, replaceValue]);
    }
    return Reflect.apply(rawString.replaceAll, this, [searchValue, replaceValue]);
  } }.replaceAll;
  const search = { search(this: any, regexp: any): any {
    "use strict";
    requireReceiver(this, "search");
    if (isObject(regexp)) {
      const matcher = regexp[Symbol.search];
      if (matcher !== null && matcher !== undefined) return Reflect.apply(matcher, regexp, [this]);
    }
    const subject = `${this}`;
    return invokeRegExpProtocol(new IntrinsicRegExp(regexp), Symbol.search, [subject]);
  } }.search;
  const split = { split(this: any, separator: any, limit: any): any {
    "use strict";
    requireReceiver(this, "split");
    if (isObject(separator)) {
      const splitter = separator[Symbol.split];
      if (splitter !== null && splitter !== undefined) return Reflect.apply(splitter, separator, [this, limit]);
    }
    return Reflect.apply(rawString.split, this, [separator, limit]);
  } }.split;
  for (const [name, value] of [["match", match], ["matchAll", matchAll], ["replace", replace],
                               ["replaceAll", replaceAll], ["search", search], ["split", split]] as any) {
    Object.defineProperty(String.prototype, name, {
      value, writable: true, enumerable: false, configurable: true,
    });
  }

  // Annex B legacy capture properties. Both engines keep this state per realm; keep the public
  // accessors in this TypeScript bootstrap so runtime C owns only the private capture bridge.
  const legacyCapture = (globalThis as any).__kilnRegExpLegacyCapture;
  delete (globalThis as any).__kilnRegExpLegacyCapture;
  for (let index = 1; index <= 9; index++) {
    const property = `$${index}`;
    const descriptor = {
      get [property]() { return legacyCapture === undefined ? "" : legacyCapture(index) ?? ""; },
      set [property](_value: any) {},
    };
    const accessor = Object.getOwnPropertyDescriptor(descriptor, property) as PropertyDescriptor;
    Object.defineProperty(RegExp, property, {
      get: accessor.get,
      set: accessor.set,
      enumerable: false,
      configurable: true,
    });
  }

  // Annex B.3 method `compile(pattern, flags?)`:
  const compile = { compile(this: any, pattern: any, flags?: any): RegExp {
    if (!(this instanceof RegExp)) {
      throw new TypeError("RegExp.prototype.compile called on incompatible receiver");
    }
    return new RegExp(pattern, flags);
  } }.compile;

  Object.defineProperty(Proto, "compile", {
    value: compile,
    writable: true,
    enumerable: false,
    configurable: true,
  });

  // Static `RegExp.escape(text)` (ES2025):
  const escape = { escape(text: any): string {
    if (typeof text !== "string") {
      throw new TypeError("RegExp.escape requires a string");
    }
    const syntax = "^$\\.*+?()[]{}|/";
    const punctuators = ",-=<>#&!%:;@~'`\"";
    let escaped = "";
    for (let i = 0; i < text.length; i++) {
      const code = text.charCodeAt(i);
      const ch = text.charAt(i);

      if (
        escaped.length === 0 &&
        ((code >= 0x30 && code <= 0x39) ||
          (code >= 0x41 && code <= 0x5a) ||
          (code >= 0x61 && code <= 0x7a))
      ) {
        escaped += "\\x" + code.toString(16).padStart(2, "0");
        continue;
      }
      if (syntax.includes(ch)) {
        escaped += "\\" + ch;
        continue;
      }
      if (ch === "\t") {
        escaped += "\\t";
        continue;
      }
      if (ch === "\n") {
        escaped += "\\n";
        continue;
      }
      if (ch === "\v") {
        escaped += "\\v";
        continue;
      }
      if (ch === "\f") {
        escaped += "\\f";
        continue;
      }
      if (ch === "\r") {
        escaped += "\\r";
        continue;
      }

      // `EncodeForRegExpEscape`: the other punctuators, every WhiteSpace and LineTerminator,
      // and a lone surrogate are written as a hex escape -- `\xHH` below 0x100, `\uHHHH` above.
      const whiteSpace =
        code === 0x20 || code === 0xa0 || code === 0x1680 || (code >= 0x2000 && code <= 0x200a) ||
        code === 0x202f || code === 0x205f || code === 0x3000 || code === 0xfeff ||
        code === 0x2028 || code === 0x2029;
      let loneSurrogate = false;
      if (code >= 0xd800 && code <= 0xdbff) {
        const low = i + 1 < text.length ? text.charCodeAt(i + 1) : 0;
        if (low >= 0xdc00 && low <= 0xdfff) {
          escaped += ch + text.charAt(i + 1);
          i++;
          continue;
        }
        loneSurrogate = true;
      } else if (code >= 0xdc00 && code <= 0xdfff) {
        loneSurrogate = true;
      }
      if (punctuators.includes(ch) || whiteSpace || loneSurrogate) {
        escaped += code <= 0xff
          ? "\\x" + code.toString(16).padStart(2, "0")
          : "\\u" + code.toString(16).padStart(4, "0");
        continue;
      }
      escaped += ch;
    }
    return escaped;
  } }.escape;

  Object.defineProperty(RegExp, "escape", {
    value: escape,
    writable: true,
    enumerable: false,
    configurable: true,
  });
})();
