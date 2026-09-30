// kiln-builtins: `RegExp.prototype` methods and `RegExp.escape` (ECMAScript specification / ES2025)
// ADR 0005, ADR 0063 rule 4

(function installRegExp() {
  if (typeof RegExp === "undefined" || !RegExp.prototype) return;
  const Proto = RegExp.prototype as any;

  // Annex B legacy capture properties. The native backend records these per realm as part of
  // each RegExp match; keep the public accessors in this TypeScript bootstrap so the runtime C
  // only owns match storage, not a JavaScript builtin implementation.
  const legacyCapture = (globalThis as any).__kilnRegExpLegacyCapture;
  delete (globalThis as any).__kilnRegExpLegacyCapture;
  for (let index = 1; index <= 9; index++) {
    Object.defineProperty(RegExp, `$${index}`, {
      get: legacyCapture === undefined ? () => "" : () => legacyCapture(index) ?? "",
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
