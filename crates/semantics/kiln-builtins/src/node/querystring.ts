// kiln-builtins: node:querystring implementation (Phase 5.2, ADR 0005)

function hexVal(code: number): number {
  if (code >= 48 && code <= 57) return code - 48; // '0'-'9'
  if (code >= 65 && code <= 70) return code - 55; // 'A'-'F'
  if (code >= 97 && code <= 102) return code - 87; // 'a'-'f'
  return -1;
}

function decodeFormComponent(input: string): string {
  const len = input.length;
  if (len === 0) return "";

  // Fast-path: check if any decoding or replacement is necessary
  let needsDecode = false;
  for (let idx = 0; idx < len; idx++) {
    const ch = input.charCodeAt(idx);
    if (ch === 43 || ch === 37 || ch >= 128) {
      needsDecode = true;
      break;
    }
  }
  if (!needsDecode) {
    return input;
  }

  // Convert input string and percent-escapes into raw byte array
  const bytes: number[] = [];
  let i = 0;
  while (i < len) {
    const ch = input.charCodeAt(i);
    if (ch === 43) {
      // '+' -> ' ' (0x20)
      bytes.push(32);
      i++;
    } else if (ch === 37 && i + 2 < len) {
      // '%' -> check hex digits
      const h = hexVal(input.charCodeAt(i + 1));
      const l = hexVal(input.charCodeAt(i + 2));
      if (h >= 0 && l >= 0) {
        bytes.push((h << 4) | l);
        i += 3;
      } else {
        bytes.push(37);
        i++;
      }
    } else if (ch < 128) {
      bytes.push(ch);
      i++;
    } else {
      // Non-ASCII character present in source string: encode to UTF-8
      if (ch >= 0xd800 && ch <= 0xdbff && i + 1 < len) {
        const next = input.charCodeAt(i + 1);
        if (next >= 0xdc00 && next <= 0xdfff) {
          const cp = ((ch - 0xd800) << 10) + (next - 0xdc00) + 0x10000;
          bytes.push(0xf0 | (cp >> 18));
          bytes.push(0x80 | ((cp >> 12) & 0x3f));
          bytes.push(0x80 | ((cp >> 6) & 0x3f));
          bytes.push(0x80 | (cp & 0x3f));
          i += 2;
          continue;
        }
      }
      if (ch < 0x800) {
        bytes.push(0xc0 | (ch >> 6));
        bytes.push(0x80 | (ch & 0x3f));
      } else {
        bytes.push(0xe0 | (ch >> 12));
        bytes.push(0x80 | ((ch >> 6) & 0x3f));
        bytes.push(0x80 | (ch & 0x3f));
      }
      i++;
    }
  }

  // UTF-8 decoding with replacement character (\uFFFD) for invalid sequences
  let out = "";
  let j = 0;
  const byteLen = bytes.length;
  while (j < byteLen) {
    const b1 = bytes[j];
    if (b1 < 0x80) {
      out += String.fromCharCode(b1);
      j++;
    } else if (b1 >= 0xc2 && b1 <= 0xdf) {
      if (j + 1 < byteLen) {
        const b2 = bytes[j + 1];
        if ((b2 & 0xc0) === 0x80) {
          out += String.fromCharCode(((b1 & 0x1f) << 6) | (b2 & 0x3f));
          j += 2;
          continue;
        }
      }
      out += "\uFFFD";
      j++;
    } else if (b1 >= 0xe0 && b1 <= 0xef) {
      if (j + 2 < byteLen) {
        const b2 = bytes[j + 1];
        const b3 = bytes[j + 2];
        if (
          (b2 & 0xc0) === 0x80 &&
          (b3 & 0xc0) === 0x80 &&
          (b1 !== 0xe0 || b2 >= 0xa0) &&
          (b1 !== 0xed || b2 <= 0x9f)
        ) {
          out += String.fromCharCode(
            ((b1 & 0x0f) << 12) | ((b2 & 0x3f) << 6) | (b3 & 0x3f)
          );
          j += 3;
          continue;
        }
      }
      out += "\uFFFD";
      j++;
    } else if (b1 >= 0xf0 && b1 <= 0xf4) {
      if (j + 3 < byteLen) {
        const b2 = bytes[j + 1];
        const b3 = bytes[j + 2];
        const b4 = bytes[j + 3];
        if (
          (b2 & 0xc0) === 0x80 &&
          (b3 & 0xc0) === 0x80 &&
          (b4 & 0xc0) === 0x80 &&
          (b1 !== 0xf0 || b2 >= 0x90) &&
          (b1 !== 0xf4 || b2 <= 0x8f)
        ) {
          const cp =
            ((b1 & 0x07) << 18) |
            ((b2 & 0x3f) << 12) |
            ((b3 & 0x3f) << 6) |
            (b4 & 0x3f);
          out += String.fromCodePoint(cp);
          j += 4;
          continue;
        }
      }
      out += "\uFFFD";
      j++;
    } else {
      out += "\uFFFD";
      j++;
    }
  }

  return out;
}

export function unescape(str: string): string {
  if (typeof str !== "string") {
    str = String(str);
  }
  return decodeFormComponent(str);
}

function setProp(obj: Record<string, any>, key: string, val: any) {
  if (key === "__proto__") {
    Object.defineProperty(obj, "__proto__", {
      value: val,
      writable: true,
      enumerable: true,
      configurable: true,
    });
  } else {
    obj[key] = val;
  }
}

export function parse(
  str?: any,
  sep?: any,
  eq?: any,
  options?: any
): Record<string, any> {
  const result: Record<string, any> = Object.create(null);
  if (typeof str !== "string") {
    if (str === null || str === undefined || typeof str === "boolean") {
      return result;
    }
    str = String(str);
  }
  if (str.length === 0) {
    return result;
  }

  const separator = typeof sep === "string" && sep.length > 0 ? sep : "&";
  const equality = typeof eq === "string" && eq.length > 0 ? eq : "=";

  let decodeFn = decodeFormComponent;
  let maxKeys = 1000;
  if (options && typeof options === "object") {
    if (typeof options.decodeURIComponent === "function") {
      decodeFn = options.decodeURIComponent;
    }
    if (typeof options.maxKeys === "number") {
      maxKeys = options.maxKeys;
    }
  }

  const parts = str.split(separator);
  const total = parts.length;
  let count = 0;

  for (let i = 0; i < total; i++) {
    const part = parts[i];
    if (part.length === 0) {
      continue;
    }
    if (maxKeys > 0 && count >= maxKeys) {
      break;
    }

    let key: string;
    let val: string;
    const eqIdx = part.indexOf(equality);
    if (eqIdx !== -1) {
      key = decodeFn(part.slice(0, eqIdx));
      val = decodeFn(part.slice(eqIdx + equality.length));
    } else {
      key = decodeFn(part);
      val = "";
    }

    if (Object.prototype.hasOwnProperty.call(result, key)) {
      const prev = result[key];
      if (Array.isArray(prev)) {
        prev.push(val);
      } else {
        setProp(result, key, [prev, val]);
      }
    } else {
      setProp(result, key, val);
      count++;
    }
  }

  return result;
}


export const decode = parse;

function stringifyPrimitive(v: any): string {
  if (typeof v === "string") return v;
  if (typeof v === "number" && isFinite(v)) return String(v);
  if (typeof v === "boolean") return String(v);
  if (typeof v === "bigint") return String(v);
  return "";
}

export function escape(str: any): string {
  if (typeof str !== "string") {
    str = String(str);
  }
  return encodeURIComponent(str);
}

export function stringify(
  obj?: any,
  sep?: any,
  eq?: any,
  options?: any
): string {
  if (obj === null || typeof obj !== "object") {
    return "";
  }
  const separator = typeof sep === "string" && sep.length > 0 ? sep : "&";
  const equality = typeof eq === "string" && eq.length > 0 ? eq : "=";

  let encodeFn = escape;
  if (
    options &&
    typeof options === "object" &&
    typeof options.encodeURIComponent === "function"
  ) {
    encodeFn = options.encodeURIComponent;
  }

  const keys = Object.keys(obj);
  const pairs: string[] = [];
  for (let i = 0; i < keys.length; i++) {
    const key = keys[i];
    const val = obj[key];
    const encodedKey = encodeFn(key);
    if (Array.isArray(val)) {
      for (let j = 0; j < val.length; j++) {
        const item = val[j];
        pairs.push(encodedKey + equality + encodeFn(stringifyPrimitive(item)));
      }
    } else {
      pairs.push(encodedKey + equality + encodeFn(stringifyPrimitive(val)));
    }
  }
  return pairs.join(separator);
}

export const encode = stringify;

const querystring = {
  decode,
  encode,
  escape,
  parse,
  stringify,
  unescape,
  default: undefined as any,
};
querystring.default = querystring;

export default querystring;
