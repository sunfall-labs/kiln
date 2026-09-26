// kiln-builtins: `RegExp.prototype` methods and `RegExp.escape` (ECMAScript specification / ES2025)
// ADR 0005, ADR 0063 rule 4

(function installRegExp() {
  if (typeof RegExp === "undefined" || !RegExp.prototype) return;
  const Proto = RegExp.prototype as any;

  // Annex B.3 method `compile(pattern, flags?)`:
  function compile(this: any, pattern: any, flags?: any): RegExp {
    if (!(this instanceof RegExp)) {
      throw new TypeError("RegExp.prototype.compile called on incompatible receiver");
    }
    return new RegExp(pattern, flags);
  }

  Object.defineProperty(Proto, "compile", {
    value: compile,
    writable: true,
    enumerable: false,
    configurable: true,
  });

  // Static `RegExp.escape(text)` (ES2025):
  function escape(text: any): string {
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

      if (punctuators.includes(ch) || ch === " ") {
        escaped += "\\x" + code.toString(16).padStart(2, "0");
        continue;
      }
      escaped += ch;
    }
    return escaped;
  }

  Object.defineProperty(RegExp, "escape", {
    value: escape,
    writable: true,
    enumerable: false,
    configurable: true,
  });
})();
