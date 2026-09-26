// kiln-builtins: WHATWG Web Standards (fetch, Headers, Request, Response, FormData, Blob)
// Phase 5.2, ADR 0005

// Declared here rather than in the class body (see the note above `Response`).
interface Headers {
  // Lowercased name to value, the values of a repeated name joined with ", ". Null while the
  // headers are still the flattened block a request arrived with (`_flat`): reading one field
  // scans that block, and only a write or an iteration builds the map.
  _map: Map<string, string> | null;
  // `name NUL value NUL` per field, names lowercased and values trimmed; null once `_map` exists.
  _flat: string | null;
  // Every `set-cookie` value, which `_map` joins and `getSetCookie` must not. Null for none.
  _setCookies: string[] | null;
}

class Headers {
  constructor(init?: any) {
    this._map = new Map();
    this._flat = null;
    this._setCookies = null;
    if (!init) return;
    if (init instanceof Headers) {
      const source = init._materialize();
      this._map = new Map(source);
      if (init._setCookies !== null) this._setCookies = init._setCookies.slice();
    } else if (Array.isArray(init)) {
      for (let i = 0; i < init.length; i++) {
        const entry = init[i];
        if (Array.isArray(entry) && entry.length >= 2) {
          this.append(entry[0], entry[1]);
        }
      }
    } else if (typeof init === "object") {
      // Own enumerable string keys, which is what `for...in` filtered by `hasOwnProperty` gave.
      const keys = Object.keys(init);
      for (let i = 0; i < keys.length; i++) {
        const key = keys[i];
        const val = init[key];
        if (Array.isArray(val)) {
          for (let j = 0; j < val.length; j++) {
            this.append(key, val[j]);
          }
        } else if (val !== undefined && val !== null) {
          this.set(key, String(val));
        }
      }
    }
  }

  // The map, built from the flattened block the first time something needs more than a read.
  _materialize(): Map<string, string> {
    let map = this._map;
    if (map === null) {
      map = new Map();
      const flattened = this._flat as string;
      const length = flattened.length;
      let at = 0;
      while (at < length) {
        const nameEnd = flattened.indexOf("\0", at);
        if (nameEnd < 0) break;
        const valueEnd = flattened.indexOf("\0", nameEnd + 1);
        if (valueEnd < 0) break;
        const name = flattened.slice(at, nameEnd);
        const value = flattened.slice(nameEnd + 1, valueEnd);
        const existing = map.get(name);
        map.set(name, existing === undefined ? value : `${existing}, ${value}`);
        if (name === "set-cookie") {
          if (this._setCookies === null) this._setCookies = [];
          this._setCookies.push(value);
        }
        at = valueEnd + 1;
      }
      this._map = map;
      this._flat = null;
    }
    return map;
  }

  // One field of the flattened block, without building the map: its values joined with ", ", or
  // null. `name` is already lowercased.
  _flatGet(name: string): string | null {
    const flattened = this._flat as string;
    const length = flattened.length;
    const nameLength = name.length;
    let found: string | null = null;
    let at = 0;
    while (at < length) {
      const nameEnd = flattened.indexOf("\0", at);
      if (nameEnd < 0) break;
      const valueEnd = flattened.indexOf("\0", nameEnd + 1);
      if (valueEnd < 0) break;
      if (nameEnd - at === nameLength && flattened.startsWith(name, at)) {
        const value = flattened.slice(nameEnd + 1, valueEnd);
        found = found === null ? value : `${found}, ${value}`;
      }
      at = valueEnd + 1;
    }
    return found;
  }

  append(name: string, value: string): void {
    const k = String(name).toLowerCase().trim();
    const v = String(value).trim();
    const map = this._materialize();
    if (k === "set-cookie") {
      if (this._setCookies === null) this._setCookies = [];
      this._setCookies.push(v);
    }
    const existing = map.get(k);
    if (existing !== undefined) {
      map.set(k, `${existing}, ${v}`);
    } else {
      map.set(k, v);
    }
  }

  delete(name: string): void {
    const k = String(name).toLowerCase().trim();
    this._materialize().delete(k);
    if (k === "set-cookie") {
      this._setCookies = null;
    }
  }

  get(name: string): string | null {
    const k = String(name).toLowerCase().trim();
    if (this._map === null) return this._flatGet(k);
    const v = this._map.get(k);
    return v !== undefined ? v : null;
  }

  getSetCookie(): string[] {
    this._materialize();
    return this._setCookies === null ? [] : this._setCookies.slice();
  }

  has(name: string): boolean {
    const k = String(name).toLowerCase().trim();
    if (this._map === null) return this._flatGet(k) !== null;
    return this._map.has(k);
  }

  set(name: string, value: string): void {
    const k = String(name).toLowerCase().trim();
    const v = String(value).trim();
    const map = this._materialize();
    if (k === "set-cookie") {
      this._setCookies = [v];
    }
    map.set(k, v);
  }

  forEach(callback: (value: string, key: string, parent: Headers) => void, thisArg?: any): void {
    this._materialize().forEach((value, key) => {
      callback.call(thisArg, value, key, this);
    });
  }

  keys(): IterableIterator<string> {
    return this._materialize().keys();
  }

  values(): IterableIterator<string> {
    return this._materialize().values();
  }

  entries(): IterableIterator<[string, string]> {
    return this._materialize().entries();
  }

  [Symbol.iterator](): IterableIterator<[string, string]> {
    return this._materialize().entries();
  }

  get [Symbol.toStringTag](): string {
    return "Headers";
  }
}

class Blob {
  readonly size: number;
  readonly type: string;
  readonly _data: Uint8Array;

  constructor(blobParts?: any[], options?: { type?: string }) {
    this.type = options?.type ? String(options.type).toLowerCase() : "";
    const parts: Uint8Array[] = [];
    let totalLen = 0;
    if (blobParts && Array.isArray(blobParts)) {
      for (let i = 0; i < blobParts.length; i++) {
        const part = blobParts[i];
        if (typeof part === "string") {
          const enc = typeof TextEncoder !== "undefined" ? new TextEncoder().encode(part) : new Uint8Array(0);
          parts.push(enc);
          totalLen += enc.length;
        } else if (part instanceof Uint8Array) {
          parts.push(part);
          totalLen += part.length;
        } else if (part instanceof ArrayBuffer) {
          const u8 = new Uint8Array(part);
          parts.push(u8);
          totalLen += u8.length;
        } else if (part instanceof Blob) {
          parts.push(part._data);
          totalLen += part._data.length;
        } else if (typeof Buffer !== "undefined" && Buffer.isBuffer(part)) {
          const u8 = new Uint8Array(part);
          parts.push(u8);
          totalLen += u8.length;
        } else if (part !== undefined && part !== null) {
          const s = String(part);
          const enc = typeof TextEncoder !== "undefined" ? new TextEncoder().encode(s) : new Uint8Array(0);
          parts.push(enc);
          totalLen += enc.length;
        }
      }
    }
    const merged = new Uint8Array(totalLen);
    let offset = 0;
    for (let i = 0; i < parts.length; i++) {
      merged.set(parts[i], offset);
      offset += parts[i].length;
    }
    this._data = merged;
    this.size = totalLen;
  }

  async text(): Promise<string> {
    if (typeof TextDecoder !== "undefined") {
      return new TextDecoder().decode(this._data);
    }
    let res = "";
    for (let i = 0; i < this._data.length; i++) {
      res += String.fromCharCode(this._data[i]);
    }
    return res;
  }

  async arrayBuffer(): Promise<ArrayBuffer> {
    return this._data.buffer.slice(this._data.byteOffset, this._data.byteOffset + this._data.byteLength);
  }

  async bytes(): Promise<Uint8Array> {
    return new Uint8Array(this._data);
  }

  slice(start?: number, end?: number, contentType?: string): Blob {
    const s = start === undefined ? 0 : start < 0 ? Math.max(this.size + start, 0) : Math.min(start, this.size);
    const e = end === undefined ? this.size : end < 0 ? Math.max(this.size + end, 0) : Math.min(end, this.size);
    const sub = this._data.subarray(s, Math.max(s, e));
    return new Blob([sub], { type: contentType });
  }

  get [Symbol.toStringTag](): string {
    return "Blob";
  }
}

class File extends Blob {
  readonly name: string;
  readonly lastModified: number;

  constructor(
    fileBits: any[],
    fileName: string,
    options?: { type?: string; lastModified?: number }
  ) {
    super(fileBits, options);
    this.name = String(fileName);
    this.lastModified =
      options?.lastModified !== undefined ? Number(options.lastModified) : Date.now();
  }

  get [Symbol.toStringTag](): string {
    return "File";
  }
}

class DOMException extends Error {
  readonly name: string;

  constructor(message: string = "", name: string = "Error") {
    super(message);
    this.name = name;
  }

  get [Symbol.toStringTag](): string {
    return "DOMException";
  }
}

class FormData {
  private _entries: [string, string | Blob][] = [];

  append(name: string, value: any, _filename?: string): void {
    const k = String(name);
    const val = value instanceof Blob ? value : String(value);
    this._entries.push([k, val]);
  }

  delete(name: string): void {
    const k = String(name);
    this._entries = this._entries.filter(([key]) => key !== k);
  }

  get(name: string): string | Blob | null {
    const k = String(name);
    for (let i = 0; i < this._entries.length; i++) {
      if (this._entries[i][0] === k) return this._entries[i][1];
    }
    return null;
  }

  getAll(name: string): (string | Blob)[] {
    const k = String(name);
    const res: (string | Blob)[] = [];
    for (let i = 0; i < this._entries.length; i++) {
      if (this._entries[i][0] === k) res.push(this._entries[i][1]);
    }
    return res;
  }

  has(name: string): boolean {
    const k = String(name);
    for (let i = 0; i < this._entries.length; i++) {
      if (this._entries[i][0] === k) return true;
    }
    return false;
  }

  set(name: string, value: any, _filename?: string): void {
    const k = String(name);
    const val = value instanceof Blob ? value : String(value);
    let found = false;
    const newEntries: [string, string | Blob][] = [];
    for (let i = 0; i < this._entries.length; i++) {
      if (this._entries[i][0] === k) {
        if (!found) {
          newEntries.push([k, val]);
          found = true;
        }
      } else {
        newEntries.push(this._entries[i]);
      }
    }
    if (!found) {
      newEntries.push([k, val]);
    }
    this._entries = newEntries;
  }

  *keys(): IterableIterator<string> {
    for (let i = 0; i < this._entries.length; i++) {
      yield this._entries[i][0];
    }
  }

  *values(): IterableIterator<string | Blob> {
    for (let i = 0; i < this._entries.length; i++) {
      yield this._entries[i][1];
    }
  }

  *entries(): IterableIterator<[string, string | Blob]> {
    for (let i = 0; i < this._entries.length; i++) {
      yield [this._entries[i][0], this._entries[i][1]];
    }
  }

  [Symbol.iterator](): IterableIterator<[string, string | Blob]> {
    return this.entries();
  }

  get [Symbol.toStringTag](): string {
    return "FormData";
  }
}

let _sharedEncoder: any = null;
function getSharedEncoder(): any {
  if (!_sharedEncoder && typeof TextEncoder !== "undefined") {
    _sharedEncoder = new TextEncoder();
  }
  return _sharedEncoder;
}

let _sharedDecoder: any = null;
function getSharedDecoder(): any {
  if (!_sharedDecoder && typeof TextDecoder !== "undefined") {
    _sharedDecoder = new TextDecoder();
  }
  return _sharedDecoder;
}

const EMPTY_BODY_DATA: { bytes: Uint8Array | null; text: string | null } = { bytes: null, text: null };

function extractBodyData(body: any): { bytes: Uint8Array | null; text: string | null } {
  if (body === undefined || body === null) {
    return EMPTY_BODY_DATA;
  }
  if (typeof body === "string") {
    return { bytes: null, text: body };
  }
  if (body instanceof Uint8Array) {
    return { bytes: body, text: null };
  }
  if (body instanceof ArrayBuffer) {
    return { bytes: new Uint8Array(body), text: null };
  }
  if (body instanceof Blob) {
    return { bytes: body._data, text: null };
  }
  if (typeof Buffer !== "undefined" && Buffer.isBuffer(body)) {
    return { bytes: new Uint8Array(body), text: null };
  }
  const s = String(body);
  return { bytes: null, text: s };
}

// The `Content-Type` the Fetch standard's "extract a body" gives a body that is a string.
const TEXT_PLAIN_UTF8 = "text/plain;charset=UTF-8";

// The body's own content type, which a Request or Response adds to its headers when they do not
// name one (Fetch, "extract a body"): a string is `text/plain;charset=UTF-8`, a Blob its `type`.
function bodyContentType(body: any): string | null {
  if (typeof body === "string") return TEXT_PLAIN_UTF8;
  if (body instanceof Blob) return body.type !== "" ? body.type : null;
  return null;
}

// Fields are declared here rather than in the class body: a class-body declaration without an
// initializer still defines the property (as undefined) before the constructor runs, which on the
// compiled backend is one more property store per field per Response. The constructor assigns
// every field exactly once, in one order on every path, so every Response shares one shape.
interface Response {
  readonly status: number;
  readonly statusText: string;
  readonly redirected: boolean;
  readonly type: string;
  readonly url: string;
  bodyUsed: boolean;
  _bodyStream: any;
  _rawBytes: Uint8Array | null;
  _rawText: string | null;
  // Null until `headers` is first read when no headers were given: most responses a server
  // builds are never asked, and `serve` sends only `_contentType` for them.
  _headers: Headers | null;
  _contentType: string | null;
}

class Response {
  constructor(body?: any, init?: any) {
    let status = 200;
    let statusText = "";
    let url = "";
    let redirected = false;
    let headers: Headers | null = null;
    let ownHeaders = false;
    if (init !== undefined && init !== null) {
      if (init.status !== undefined) status = Number(init.status);
      if (init.statusText !== undefined) statusText = String(init.statusText);
      if (init.url) url = String(init.url);
      redirected = Boolean(init.redirected);
      const given = init.headers;
      if (given instanceof Headers) {
        headers = given;
      } else if (given !== undefined && given !== null) {
        // Copied now: the init is the caller's to change afterwards.
        headers = new Headers(given);
        ownHeaders = true;
      }
    }
    let rawBytes: Uint8Array | null = null;
    let rawText: string | null = null;
    let contentType: string | null = null;
    if (typeof body === "string") {
      rawText = body;
      contentType = TEXT_PLAIN_UTF8;
    } else if (body !== undefined && body !== null) {
      const extracted = extractBodyData(body);
      rawBytes = extracted.bytes;
      rawText = extracted.text;
      contentType = bodyContentType(body);
    }
    if (headers !== null && contentType !== null && !headers.has("content-type")) {
      // The body's type is added to this object's own headers, never to the caller's Headers.
      if (!ownHeaders) headers = new Headers(headers);
      headers.set("content-type", contentType);
    }
    (this as any).status = status;
    (this as any).statusText = statusText;
    (this as any).redirected = redirected;
    (this as any).type = "default";
    (this as any).url = url;
    this.bodyUsed = false;
    this._bodyStream = undefined;
    this._rawBytes = rawBytes;
    this._rawText = rawText;
    this._headers = headers;
    this._contentType = contentType;
  }

  get ok(): boolean {
    return this.status >= 200 && this.status <= 299;
  }

  get headers(): Headers {
    let headers = this._headers;
    if (headers === null) {
      headers = new Headers();
      if (this._contentType !== null) headers.set("content-type", this._contentType);
      this._headers = headers;
    }
    return headers;
  }

  // A stream over the bytes, made once: node's `body` is a `ReadableStream` of `Uint8Array`
  // chunks and reading it is what marks the body used. It answered `null` until 2026-09-19.
  get body(): any {
    if (this._bodyStream !== undefined) return this._bodyStream;
    if (this._rawBytes === null && this._rawText === null) {
      this._bodyStream = null;
      return null;
    }
    const RS = (globalThis as any).ReadableStream;
    if (typeof RS !== "function") return null;
    const self = this;
    this._bodyStream = new RS({
      pull(controller: any) {
        self.bodyUsed = true;
        const bytes =
          self._rawBytes !== null
            ? self._rawBytes
            : (getSharedEncoder()?.encode(self._rawText || "") || new Uint8Array(0));
        if (bytes.length > 0) controller.enqueue(bytes);
        controller.close();
      },
    });
    return this._bodyStream;
  }

  async text(): Promise<string> {
    if (this.bodyUsed) throw new TypeError("Body has already been consumed");
    this.bodyUsed = true;
    if (this._rawText !== null) return this._rawText;
    if (this._rawBytes !== null) {
      const dec = getSharedDecoder();
      if (dec) return dec.decode(this._rawBytes);
      let res = "";
      for (let i = 0; i < this._rawBytes.length; i++) {
        res += String.fromCharCode(this._rawBytes[i]);
      }
      return res;
    }
    return "";
  }

  async json(): Promise<any> {
    const txt = await this.text();
    return JSON.parse(txt);
  }

  async arrayBuffer(): Promise<ArrayBuffer> {
    if (this.bodyUsed) throw new TypeError("Body has already been consumed");
    this.bodyUsed = true;
    if (this._rawBytes === null && this._rawText !== null) {
      this._rawBytes = getSharedEncoder()?.encode(this._rawText) || null;
    }
    if (this._rawBytes !== null) {
      return this._rawBytes.buffer.slice(
        this._rawBytes.byteOffset,
        this._rawBytes.byteOffset + this._rawBytes.byteLength
      );
    }
    return new ArrayBuffer(0);
  }

  async bytes(): Promise<Uint8Array> {
    if (this.bodyUsed) throw new TypeError("Body has already been consumed");
    this.bodyUsed = true;
    if (this._rawBytes !== null) {
      return new Uint8Array(this._rawBytes);
    }
    if (this._rawText !== null) {
      const enc = getSharedEncoder();
      return enc ? enc.encode(this._rawText) : new Uint8Array(0);
    }
    return new Uint8Array(0);
  }

  async blob(): Promise<Blob> {
    const u8 = await this.bytes();
    const type = this.headers.get("content-type") || "";
    return new Blob([u8], { type });
  }

  clone(): Response {
    if (this.bodyUsed) throw new TypeError("Body has already been consumed");
    const init = {
      status: this.status,
      statusText: this.statusText,
      headers: new Headers(this.headers),
      url: this.url,
      redirected: this.redirected,
    };
    return new Response(this._rawBytes || this._rawText, init);
  }

  static error(): Response {
    const res = new Response(null, { status: 0, statusText: "" });
    (res as any).type = "error";
    return res;
  }

  static json(data: any, init?: any): Response {
    const body = JSON.stringify(data);
    const headers = new Headers(init?.headers);
    if (!headers.has("content-type")) {
      headers.set("content-type", "application/json");
    }
    return new Response(body, {
      ...init,
      headers,
    });
  }

  static redirect(url: any, status: number = 302): Response {
    if (![301, 302, 303, 307, 308].includes(status)) {
      throw new RangeError("Invalid status code for redirect");
    }
    const headers = new Headers();
    headers.set("location", String(url));
    return new Response(null, { status, headers });
  }

  get [Symbol.toStringTag](): string {
    return "Response";
  }
}

// A request `serve()` received, handed to the Request constructor through these rather than
// through an init object so that nothing but the Request itself is allocated per request. Set
// immediately before `new Request(SERVED_REQUEST)` and read by nothing else.
const SERVED_REQUEST: any = {};
let servedMethod = "GET";
let servedTarget = "/";
let servedHeaders = "";
let servedBody: Uint8Array | null = null;
let servedHost = "";

// Headers over one `name NUL value NUL` block (the engine's flattening of a request head, names
// lowercased and values trimmed), kept flat until something needs more than a read.
function headersFromFlattened(flattened: string): Headers {
  const headers = new Headers();
  headers._map = null;
  headers._flat = flattened;
  return headers;
}

// The `Host` field of a flattened request head, or null.
function flattenedHost(flattened: string): string | null {
  const length = flattened.length;
  let at = 0;
  while (at < length) {
    const nameEnd = flattened.indexOf("\0", at);
    if (nameEnd < 0) return null;
    const valueEnd = flattened.indexOf("\0", nameEnd + 1);
    if (valueEnd < 0) return null;
    if (nameEnd - at === 4 && flattened.startsWith("host", at)) {
      return flattened.slice(nameEnd + 1, valueEnd);
    }
    at = valueEnd + 1;
  }
  return null;
}

// Declared here rather than in the class body, for the reason given above `Response`.
interface Request {
  readonly method: string;
  readonly signal: any;
  readonly redirect: string;
  bodyUsed: boolean;
  _bodyStream: any;
  _rawBytes: Uint8Array | null;
  _rawText: string | null;
  // `url` and `headers` are built on first read. A request `serve()` received keeps the request
  // target and the flattened head (`_target`, `_flatHeaders`, with `_host` the authority to use
  // when the head names none); a constructed one keeps its URL and the headers init.
  _url: string | null;
  _headers: Headers | null;
  _contentType: string | null;
  _target: string;
  _flatHeaders: string | null;
  _host: string;
}

class Request {
  constructor(input: any, init?: any) {
    const self = this as any;
    if (input === SERVED_REQUEST) {
      self.method = servedMethod;
      self.signal = undefined;
      self.redirect = "follow";
      this.bodyUsed = false;
      this._bodyStream = undefined;
      this._rawBytes = servedBody;
      this._rawText = null;
      this._url = null;
      this._headers = null;
      this._contentType = null;
      this._target = servedTarget;
      this._flatHeaders = servedHeaders;
      this._host = servedHost;
      return;
    }
    let url: string;
    let method: string;
    let signal: any;
    let redirect: string;
    let headersInit: any;
    let bodySource: any;
    let contentType: string | null = null;
    if (input instanceof Request) {
      url = input.url;
      method = (init?.method || input.method).toUpperCase();
      headersInit = init?.headers instanceof Headers ? init.headers : init?.headers || input.headers;
      signal = init?.signal || input.signal;
      redirect = init?.redirect || input.redirect || "follow";
      if (init?.body !== undefined) {
        bodySource = init.body;
        contentType = bodyContentType(bodySource);
      } else {
        bodySource = (input as any)._rawBytes || (input as any)._rawText;
      }
    } else {
      url = String(input);
      method = (init?.method || "GET").toUpperCase();
      headersInit = init?.headers;
      signal = init?.signal;
      redirect = init?.redirect || "follow";
      bodySource = init?.body;
      contentType = bodyContentType(bodySource);
    }
    let rawBytes: Uint8Array | null = null;
    let rawText: string | null = null;
    if (typeof bodySource === "string") {
      rawText = bodySource;
    } else if (bodySource !== undefined && bodySource !== null) {
      const extracted = extractBodyData(bodySource);
      rawBytes = extracted.bytes;
      rawText = extracted.text;
    }
    // A Headers passed as `init.headers` is this request's own (as it always was here); anything
    // else is copied now, since the init is the caller's to change afterwards.
    let headers: Headers =
      init?.headers instanceof Headers && headersInit === init.headers ? init.headers : new Headers(headersInit);
    if (contentType !== null && !headers.has("content-type")) {
      // The body's type is added to this object's own headers, never to the caller's Headers.
      if (headers === headersInit) headers = new Headers(headers);
      headers.set("content-type", contentType);
    }
    self.method = method;
    self.signal = signal;
    self.redirect = redirect;
    this.bodyUsed = false;
    this._bodyStream = undefined;
    this._rawBytes = rawBytes;
    this._rawText = rawText;
    this._url = url;
    this._headers = headers;
    this._contentType = contentType;
    this._target = "";
    this._flatHeaders = null;
    this._host = "";
  }

  get url(): string {
    let url = this._url;
    if (url === null) {
      const target = this._target;
      if (target.charCodeAt(0) !== 47 /* "/" */ && /^[a-zA-Z][a-zA-Z0-9+.-]*:/.test(target)) {
        // An absolute-form target (a proxy request) is already the URL.
        url = target;
      } else {
        const flattened = this._flatHeaders;
        const host = flattened !== null ? flattenedHost(flattened) : null;
        url = `http://${host !== null && host !== "" ? host : this._host}${target}`;
      }
      this._url = url;
    }
    return url;
  }

  get headers(): Headers {
    let headers = this._headers;
    if (headers === null) {
      const flattened = this._flatHeaders;
      if (flattened !== null) {
        // The URL may still need the Host field, which is about to stop being kept.
        if (this._url === null) this._url = this.url;
        headers = headersFromFlattened(flattened);
        this._flatHeaders = null;
      } else {
        headers = new Headers();
      }
      this._headers = headers;
    }
    return headers;
  }

  // A stream over the bytes, made once: node's `body` is a `ReadableStream` of `Uint8Array`
  // chunks and reading it is what marks the body used. It answered `null` until 2026-09-19.
  get body(): any {
    if (this._bodyStream !== undefined) return this._bodyStream;
    if (this._rawBytes === null && this._rawText === null) {
      this._bodyStream = null;
      return null;
    }
    const RS = (globalThis as any).ReadableStream;
    if (typeof RS !== "function") return null;
    const self = this;
    this._bodyStream = new RS({
      pull(controller: any) {
        self.bodyUsed = true;
        const bytes =
          self._rawBytes !== null
            ? self._rawBytes
            : (getSharedEncoder()?.encode(self._rawText || "") || new Uint8Array(0));
        if (bytes.length > 0) controller.enqueue(bytes);
        controller.close();
      },
    });
    return this._bodyStream;
  }

  async text(): Promise<string> {
    if (this.bodyUsed) throw new TypeError("Body has already been consumed");
    this.bodyUsed = true;
    if (this._rawText !== null) return this._rawText;
    if (this._rawBytes !== null) {
      const dec = getSharedDecoder();
      if (dec) return dec.decode(this._rawBytes);
      let res = "";
      for (let i = 0; i < this._rawBytes.length; i++) {
        res += String.fromCharCode(this._rawBytes[i]);
      }
      return res;
    }
    return "";
  }

  async json(): Promise<any> {
    const txt = await this.text();
    return JSON.parse(txt);
  }

  async arrayBuffer(): Promise<ArrayBuffer> {
    if (this.bodyUsed) throw new TypeError("Body has already been consumed");
    this.bodyUsed = true;
    if (this._rawBytes === null && this._rawText !== null) {
      this._rawBytes = getSharedEncoder()?.encode(this._rawText) || null;
    }
    if (this._rawBytes !== null) {
      return this._rawBytes.buffer.slice(
        this._rawBytes.byteOffset,
        this._rawBytes.byteOffset + this._rawBytes.byteLength
      );
    }
    return new ArrayBuffer(0);
  }

  async bytes(): Promise<Uint8Array> {
    if (this.bodyUsed) throw new TypeError("Body has already been consumed");
    this.bodyUsed = true;
    if (this._rawBytes !== null) {
      return new Uint8Array(this._rawBytes);
    }
    if (this._rawText !== null) {
      const enc = getSharedEncoder();
      return enc ? enc.encode(this._rawText) : new Uint8Array(0);
    }
    return new Uint8Array(0);
  }

  async blob(): Promise<Blob> {
    const u8 = await this.bytes();
    const type = this.headers.get("content-type") || "";
    return new Blob([u8], { type });
  }

  clone(): Request {
    if (this.bodyUsed) throw new TypeError("Body has already been consumed");
    return new Request(this);
  }

  get [Symbol.toStringTag](): string {
    return "Request";
  }
}

function fetch(input: any, init?: any): Promise<Response> {
  return new Promise((resolve, reject) => {
    let req: Request;
    try {
      req = new Request(input, init);
    } catch (e) {
      return reject(e);
    }

    if (req.signal && req.signal.aborted) {
      return reject(req.signal.reason || new Error("The operation was aborted"));
    }

    let parsed: { protocol: string; hostname: string; port: number; pathname: string; search: string };
    try {
      let urlStr = String(req.url);
      let protocol = "http:";
      let rest = urlStr;
      if (urlStr.startsWith("https://")) {
        protocol = "https:";
        rest = urlStr.slice(8);
      } else if (urlStr.startsWith("http://")) {
        protocol = "http:";
        rest = urlStr.slice(7);
      } else {
        const colonIdx = urlStr.indexOf("://");
        if (colonIdx !== -1) {
          protocol = urlStr.slice(0, colonIdx + 1);
          rest = urlStr.slice(colonIdx + 3);
        }
      }
      let slashIdx = rest.indexOf("/");
      let questionIdx = rest.indexOf("?");
      let hostEnd = rest.length;
      if (slashIdx !== -1) hostEnd = Math.min(hostEnd, slashIdx);
      if (questionIdx !== -1) hostEnd = Math.min(hostEnd, questionIdx);
      const hostPart = rest.slice(0, hostEnd);
      let pathAndQuery = rest.slice(hostEnd);
      if (!pathAndQuery.startsWith("/")) {
        pathAndQuery = "/" + pathAndQuery;
      }
      let qIdx = pathAndQuery.indexOf("?");
      let pathname = qIdx !== -1 ? pathAndQuery.slice(0, qIdx) : pathAndQuery;
      let search = qIdx !== -1 ? pathAndQuery.slice(qIdx) : "";
      let hostname = hostPart;
      let port = protocol === "https:" ? 443 : 80;
      if (hostPart.startsWith("[")) {
        const closeBracket = hostPart.indexOf("]");
        if (closeBracket !== -1) {
          hostname = hostPart.slice(1, closeBracket);
          if (hostPart[closeBracket + 1] === ":") {
            const p = parseInt(hostPart.slice(closeBracket + 2), 10);
            if (!isNaN(p)) port = p;
          }
        }
      } else {
        const colon = hostPart.lastIndexOf(":");
        if (colon !== -1) {
          hostname = hostPart.slice(0, colon);
          const p = parseInt(hostPart.slice(colon + 1), 10);
          if (!isNaN(p)) port = p;
        }
      }
      parsed = { protocol, hostname, port, pathname, search };
    } catch (err) {
      return reject(err);
    }

    if (parsed.protocol !== "http:" && parsed.protocol !== "https:") {
      return reject(new TypeError(`Unsupported protocol: ${parsed.protocol}`));
    }

    const httpModule =
      typeof process !== "undefined"
        ? (process as any).__kilnNativeModules?.http || null
        : null;

    let httpReqFn = httpModule?.request;
    if (!httpReqFn) {
      try {
        const h = typeof require !== "undefined" ? require("node:http") : null;
        if (h && typeof h.request === "function") httpReqFn = h.request;
      } catch (_) {}
    }

    if (!httpReqFn) {
      return reject(
        new Error("fetch requires an HTTP host capability that this realm does not provide")
      );
    }

    const headersObj: Record<string, string> = {};
    req.headers.forEach((v, k) => {
      headersObj[k] = v;
    });

    const options = {
      host: parsed.hostname,
      port: parsed.port,
      method: req.method,
      path: (parsed.pathname || "/") + (parsed.search || ""),
      headers: headersObj,
    };

    let clientReq: any;
    try {
      clientReq = httpReqFn(options, (res: any) => {
        const chunks: any[] = [];
        res.on("data", (chunk: any) => {
          chunks.push(chunk);
        });
        res.on("end", () => {
          let bodyBytes: Uint8Array;
          if (typeof Buffer !== "undefined" && typeof Buffer.concat === "function") {
            const buf = Buffer.concat(chunks);
            bodyBytes = new Uint8Array(buf);
          } else {
            let total = 0;
            for (let i = 0; i < chunks.length; i++) total += chunks[i].length || 0;
            bodyBytes = new Uint8Array(total);
            let off = 0;
            for (let i = 0; i < chunks.length; i++) {
              const c = chunks[i];
              if (c instanceof Uint8Array) {
                bodyBytes.set(c, off);
                off += c.length;
              } else if (typeof c === "string") {
                const enc = new TextEncoder().encode(c);
                bodyBytes.set(enc, off);
                off += enc.length;
              }
            }
          }

          const resHeaders = new Headers(res.headers);
          const response = new Response(bodyBytes, {
            status: res.statusCode || 200,
            statusText: res.statusMessage || "",
            headers: resHeaders,
            url: req.url,
          });
          resolve(response);
        });
        res.on("error", (err: any) => {
          reject(err);
        });
      });
    } catch (err) {
      return reject(err);
    }

    if (clientReq && typeof clientReq.on === "function") {
      clientReq.on("error", (err: any) => {
        reject(err);
      });
    }

    if (req.signal) {
      const onAbort = () => {
        if (clientReq && typeof clientReq.destroy === "function") {
          clientReq.destroy();
        }
        reject(req.signal.reason || new Error("The operation was aborted"));
      };
      if (typeof req.signal.addEventListener === "function") {
        req.signal.addEventListener("abort", onAbort);
      }
    }

    (async () => {
      try {
        const rawText = (req as any)._rawText;
        if (rawText !== null && rawText !== undefined) {
          if (clientReq && typeof clientReq.write === "function") {
            clientReq.write(rawText);
          }
        } else {
          const bodyBytes = await (req as any).bytes();
          if (bodyBytes && bodyBytes.length > 0 && clientReq && typeof clientReq.write === "function") {
            const chunk = typeof Buffer !== "undefined"
              ? Buffer.from(bodyBytes.buffer, bodyBytes.byteOffset, bodyBytes.byteLength)
              : bodyBytes;
            clientReq.write(chunk);
          }
        }
        if (clientReq && typeof clientReq.end === "function") {
          clientReq.end();
        }
      } catch (err) {
        if (clientReq && typeof clientReq.destroy === "function") {
          clientReq.destroy();
        }
        reject(err);
      }
    })();
  });
}

// Global initialization
if (typeof (globalThis as any).Headers === "undefined" || !((globalThis as any).Headers.prototype?.append)) {
  (globalThis as any).Headers = Headers;
}
if (typeof (globalThis as any).Blob === "undefined" || !((globalThis as any).Blob.prototype?.slice)) {
  (globalThis as any).Blob = Blob;
}
if (typeof (globalThis as any).FormData === "undefined" || !((globalThis as any).FormData.prototype?.append)) {
  (globalThis as any).FormData = FormData;
}
if (typeof (globalThis as any).Response === "undefined" || !((globalThis as any).Response.prototype?.json)) {
  (globalThis as any).Response = Response;
}
if (typeof (globalThis as any).Request === "undefined" || !((globalThis as any).Request.prototype?.clone)) {
  (globalThis as any).Request = Request;
}
if (typeof (globalThis as any).File === "undefined") {
  (globalThis as any).File = File;
}
if (typeof (globalThis as any).DOMException === "undefined") {
  (globalThis as any).DOMException = DOMException;
}
if (typeof (globalThis as any).fetch === "undefined") {
  (globalThis as any).fetch = fetch;
}

// The header block `fetchRespond` takes: `name NUL value NUL` per field, the flattening the
// engine parses straight into Hyper's map. A response whose headers were never read or given
// is only its body's content type, and costs no Headers at all.
function flattenResponseHeaders(response: any): string {
  const headers: Headers | null = response._headers;
  if (headers === null) {
    const contentType = response._contentType;
    return contentType === null ? "" : `content-type\0${contentType}\0`;
  }
  // A request's headers handed straight back are already the block.
  if (headers._map === null) return headers._flat as string;
  let flattened = "";
  // `forEach` rather than `for...of`: no iterator and no entry array per field.
  headers._map.forEach((value: string, name: string) => {
    if (name === "set-cookie") {
      // Each cookie is its own field; the map's joined value is only what `get` answers.
      const cookies = headers._setCookies;
      if (cookies !== null) {
        for (let i = 0; i < cookies.length; i++) flattened += `set-cookie\0${cookies[i]}\0`;
      }
    } else {
      flattened += `${name}\0${value}\0`;
    }
  });
  return flattened;
}

// `serve({ fetch, port, hostname }, callback)`, the one implementation behind `Bun.serve`,
// `Kiln.serve` and `node:http`'s `serve`.
//
// It is not built on `createServer`. The host's fetch server hands each request over whole --
// method, target and the flattened header block as strings, the body as bytes -- and the
// handler's Response goes back through one `fetchRespond` call carrying status, header block and
// body. No IncomingMessage or ServerResponse exists, and a Request's `url` and `headers` are
// only built if the handler reads them.
function serve(
  options:
    | {
        fetch: (request: Request) => Response | Promise<Response>;
        port?: number;
        hostname?: string;
      }
    | ((request: Request) => Response | Promise<Response>),
  callback?: () => void
): any {
  const fetchHandler = typeof options === "function" ? options : options?.fetch;
  if (typeof fetchHandler !== "function") {
    throw new TypeError("serve requires a fetch handler function");
  }
  const port = typeof options === "object" && options.port !== undefined ? Number(options.port) : 3000;
  const hostname = typeof options === "object" && options.hostname ? String(options.hostname) : undefined;

  const httpMod = typeof process !== "undefined" ? (process as any).__kilnNativeModules?.http : null;
  if (!httpMod || typeof httpMod.createFetchServer !== "function" || typeof httpMod.fetchRespond !== "function") {
    throw new Error("serve requires an HTTP host capability that this realm does not provide");
  }
  const respond = httpMod.fetchRespond;
  // The authority a request's URL names when it carries no Host field (HTTP/1.0).
  const fallbackHost = hostname ? `${hostname}:${port}` : `localhost:${port}`;

  function fail(resource: number, id: number, err: any) {
    respond(resource, id, 500, "", err?.message || "Internal Server Error");
  }

  function send(resource: number, id: number, response: any) {
    if (!(response instanceof Response)) {
      fail(resource, id, new TypeError("serve: the fetch handler must return a Response"));
      return;
    }
    try {
      const text = response._rawText;
      const body = text !== null ? text : response._rawBytes !== null ? response._rawBytes : "";
      respond(resource, id, response.status, flattenResponseHeaders(response), body);
    } catch (err: any) {
      fail(resource, id, err);
    }
  }

  const server = httpMod.createFetchServer(
    (method: string, target: string, headers: string, body: any, resource: number, id: number) => {
      servedMethod = method;
      servedTarget = target;
      servedHeaders = headers;
      servedBody = body === undefined ? null : body;
      servedHost = fallbackHost;
      let result: any;
      try {
        result = fetchHandler(new Request(SERVED_REQUEST));
      } catch (err: any) {
        fail(resource, id, err);
        return;
      }
      if (result instanceof Response) {
        send(resource, id, result);
      } else if (result !== null && typeof result === "object" && typeof result.then === "function") {
        result.then(
          (response: any) => send(resource, id, response),
          (err: any) => fail(resource, id, err)
        );
      } else {
        send(resource, id, result);
      }
    }
  );

  if (hostname) {
    server.listen(port, hostname, callback);
  } else {
    server.listen(port, callback);
  }
  return server;
}

if (typeof (globalThis as any).Kiln === "undefined") {
  (globalThis as any).Kiln = { serve };
} else {
  (globalThis as any).Kiln.serve = serve;
}
if (typeof (globalThis as any).Bun === "undefined") {
  (globalThis as any).Bun = { serve };
} else if (!(globalThis as any).Bun.serve) {
  (globalThis as any).Bun.serve = serve;
}
