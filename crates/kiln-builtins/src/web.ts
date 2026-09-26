// kiln-builtins: WHATWG Web Standards (fetch, Headers, Request, Response, FormData, Blob)
// Phase 5.2, ADR 0005

class Headers {
  private _map: Map<string, string> = new Map();
  private _setCookies: string[] = [];

  constructor(init?: any) {
    if (!init) return;
    if (init instanceof Headers) {
      this._map = new Map(init._map);
      this._setCookies = init._setCookies.slice();
    } else if (Array.isArray(init)) {
      for (let i = 0; i < init.length; i++) {
        const entry = init[i];
        if (Array.isArray(entry) && entry.length >= 2) {
          this.append(entry[0], entry[1]);
        }
      }
    } else if (typeof init === "object") {
      for (const key in init) {
        if (!Object.prototype.hasOwnProperty.call(init, key)) continue;
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

  append(name: string, value: string): void {
    const k = String(name).toLowerCase().trim();
    const v = String(value).trim();
    if (k === "set-cookie") {
      this._setCookies.push(v);
    }
    const existing = this._map.get(k);
    if (existing !== undefined) {
      this._map.set(k, `${existing}, ${v}`);
    } else {
      this._map.set(k, v);
    }
  }

  delete(name: string): void {
    const k = String(name).toLowerCase().trim();
    this._map.delete(k);
    if (k === "set-cookie") {
      this._setCookies = [];
    }
  }

  get(name: string): string | null {
    const k = String(name).toLowerCase().trim();
    const v = this._map.get(k);
    return v !== undefined ? v : null;
  }

  getSetCookie(): string[] {
    return this._setCookies.slice();
  }

  has(name: string): boolean {
    const k = String(name).toLowerCase().trim();
    return this._map.has(k);
  }

  set(name: string, value: string): void {
    const k = String(name).toLowerCase().trim();
    const v = String(value).trim();
    if (k === "set-cookie") {
      this._setCookies = [v];
    }
    this._map.set(k, v);
  }

  forEach(callback: (value: string, key: string, parent: Headers) => void, thisArg?: any): void {
    this._map.forEach((value, key) => {
      callback.call(thisArg, value, key, this);
    });
  }

  keys(): IterableIterator<string> {
    return this._map.keys();
  }

  values(): IterableIterator<string> {
    return this._map.values();
  }

  entries(): IterableIterator<[string, string]> {
    return this._map.entries();
  }

  [Symbol.iterator](): IterableIterator<[string, string]> {
    return this._map.entries();
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

class Response {
  readonly status: number;
  readonly statusText: string;
  readonly headers: Headers;
  readonly ok: boolean;
  readonly redirected: boolean;
  readonly type: string;
  readonly url: string;
  bodyUsed: boolean = false;
  _bodyStream: any = undefined;
  _rawBytes: Uint8Array | null;
  _rawText: string | null;

  constructor(body?: any, init?: any) {
    this.status = init?.status !== undefined ? Number(init.status) : 200;
    this.statusText = init?.statusText !== undefined ? String(init.statusText) : "";
    this.ok = this.status >= 200 && this.status <= 299;
    this.headers = init?.headers instanceof Headers ? init.headers : new Headers(init?.headers);
    this.url = init?.url ? String(init.url) : "";
    this.redirected = Boolean(init?.redirected);
    this.type = "default";
    if (typeof body === "string") {
      this._rawBytes = null;
      this._rawText = body;
    } else if (body === undefined || body === null) {
      this._rawBytes = null;
      this._rawText = null;
    } else {
      const extracted = extractBodyData(body);
      this._rawBytes = extracted.bytes;
      this._rawText = extracted.text;
    }
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
    (res as any).ok = false;
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

class Request {
  readonly method: string;
  readonly url: string;
  readonly headers: Headers;
  readonly signal: any;
  readonly redirect: string;
  bodyUsed: boolean = false;
  _bodyStream: any = undefined;
  _rawBytes: Uint8Array | null;
  _rawText: string | null;

  constructor(input: any, init?: any) {
    if (input instanceof Request) {
      this.url = input.url;
      this.method = (init?.method || input.method).toUpperCase();
      this.headers = init?.headers instanceof Headers ? init.headers : new Headers(init?.headers || input.headers);
      this.signal = init?.signal || input.signal;
      this.redirect = init?.redirect || input.redirect || "follow";
      const bodySource =
        init?.body !== undefined
          ? init.body
          : (input as any)._rawBytes || (input as any)._rawText;
      if (typeof bodySource === "string") {
        this._rawBytes = null;
        this._rawText = bodySource;
      } else if (bodySource === undefined || bodySource === null) {
        this._rawBytes = null;
        this._rawText = null;
      } else {
        const extracted = extractBodyData(bodySource);
        this._rawBytes = extracted.bytes;
        this._rawText = extracted.text;
      }
    } else {
      this.url = String(input);
      this.method = (init?.method || "GET").toUpperCase();
      this.headers = init?.headers instanceof Headers ? init.headers : new Headers(init?.headers);
      this.signal = init?.signal;
      this.redirect = init?.redirect || "follow";
      const body = init?.body;
      if (typeof body === "string") {
        this._rawBytes = null;
        this._rawText = body;
      } else if (body === undefined || body === null) {
        this._rawBytes = null;
        this._rawText = null;
      } else {
        const extracted = extractBodyData(body);
        this._rawBytes = extracted.bytes;
        this._rawText = extracted.text;
      }
    }
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
  if (!httpMod || typeof httpMod.createServer !== "function") {
    throw new Error("serve requires an HTTP host capability that this realm does not provide");
  }

  function sendResponse(webRes: any, res: any) {
    res.statusCode = webRes.status;
    for (const [k, v] of webRes.headers.entries()) {
      res.setHeader(k, v);
    }
    const rawText = webRes._rawText;
    if (rawText !== null && rawText !== undefined) {
      res.end(rawText);
    } else {
      const rawBytes = webRes._rawBytes;
      if (rawBytes !== null && rawBytes !== undefined) {
        res.end(rawBytes);
      } else {
        webRes.text().then(
          (body: string) => res.end(body),
          (err: any) => {
            res.statusCode = 500;
            res.end(err?.message || "Internal Server Error");
          }
        );
      }
    }
  }

  const server = httpMod.createServer((req: any, res: any) => {
    try {
      const hostHeader = req.headers?.host || (hostname ? `${hostname}:${port}` : `localhost:${port}`);
      const url = `http://${hostHeader}${req.url}`;
      const webReq = new Request(url, {
        method: req.method,
        headers: req.headers,
      });
      const webRes = fetchHandler(webReq);
      if (webRes instanceof Promise) {
        webRes.then(
          (r: any) => sendResponse(r, res),
          (err: any) => {
            res.statusCode = 500;
            res.end(err?.message || "Internal Server Error");
          }
        );
      } else {
        sendResponse(webRes, res);
      }
    } catch (err: any) {
      res.statusCode = 500;
      res.end(err?.message || "Internal Server Error");
    }
  });

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
