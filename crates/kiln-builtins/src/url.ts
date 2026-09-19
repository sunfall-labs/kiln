// kiln-builtins: `URL` and `URLSearchParams` (ADR 0063 rule 4, ADR 0025).
//
// The parsing is the `url` crate's, reached through two intrinsics the host installs before this
// runs -- `__kilnUrlParse(input, base)` answers the eleven WHATWG components NUL-separated,
// `__kilnUrlSet(href, field, value)` answers the record after one setter -- so a URL here is
// what the interpreter's `url` crate says it is, and what node says it is, with no second parser.
//
// `__kilnUrlFast(input, base)` is the request fast path (plan step B.3): inside a `node:http`
// handler the first `new URL(req.url, ...)` is answered with an object the runtime built for the
// response, carrying `pathname`, a `searchParams` whose `get` reads the raw query, and this
// class's prototype. Its other getters parse from `_input`/`_base` on first use, so a handler
// that touches the URL lightly allocates nothing and one that reads `host` still gets it.

const __urlParse = (globalThis as any).__kilnUrlParse;
const __urlSet = (globalThis as any).__kilnUrlSet;
const __urlFast = (globalThis as any).__kilnUrlFast;
const __urlAdopt = (globalThis as any).__kilnUrlAdopt;
delete (globalThis as any).__kilnUrlParse;
delete (globalThis as any).__kilnUrlSet;
delete (globalThis as any).__kilnUrlFast;
delete (globalThis as any).__kilnUrlAdopt;

// Record fields, in the order the provider spills them.
const HREF = 0, ORIGIN = 1, PROTOCOL = 2, USERNAME = 3, PASSWORD = 4, HOST = 5, HOSTNAME = 6,
  PORT = 7, PATHNAME = 8, SEARCH = 9, HASH = 10;
// Setter fields, as `__kilnUrlSet` numbers them.
const SET_HREF = 0, SET_PROTOCOL = 1, SET_USERNAME = 2, SET_PASSWORD = 3, SET_HOST = 4,
  SET_HOSTNAME = 5, SET_PORT = 6, SET_PATHNAME = 7, SET_SEARCH = 8, SET_HASH = 9;

function parseRecord(input: any, base: any): string[] {
  const record: string = __urlParse(String(input), base === undefined ? undefined : String(base));
  return record.split("\0");
}

// application/x-www-form-urlencoded, both directions (WHATWG URL §5).
function decodeForm(text: string): string {
  const spaced = text.replace(/\+/g, " ");
  try {
    return decodeURIComponent(spaced);
  } catch {
    // A malformed escape decodes byte by byte, leaving what is not a valid sequence alone.
    return spaced.replace(/%[0-9A-Fa-f]{2}/g, (m) => {
      try { return decodeURIComponent(m); } catch { return m; }
    });
  }
}

function encodeForm(text: string): string {
  return encodeURIComponent(text)
    .replace(/[!'()~]/g, (c) => "%" + c.charCodeAt(0).toString(16).toUpperCase())
    .replace(/%20/g, "+");
}

function parseQuery(query: string): [string, string][] {
  const list: [string, string][] = [];
  if (query === "") return list;
  for (const part of query.split("&")) {
    if (part === "") continue;
    const eq = part.indexOf("=");
    const name = eq === -1 ? part : part.slice(0, eq);
    const value = eq === -1 ? "" : part.slice(eq + 1);
    list.push([decodeForm(name), decodeForm(value)]);
  }
  return list;
}

function serializeQuery(list: [string, string][]): string {
  let out = "";
  for (let i = 0; i < list.length; i++) {
    if (i > 0) out += "&";
    out += encodeForm(list[i][0]) + "=" + encodeForm(list[i][1]);
  }
  return out;
}

// Instance state lives under one registry symbol rather than in named fields, so `Object.keys`
// and enumeration see nothing, as node's do. The request fast path's object is built in C with
// own `_input`/`_base` (and `_query` on its `searchParams`) instead -- the C side spells this
// symbol as `@@symbol-for:kiln.url` and could carry the state itself, but then every request
// would allocate it; `state()` builds it on the first read that needs it.
const STATE = Symbol.for("kiln.url");

type ParamsState = { list: [string, string][] | undefined; url: any; query: any };
type UrlState = { c: string[] | undefined; sp: any; input: any; base: any };

// The request fast path's objects outlive one request -- a keep-alive socket's response is
// refilled with the next request's `_input` and `_query` -- so the state cached here records
// what it was built from and is rebuilt when that has moved on.
function paramsState(self: any): ParamsState {
  let st: ParamsState = self[STATE];
  const raw = self._query;
  if (st === undefined || (raw !== undefined && st.query !== raw)) {
    st = { list: undefined, url: undefined, query: raw };
    self[STATE] = st;
  }
  return st;
}

function paramsList(self: any): [string, string][] {
  const st = paramsState(self);
  if (st.list === undefined) {
    st.list = parseQuery(st.query === undefined ? "" : String(st.query));
  }
  return st.list;
}

// A mutation on the fast-path object also drops its own raw-query `get`, which would otherwise
// keep answering the unmodified query.
function paramsMutated(self: any): void {
  if (Object.prototype.hasOwnProperty.call(self, "get")) delete self.get;
  const st = paramsState(self);
  if (st.url !== undefined) setSearchFromParams(st.url, self.toString());
}

class URLSearchParams {
  constructor(init?: any) {
    const st: ParamsState = { list: [], url: undefined, query: undefined };
    this[STATE] = st;
    if (init === undefined || init === null) return;
    if (typeof init === "string") {
      st.list = parseQuery(init.startsWith("?") ? init.slice(1) : init);
    } else if (typeof init === "object" || typeof init === "function") {
      if (typeof init[Symbol.iterator] === "function") {
        for (const pair of init) {
          if (pair === null || typeof pair !== "object" || typeof pair[Symbol.iterator] !== "function") {
            throw new TypeError("Each query pair must be an iterable [name, value] tuple");
          }
          const items = [...pair];
          if (items.length !== 2) {
            throw new TypeError("Each query pair must be an iterable [name, value] tuple");
          }
          st.list!.push([String(items[0]), String(items[1])]);
        }
      } else {
        for (const key of Object.keys(init)) {
          st.list!.push([String(key), String(init[key])]);
        }
      }
    } else {
      st.list = parseQuery(String(init));
    }
  }

  get size(): number {
    return paramsList(this).length;
  }

  append(name: any, value: any): void {
    paramsList(this).push([String(name), String(value)]);
    paramsMutated(this);
  }

  delete(name: any, value?: any): void {
    const key = String(name);
    const list = paramsList(this);
    paramsState(this).list = value === undefined
      ? list.filter((pair) => pair[0] !== key)
      : list.filter((pair) => !(pair[0] === key && pair[1] === String(value)));
    paramsMutated(this);
  }

  get(name: any): string | null {
    const key = String(name);
    for (const pair of paramsList(this)) if (pair[0] === key) return pair[1];
    return null;
  }

  getAll(name: any): string[] {
    const key = String(name);
    const out: string[] = [];
    for (const pair of paramsList(this)) if (pair[0] === key) out.push(pair[1]);
    return out;
  }

  has(name: any, value?: any): boolean {
    const key = String(name);
    for (const pair of paramsList(this)) {
      if (pair[0] === key && (value === undefined || pair[1] === String(value))) return true;
    }
    return false;
  }

  set(name: any, value: any): void {
    const key = String(name);
    const text = String(value);
    let seen = false;
    const next: [string, string][] = [];
    for (const pair of paramsList(this)) {
      if (pair[0] === key) {
        if (!seen) {
          next.push([key, text]);
          seen = true;
        }
      } else {
        next.push(pair);
      }
    }
    if (!seen) next.push([key, text]);
    paramsState(this).list = next;
    paramsMutated(this);
  }

  sort(): void {
    // Stable, by name only, comparing code units as the specification says.
    const indexed = paramsList(this).map((pair, index) => ({ pair, index }));
    indexed.sort((a, b) => (a.pair[0] < b.pair[0] ? -1 : a.pair[0] > b.pair[0] ? 1 : a.index - b.index));
    paramsState(this).list = indexed.map((entry) => entry.pair);
    paramsMutated(this);
  }

  toString(): string {
    return serializeQuery(paramsList(this));
  }

  forEach(callback: any, thisArg?: any): void {
    if (typeof callback !== "function") {
      throw new TypeError("The \"callback\" argument must be of type function");
    }
    for (const pair of paramsList(this).slice()) callback.call(thisArg, pair[1], pair[0], this);
  }

  *entries(): IterableIterator<[string, string]> {
    for (const pair of paramsList(this).slice()) yield [pair[0], pair[1]];
  }

  *keys(): IterableIterator<string> {
    for (const pair of paramsList(this).slice()) yield pair[0];
  }

  *values(): IterableIterator<string> {
    for (const pair of paramsList(this).slice()) yield pair[1];
  }

  [Symbol.iterator](): IterableIterator<[string, string]> {
    return this.entries();
  }
}
Object.defineProperty(URLSearchParams.prototype, Symbol.toStringTag, { value: "URLSearchParams", configurable: true });

function urlState(self: any): UrlState {
  let st: UrlState = self[STATE];
  const input = self._input;
  if (st === undefined || (input !== undefined && st.input !== input)) {
    st = { c: undefined, sp: undefined, input, base: self._base };
    self[STATE] = st;
  }
  return st;
}

function components(self: any): string[] {
  const st = urlState(self);
  if (st.c === undefined) st.c = parseRecord(st.input, st.base);
  return st.c;
}

function applySetter(self: any, field: number, value: any): void {
  const st = urlState(self);
  const record: string = __urlSet(components(self)[HREF], field, String(value));
  st.c = record.split("\0");
  if (st.sp !== undefined && field !== SET_SEARCH) {
    paramsState(st.sp).list = parseQuery(st.c[SEARCH].slice(1));
  }
}

function refreshParams(self: any): void {
  const st = urlState(self);
  if (st.sp !== undefined) paramsState(st.sp).list = parseQuery(st.c![SEARCH].slice(1));
}

// A `searchParams` mutation: the search follows the list, without rebuilding the list.
function setSearchFromParams(self: any, query: string): void {
  const st = urlState(self);
  const record: string = __urlSet(components(self)[HREF], SET_SEARCH, query);
  st.c = record.split("\0");
}

class URL {
  constructor(input: any, base?: any) {
    const fast = __urlFast(input, base);
    if (fast !== undefined) return fast;
    // Hoisted on purpose: written as `{ c: parseRecord(input, base), sp: undefined, ... }` this
    // literal's `c` read back `undefined` when the bootstrap is emitted generic-only, while the
    // same class as a user program under `native-release-size` is fine. Filed as an emitter
    // question (.scratch/full-runtime/issues/05); do not fold it back until that is answered.
    const rec = parseRecord(input, base);
    this[STATE] = { c: rec, sp: undefined, input: undefined, base: undefined };
  }

  static canParse(input: any, base?: any): boolean {
    try {
      parseRecord(input, base);
      return true;
    } catch {
      return false;
    }
  }

  static parse(input: any, base?: any): URL | null {
    try {
      return new URL(input, base);
    } catch {
      return null;
    }
  }

  get href(): string { return components(this)[HREF]; }
  set href(value: any) { applySetter(this, SET_HREF, value); refreshParams(this); }
  get origin(): string { return components(this)[ORIGIN]; }
  get protocol(): string { return components(this)[PROTOCOL]; }
  set protocol(value: any) { applySetter(this, SET_PROTOCOL, value); }
  get username(): string { return components(this)[USERNAME]; }
  set username(value: any) { applySetter(this, SET_USERNAME, value); }
  get password(): string { return components(this)[PASSWORD]; }
  set password(value: any) { applySetter(this, SET_PASSWORD, value); }
  get host(): string { return components(this)[HOST]; }
  set host(value: any) { applySetter(this, SET_HOST, value); }
  get hostname(): string { return components(this)[HOSTNAME]; }
  set hostname(value: any) { applySetter(this, SET_HOSTNAME, value); }
  get port(): string { return components(this)[PORT]; }
  set port(value: any) { applySetter(this, SET_PORT, value); }
  get pathname(): string { return components(this)[PATHNAME]; }
  set pathname(value: any) { applySetter(this, SET_PATHNAME, value); }
  get search(): string { return components(this)[SEARCH]; }
  set search(value: any) { applySetter(this, SET_SEARCH, value); refreshParams(this); }
  get hash(): string { return components(this)[HASH]; }
  set hash(value: any) { applySetter(this, SET_HASH, value); }

  get searchParams(): URLSearchParams {
    const st = urlState(this);
    if (st.sp === undefined) {
      const params = new URLSearchParams(components(this)[SEARCH]);
      paramsState(params).url = this;
      st.sp = params;
    }
    return st.sp;
  }

  toString(): string { return this.href; }
  toJSON(): string { return this.href; }
}
Object.defineProperty(URL.prototype, Symbol.toStringTag, { value: "URL", configurable: true });

__urlAdopt(URL, URL.prototype, URLSearchParams.prototype);
(globalThis as any).URL = URL;
(globalThis as any).URLSearchParams = URLSearchParams;
