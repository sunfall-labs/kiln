// kiln-builtins: node:url implementation (Phase 5.2, ADR 0005)

class URLSearchParamsPolyfill {
  private _entries: [string, string][] = [];

  constructor(init?: any) {
    if (!init) return;
    if (typeof init === "string") {
      let s = init;
      if (s.startsWith("?")) s = s.slice(1);
      if (s.length > 0) {
        const parts = s.split("&");
        for (let i = 0; i < parts.length; i++) {
          const part = parts[i];
          if (!part) continue;
          const eq = part.indexOf("=");
          const k = eq !== -1 ? decodeURIComponent(part.slice(0, eq).replace(/\+/g, " ")) : decodeURIComponent(part.replace(/\+/g, " "));
          const v = eq !== -1 ? decodeURIComponent(part.slice(eq + 1).replace(/\+/g, " ")) : "";
          this._entries.push([k, v]);
        }
      }
    } else if (Array.isArray(init)) {
      for (let i = 0; i < init.length; i++) {
        this._entries.push([String(init[i][0]), String(init[i][1])]);
      }
    } else if (typeof init === "object") {
      const keys = Object.keys(init);
      for (let i = 0; i < keys.length; i++) {
        const k = keys[i];
        this._entries.push([k, String(init[k])]);
      }
    }
  }

  append(name: string, value: string): void {
    this._entries.push([String(name), String(value)]);
  }

  delete(name: string): void {
    const n = String(name);
    this._entries = this._entries.filter(([k]) => k !== n);
  }

  get(name: string): string | null {
    const n = String(name);
    for (let i = 0; i < this._entries.length; i++) {
      if (this._entries[i][0] === n) return this._entries[i][1];
    }
    return null;
  }

  getAll(name: string): string[] {
    const n = String(name);
    const res: string[] = [];
    for (let i = 0; i < this._entries.length; i++) {
      if (this._entries[i][0] === n) res.push(this._entries[i][1]);
    }
    return res;
  }

  has(name: string): boolean {
    const n = String(name);
    for (let i = 0; i < this._entries.length; i++) {
      if (this._entries[i][0] === n) return true;
    }
    return false;
  }

  set(name: string, value: string): void {
    const n = String(name);
    const v = String(value);
    let found = false;
    const newEntries: [string, string][] = [];
    for (let i = 0; i < this._entries.length; i++) {
      if (this._entries[i][0] === n) {
        if (!found) {
          newEntries.push([n, v]);
          found = true;
        }
      } else {
        newEntries.push(this._entries[i]);
      }
    }
    if (!found) {
      newEntries.push([n, v]);
    }
    this._entries = newEntries;
  }

  sort(): void {
    this._entries.sort((a, b) => (a[0] < b[0] ? -1 : a[0] > b[0] ? 1 : 0));
  }

  toString(): string {
    const parts: string[] = [];
    for (let i = 0; i < this._entries.length; i++) {
      const k = encodeURIComponent(this._entries[i][0]).replace(/%20/g, "+");
      const v = encodeURIComponent(this._entries[i][1]).replace(/%20/g, "+");
      parts.push(`${k}=${v}`);
    }
    return parts.join("&");
  }

  forEach(callback: (value: string, key: string, parent: any) => void, thisArg?: any): void {
    for (let i = 0; i < this._entries.length; i++) {
      callback.call(thisArg, this._entries[i][1], this._entries[i][0], this);
    }
  }

  [Symbol.iterator]() {
    return this._entries[Symbol.iterator]();
  }

  entries() {
    return this._entries[Symbol.iterator]();
  }

  keys() {
    return this._entries.map(([k]) => k)[Symbol.iterator]();
  }

  values() {
    return this._entries.map(([, v]) => v)[Symbol.iterator]();
  }
}

export const URLSearchParams: any =
  typeof (globalThis as any).URLSearchParams !== "undefined"
    ? (globalThis as any).URLSearchParams
    : typeof (globalThis as any).URL === "function" && (globalThis as any).URL.prototype && "searchParams" in (globalThis as any).URL.prototype
      ? new (globalThis as any).URL("http://localhost/").searchParams.constructor
      : URLSearchParamsPolyfill;


export function fileURLToPath(url: any): string {
  if (typeof url === "object" && url !== null && "href" in url) {
    url = url.href;
  }
  if (typeof url !== "string") {
    throw new TypeError("The \"url\" argument must be of type string or an instance of URL");
  }
  if (!url.startsWith("file://")) {
    throw new TypeError("fileURLToPath requires a file: URL");
  }
  let path = url.slice(7);
  if (path.startsWith("localhost/")) {
    path = path.slice(9);
  }
  path = decodeURIComponent(path);
  if (path.length === 0 || path.charCodeAt(0) !== 47) {
    path = "/" + path;
  }
  return path;
}

export function pathToFileURL(filepath: string): URL {
  if (typeof filepath !== "string") {
    throw new TypeError("The \"filepath\" argument must be of type string");
  }
  let path = filepath;
  if (!path.startsWith("/")) {
    path = "/" + path;
  }
  const encoded = encodeURI(path);
  return new URL("file://" + encoded);
}

export function parse(
  urlStr: string,
  parseQueryString?: boolean,
  _slashesDenoteHost?: boolean
): Record<string, any> {
  if (typeof urlStr !== "string") {
    urlStr = String(urlStr);
  }
  const result: Record<string, any> = {
    protocol: null,
    slashes: null,
    auth: null,
    host: null,
    port: null,
    hostname: null,
    hash: null,
    search: null,
    query: null,
    pathname: null,
    path: null,
    href: urlStr,
  };

  const fragmentIdx = urlStr.indexOf("#");
  let rest = fragmentIdx !== -1 ? urlStr.slice(0, fragmentIdx) : urlStr;
  if (fragmentIdx !== -1) {
    result.hash = urlStr.slice(fragmentIdx);
  }

  const queryIdx = rest.indexOf("?");
  let pathname = queryIdx !== -1 ? rest.slice(0, queryIdx) : rest;
  if (queryIdx !== -1) {
    result.search = rest.slice(queryIdx);
    const queryStr = rest.slice(queryIdx + 1);
    if (parseQueryString) {
      const qObj: Record<string, any> = Object.create(null);
      const parts = queryStr.split("&");
      for (let i = 0; i < parts.length; i++) {
        const part = parts[i];
        if (part.length === 0) continue;
        const eq = part.indexOf("=");
        const k = eq !== -1 ? decodeURIComponent(part.slice(0, eq)) : decodeURIComponent(part);
        const v = eq !== -1 ? decodeURIComponent(part.slice(eq + 1)) : "";
        if (k in qObj) {
          if (Array.isArray(qObj[k])) {
            qObj[k].push(v);
          } else {
            qObj[k] = [qObj[k], v];
          }
        } else {
          qObj[k] = v;
        }
      }
      result.query = qObj;
    } else {
      result.query = queryStr;
    }
  }

  // Detect protocol and host
  const protoMatch = /^([a-zA-Z][a-zA-Z0-9+.-]*):/.exec(pathname);
  if (protoMatch) {
    result.protocol = protoMatch[1].toLowerCase() + ":";
    let afterProto = pathname.slice(protoMatch[0].length);
    if (afterProto.startsWith("//")) {
      result.slashes = true;
      afterProto = afterProto.slice(2);
      const slashIdx = afterProto.indexOf("/");
      const hostPart = slashIdx !== -1 ? afterProto.slice(0, slashIdx) : afterProto;
      pathname = slashIdx !== -1 ? afterProto.slice(slashIdx) : "/";

      const atIdx = hostPart.indexOf("@");
      let hostWithoutAuth = hostPart;
      if (atIdx !== -1) {
        result.auth = hostPart.slice(0, atIdx);
        hostWithoutAuth = hostPart.slice(atIdx + 1);
      }
      result.host = hostWithoutAuth;
      const colonIdx = hostWithoutAuth.lastIndexOf(":");
      if (colonIdx !== -1) {
        result.hostname = hostWithoutAuth.slice(0, colonIdx);
        result.port = hostWithoutAuth.slice(colonIdx + 1);
      } else {
        result.hostname = hostWithoutAuth;
      }
    } else {
      pathname = afterProto;
    }
  }

  result.pathname = pathname;
  result.path = pathname + (result.search || "");
  return result;
}

class URLPolyfill {
  href: string;
  origin: string;
  protocol: string;
  username: string;
  password: string;
  host: string;
  hostname: string;
  port: string;
  pathname: string;
  search: string;
  searchParams: any;
  hash: string;

  constructor(input: any, base?: any) {
    if (typeof input !== "string") {
      input = String(input);
    }
    let full = input;
    if (base !== undefined) {
      let baseStr =
        typeof base === "object" && base !== null && "href" in base
          ? base.href
          : String(base);
      const b = parse(baseStr);
      if (!b.protocol) {
        throw new TypeError("Invalid base URL");
      }
      if (input.startsWith("//")) {
        full = b.protocol + input;
      } else if (input.startsWith("/")) {
        full = `${b.protocol}//${b.host || ""}${input}`;
      } else if (/^[a-zA-Z][a-zA-Z0-9+.-]*:/.test(input)) {
        full = input;
      } else {
        let basePath = b.pathname || "/";
        const lastSlash = basePath.lastIndexOf("/");
        basePath = lastSlash !== -1 ? basePath.slice(0, lastSlash + 1) : "/";
        full = `${b.protocol}//${b.host || ""}${basePath}${input}`;
      }
    }

    const p = parse(full);
    if (!p.protocol) {
      throw new TypeError(`Invalid URL: ${input}`);
    }

    this.protocol = p.protocol;
    this.hostname = p.hostname || "";
    this.port = p.port || "";
    this.host =
      p.host || (this.port ? `${this.hostname}:${this.port}` : this.hostname);
    this.pathname = p.pathname || "/";
    this.search = p.search || "";
    this.hash = p.hash || "";
    this.username = "";
    this.password = "";
    if (p.auth) {
      const colon = p.auth.indexOf(":");
      if (colon !== -1) {
        this.username = p.auth.slice(0, colon);
        this.password = p.auth.slice(colon + 1);
      } else {
        this.username = p.auth;
      }
    }
    this.origin = `${this.protocol}//${this.host}`;
    this.href = `${this.protocol}//${p.auth ? p.auth + "@" : ""}${this.host}${this.pathname}${this.search}${this.hash}`;
    this.searchParams = new URLSearchParams(this.search);
  }

  toString(): string {
    return this.href;
  }

  toJSON(): string {
    return this.href;
  }
}

// `require("node:url").URL` is the global `URL` in node, one object under both names; the
// shared TypeScript class (`kiln-builtins/src/url.ts`) publishes it in both engines before this
// module runs. The polyfill above stands in only where no global exists.
export const URL: any =
  typeof (globalThis as any).URL === "function" ? (globalThis as any).URL : URLPolyfill;

export function format(urlObj: any): string {
  if (typeof urlObj === "string") {
    return urlObj;
  }
  if (urlObj instanceof URL) {
    return urlObj.toString();
  }
  let out = "";
  if (urlObj.protocol) {
    out += urlObj.protocol;
    if (!urlObj.protocol.endsWith(":")) {
      out += ":";
    }
  }
  if (urlObj.slashes || urlObj.protocol) {
    out += "//";
  }
  if (urlObj.auth) {
    out += urlObj.auth + "@";
  }
  if (urlObj.host) {
    out += urlObj.host;
  } else if (urlObj.hostname) {
    out += urlObj.hostname;
    if (urlObj.port) {
      out += ":" + urlObj.port;
    }
  }
  if (urlObj.pathname) {
    out += urlObj.pathname;
  }
  if (urlObj.search) {
    out += urlObj.search.startsWith("?") ? urlObj.search : "?" + urlObj.search;
  } else if (urlObj.query) {
    if (typeof urlObj.query === "object") {
      const parts: string[] = [];
      const keys = Object.keys(urlObj.query);
      for (let i = 0; i < keys.length; i++) {
        const k = keys[i];
        parts.push(encodeURIComponent(k) + "=" + encodeURIComponent(urlObj.query[k]));
      }
      out += "?" + parts.join("&");
    } else {
      out += "?" + urlObj.query;
    }
  }
  if (urlObj.hash) {
    out += urlObj.hash.startsWith("#") ? urlObj.hash : "#" + urlObj.hash;
  }
  return out;
}

export function urlToHttpOptions(url: any): any {
  const options: Record<string, any> = {
    protocol: url.protocol,
    hostname:
      url.hostname && url.hostname.startsWith("[")
        ? url.hostname.slice(1, -1)
        : url.hostname,
    hash: url.hash,
    search: url.search,
    pathname: url.pathname,
    path: `${url.pathname || ""}${url.search || ""}`,
    href: url.href,
  };
  if (url.port !== "" && url.port !== undefined && url.port !== null) {
    options.port = Number(url.port);
  }
  if (url.username || url.password) {
    options.auth = `${decodeURIComponent(url.username || "")}:${decodeURIComponent(url.password || "")}`;
  }
  return options;
}

export function domainToASCII(domain: string): string {
  return String(domain);
}

export function domainToUnicode(domain: string): string {
  return String(domain);
}

const urlModule = {
  URL,
  URLSearchParams,
  domainToASCII,
  domainToUnicode,
  fileURLToPath,
  format,
  parse,
  pathToFileURL,
  urlToHttpOptions,
  default: undefined as any,
};
urlModule.default = urlModule;

if (typeof (globalThis as any).URL !== "function") {
  (globalThis as any).URL = URL;
}
if (typeof (globalThis as any).URLSearchParams === "undefined") {
  (globalThis as any).URLSearchParams = URLSearchParams;
}

export default urlModule;
