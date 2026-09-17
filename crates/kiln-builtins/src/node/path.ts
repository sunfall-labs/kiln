// kiln-builtins/src/node/path.ts
// Typed TypeScript implementation of node:path for Kiln (Phase 5.2).

export const sep = "/";
export const delimiter = ":";

export function isAbsolute(p?: string): boolean {
  if (p === undefined) return false;
  return String(p).startsWith("/");
}

export function dirname(path: string): string {
  path = path === undefined ? "" : String(path);
  const len = path.length;
  if (len === 0) return ".";
  const hasRoot = path.charCodeAt(0) === 47; // '/'
  let end = -1;
  let matchedSlash = true;
  for (let i = len - 1; i >= 1; i--) {
    if (path.charCodeAt(i) === 47) {
      if (!matchedSlash) {
        end = i;
        break;
      }
    } else {
      matchedSlash = false;
    }
  }
  if (end === -1) {
    return hasRoot ? "/" : ".";
  }
  if (end === 1 && hasRoot) {
    return "//";
  }
  return path.slice(0, end);
}

export function basename(path: string, suffix?: string): string {
  path = path === undefined ? "" : String(path);
  let end = path.length;
  while (end > 0 && path.charCodeAt(end - 1) === 47) {
    end--;
  }
  let start = end;
  while (start > 0 && path.charCodeAt(start - 1) !== 47) {
    start--;
  }
  const base = path.slice(start, end);
  if (!suffix) {
    return base;
  }
  const s = String(suffix);
  if (s.length === 0 || s.length > path.length) {
    return base;
  }
  if (s === path) {
    return "";
  }
  if (base.endsWith(s)) {
    const stripped = base.slice(0, base.length - s.length);
    if (stripped.length > 0) {
      return stripped;
    }
  }
  return base;
}

export function extname(path: string): string {
  path = path === undefined ? "" : String(path);
  const len = path.length;
  let startDot = -1;
  let startPart = 0;
  let end = -1;
  let matchedSlash = true;
  let preDotState = 0;
  for (let i = len - 1; i >= 0; i--) {
    const code = path.charCodeAt(i);
    if (code === 47) { // '/'
      if (!matchedSlash) {
        startPart = i + 1;
        break;
      }
      continue;
    }
    if (end === -1) {
      matchedSlash = false;
      end = i + 1;
    }
    if (code === 46) { // '.'
      if (startDot === -1) {
        startDot = i;
      } else if (preDotState !== 1) {
        preDotState = 1;
      }
    } else if (startDot !== -1) {
      preDotState = -1;
    }
  }
  if (startDot === -1 || end === -1) {
    return "";
  }
  if (
    preDotState === 0 ||
    (preDotState === 1 && startDot === end - 1 && startDot === startPart + 1)
  ) {
    return "";
  }
  return path.slice(startDot, end);
}

export function normalize(path: string): string {
  path = path === undefined ? "" : String(path);
  if (path.length === 0) return ".";
  const absolute = path.charCodeAt(0) === 47;
  const trailing = path.charCodeAt(path.length - 1) === 47;
  const rawParts = path.split("/");
  const parts: string[] = [];
  for (let i = 0; i < rawParts.length; i++) {
    const part = rawParts[i];
    if (part === "" || part === ".") {
      continue;
    }
    if (part === "..") {
      if (parts.length > 0 && parts[parts.length - 1] !== "..") {
        parts.pop();
      } else if (!absolute) {
        parts.push("..");
      }
    } else {
      parts.push(part);
    }
  }
  let normalized = parts.join("/");
  if (absolute) {
    normalized = "/" + normalized;
  }
  if (normalized.length === 0) {
    normalized = ".";
  }
  if (trailing && normalized !== "/" && normalized !== ".") {
    normalized += "/";
  }
  return normalized;
}

export function join(...paths: string[]): string {
  if (paths.length === 0) return ".";
  const strings: string[] = [];
  for (let i = 0; i < paths.length; i++) {
    strings.push(String(paths[i]));
  }
  return normalize(strings.join("/"));
}

export function resolve(...paths: string[]): string {
  let resolved = "";
  for (let i = 0; i < paths.length; i++) {
    const part = String(paths[i]);
    if (part.startsWith("/")) {
      resolved = part;
    } else if (part.length > 0) {
      if (resolved.length > 0 && !resolved.endsWith("/")) {
        resolved += "/";
      }
      resolved += part;
    }
  }
  if (!resolved.startsWith("/")) {
    resolved = "/" + resolved;
  }
  return normalize(resolved);
}

export function relative(from: string, to: string): string {
  from = from === undefined ? "" : String(from);
  to = to === undefined ? "" : String(to);
  const splitPath = (p: string): string[] => {
    const norm = normalize(p);
    const raw = norm.split("/");
    const res: string[] = [];
    for (let i = 0; i < raw.length; i++) {
      const item = raw[i];
      if (item !== "" && item !== ".") {
        res.push(item);
      }
    }
    return res;
  };
  const fromParts = splitPath(from);
  const toParts = splitPath(to);
  let shared = 0;
  const minLen = Math.min(fromParts.length, toParts.length);
  while (shared < minLen && fromParts[shared] === toParts[shared]) {
    shared++;
  }
  const parts: string[] = [];
  for (let i = 0; i < fromParts.length - shared; i++) {
    parts.push("..");
  }
  for (let i = shared; i < toParts.length; i++) {
    parts.push(toParts[i]);
  }
  return parts.join("/");
}

interface ParsedPath {
  root: string;
  dir: string;
  base: string;
  ext: string;
  name: string;
}

export function parse(path: string): ParsedPath {
  path = path === undefined ? "" : String(path);
  const b = basename(path);
  const e = extname(path);
  let n = b;
  if (e.length > 0 && b.endsWith(e)) {
    n = b.slice(0, b.length - e.length);
  }
  return {
    root: path.startsWith("/") ? "/" : "",
    dir: dirname(path),
    base: b,
    ext: e,
    name: n,
  };
}

interface FormatInputPathObject {
  root?: string;
  dir?: string;
  base?: string;
  ext?: string;
  name?: string;
}

export function format(pathObject: FormatInputPathObject): string {
  if (!pathObject || typeof pathObject !== "object") {
    return "";
  }
  const dirVal = (pathObject as any).dir;
  const rootVal = (pathObject as any).root;
  const baseVal = (pathObject as any).base;
  const nameVal = (pathObject as any).name;
  const extVal = (pathObject as any).ext;

  const dir = dirVal !== undefined && dirVal !== null ? String(dirVal) : "";
  const root = rootVal !== undefined && rootVal !== null ? String(rootVal) : "";
  const base = baseVal !== undefined && baseVal !== null ? String(baseVal) : "";
  const name = nameVal !== undefined && nameVal !== null ? String(nameVal) : "";
  const ext = extVal !== undefined && extVal !== null ? String(extVal) : "";

  const directory = dir.length > 0 ? dir : root;
  const resolvedBase = base.length > 0 ? base : name + ext;

  if (directory.length === 0) {
    return resolvedBase;
  }
  if (directory === "/") {
    return "/" + resolvedBase;
  }
  return directory + "/" + resolvedBase;
}

export function toNamespacedPath(p: string): string {
  return typeof p === "string" ? p : "";
}

const path = {
  basename,
  delimiter,
  dirname,
  extname,
  format,
  isAbsolute,
  join,
  normalize,
  parse,
  posix: undefined as any,
  relative,
  resolve,
  sep,
  toNamespacedPath,
  win32: undefined as any,
  default: undefined as any,
};
path.posix = path;
path.default = path;

export const win32: any = {
  basename: (p: string, ext?: string) => basename(p ? p.replace(/\\/g, "/") : "", ext),
  delimiter: ";",
  dirname: (p: string) => dirname(p ? p.replace(/\\/g, "/") : "").replace(/\//g, "\\"),
  extname,
  format,
  isAbsolute: (p?: string) => {
    if (!p) return false;
    return /^[a-zA-Z]:[\\/]/.test(p) || p.startsWith("\\\\");
  },
  join: (...args: string[]) => join(...args).replace(/\//g, "\\"),
  normalize: (p: string) => normalize(p ? p.replace(/\\/g, "/") : "").replace(/\//g, "\\"),
  parse,
  posix: path,
  relative,
  resolve: (...args: string[]) => resolve(...args).replace(/\//g, "\\"),
  sep: "\\",
  toNamespacedPath: (p: string) => {
    if (typeof p !== "string" || p.length === 0) return "";
    if (p.startsWith("\\\\?\\")) return p;
    if (p.startsWith("\\\\")) return "\\\\?\\UNC\\" + p.slice(2);
    if (/^[a-zA-Z]:/.test(p)) return "\\\\?\\" + p;
    return p;
  },
  win32: undefined as any,
  default: undefined as any,
};
win32.win32 = win32;
win32.default = win32;
path.win32 = win32;

export const posix = path;
export default path;
