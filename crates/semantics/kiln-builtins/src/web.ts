// kiln-builtins: WHATWG Web Standards (fetch, Headers, Request, Response, FormData, Blob)
// Phase 5.2, ADR 0005

// ---- Header lists ------------------------------------------------------------------------------
//
// A header list (Fetch's "header list") is one immutable string in HTTP wire form: `\nname:value`
// per field, in list order, with nothing before the first field or after the last; the empty
// string is the empty list. It is the one representation end to end: a `serve()` request's
// headers arrive from the engine in it, a `Headers` holds nothing else, a `Response` keeps it, and
// `fetchRespond` hands it back to the engine, which parses it straight into Hyper's map.
//
// What keeps it unambiguous is what the Fetch standard already requires of every field: a name is
// a token (never `:` or `\n`) and is stored lowercased, and a value is normalized (no leading or
// trailing HTTP whitespace) and holds no NUL, CR or LF. So a field starts exactly where
// `\n` + name + `:` occurs -- a value cannot contain `\n` -- and runs to the next `\n`. Every
// lookup is one native search for that needle. Anything that enters a list from a program goes
// through `headerNeedle` and `headerValue`, which throw the standard's TypeError otherwise.
//
// The operations are small pure functions of the string, so that copying a list (the standard's
// "copy the init", a Response snapshotting a Headers) is taking a reference.

// A header name: an RFC 9110 token.
const HEADER_NAME_TOKEN = /^[!#$%&'*+\-.^_`|~0-9A-Za-z]+$/;
// A value that needs normalizing or refusing: HTTP whitespace at either end, NUL/CR/LF anywhere,
// or a code unit a ByteString cannot hold. Nearly every value matches none of it.
const HEADER_VALUE_UNUSUAL = /^[\t\n\r ]|[\t\n\r ]$|[\0\n\r]|[^\0-\xff]/;
const HEADER_VALUE_EDGES = /^[\t\n\r ]+|[\t\n\r ]+$/g;
const HEADER_VALUE_INVALID = /[\0\n\r]|[^\0-\xff]/;

// A name as the program spelled it, to its needle. Names are few and repeat on every request,
// so validating and lowercasing each spelling once is worth a bounded table.
const headerNeedles: Map<string, string> = new Map();

// `\n` + the lowercased name + `:`, the needle a field is found by. Throws the TypeError the
// standard gives a name that is not a token.
function headerNeedle(name: any, caller: string): string {
  const text = typeof name === "string" ? name : `${name}`;
  let needle = headerNeedles.get(text);
  if (needle !== undefined) return needle;
  if (!HEADER_NAME_TOKEN.test(text)) {
    throw new TypeError(`${caller}: "${text}" is an invalid header name.`);
  }
  needle = `\n${text.toLowerCase()}:`;
  if (headerNeedles.size < 512) headerNeedles.set(text, needle);
  return needle;
}

// A value normalized (HTTP whitespace stripped from both ends), or the standard's TypeError for
// one holding NUL, CR or LF, or a code unit above 0xFF (it is a ByteString).
function headerValue(value: any, caller: string): string {
  const text = typeof value === "string" ? value : `${value}`;
  if (!HEADER_VALUE_UNUSUAL.test(text)) return text;
  const trimmed = text.replace(HEADER_VALUE_EDGES, "");
  if (HEADER_VALUE_INVALID.test(trimmed)) {
    throw new TypeError(`${caller}: "${trimmed}" is an invalid header value.`);
  }
  return trimmed;
}

// The value of the field at `at`, which `needle` names.
function headerFieldValue(list: string, at: number, needle: string): string {
  const start = at + needle.length;
  const end = list.indexOf("\n", start);
  return end < 0 ? list.slice(start) : list.slice(start, end);
}

// Fetch's "get": every value of the name, in list order, joined with ", "; null for none.
function headerListGet(list: string, needle: string): string | null {
  let at = list.indexOf(needle);
  if (at < 0) return null;
  let value = headerFieldValue(list, at, needle);
  at = list.indexOf(needle, at + needle.length);
  while (at >= 0) {
    value = `${value}, ${headerFieldValue(list, at, needle)}`;
    at = list.indexOf(needle, at + needle.length);
  }
  return value;
}

// Every value of the name, each on its own (`getSetCookie`).
function headerListGetAll(list: string, needle: string): string[] {
  const values: string[] = [];
  let at = list.indexOf(needle);
  while (at >= 0) {
    values.push(headerFieldValue(list, at, needle));
    at = list.indexOf(needle, at + needle.length);
  }
  return values;
}

// The list without any field of the name.
function headerListRemove(list: string, needle: string): string {
  let at = list.indexOf(needle);
  if (at < 0) return list;
  let kept = "";
  let from = 0;
  while (at >= 0) {
    kept += list.slice(from, at);
    const end = list.indexOf("\n", at + needle.length);
    if (end < 0) return kept;
    from = end;
    at = list.indexOf(needle, from);
  }
  return kept + list.slice(from);
}

// Fetch's "sort and combine": `[name, value]` pairs sorted by name, a repeated name's values
// joined with ", " -- except `set-cookie`, each of which is its own pair.
function headerListPairs(list: string): [string, string][] {
  const names: string[] = [];
  const values: string[] = [];
  const length = list.length;
  let at = 0;
  while (at < length) {
    const colon = list.indexOf(":", at + 1);
    let end = list.indexOf("\n", colon + 1);
    if (end < 0) end = length;
    names.push(list.slice(at + 1, colon));
    values.push(list.slice(colon + 1, end));
    at = end;
  }
  const count = names.length;
  // Insertion sort of the field indices: stable, so a repeated name keeps its values' order,
  // and a list holds a dozen fields, not a thousand.
  const order: number[] = [];
  for (let i = 0; i < count; i++) {
    const name = names[i];
    let hole = i;
    while (hole > 0 && names[order[hole - 1]] > name) {
      order[hole] = order[hole - 1];
      hole--;
    }
    order[hole] = i;
  }
  const pairs: [string, string][] = [];
  let i = 0;
  while (i < count) {
    const name = names[order[i]];
    let value = values[order[i]];
    i++;
    if (name !== "set-cookie") {
      while (i < count && names[order[i]] === name) {
        value = `${value}, ${values[order[i]]}`;
        i++;
      }
    }
    pairs.push([name, value]);
  }
  return pairs;
}

// One `[name, value]` of a sequence init as a field.
function headerPairField(entry: any, caller: string): string {
  let pair = entry;
  if (!Array.isArray(pair)) {
    if (pair === null || typeof pair !== "object" || typeof pair[Symbol.iterator] !== "function") {
      throw new TypeError(`${caller}: a header init entry must be a [name, value] sequence.`);
    }
    pair = Array.from(pair);
  }
  if (pair.length !== 2) {
    throw new TypeError(`${caller}: expected name/value pair to be length 2, found ${pair.length}.`);
  }
  return headerNeedle(pair[0], caller) + headerValue(pair[1], caller);
}

// A `HeadersInit` as a list: a Headers is its list as it stands (the copy is the reference), a
// sequence is its pairs, and a record its own enumerable string keys, in order, each appended.
function headerListFromInit(init: any, caller: string): string {
  if (init instanceof Headers) return init._list;
  if (init === null || (typeof init !== "object" && typeof init !== "function")) {
    throw new TypeError(`${caller}: headers must be a sequence of pairs or a record.`);
  }
  let list = "";
  if (typeof init[Symbol.iterator] === "function") {
    if (Array.isArray(init)) {
      for (let i = 0; i < init.length; i++) list += headerPairField(init[i], caller);
    } else {
      for (const entry of init) list += headerPairField(entry, caller);
    }
  } else {
    const keys = Object.keys(init);
    for (let i = 0; i < keys.length; i++) {
      const key = keys[i];
      list += headerNeedle(key, caller) + headerValue(init[key], caller);
    }
  }
  return list;
}

// Declared here rather than in the class body (see the note above `Response`).
interface Headers {
  // The header list. A Headers owns nothing else, and every edit replaces the string.
  _list: string;
}

class Headers {
  constructor(init?: any) {
    this._list = init === undefined ? "" : headerListFromInit(init, "Headers constructor");
  }

  append(name: string, value: string): void {
    const needle = headerNeedle(name, "Headers.append");
    this._list = this._list + needle + headerValue(value, "Headers.append");
  }

  delete(name: string): void {
    this._list = headerListRemove(this._list, headerNeedle(name, "Headers.delete"));
  }

  get(name: string): string | null {
    return headerListGet(this._list, headerNeedle(name, "Headers.get"));
  }

  getSetCookie(): string[] {
    return headerListGetAll(this._list, "\nset-cookie:");
  }

  has(name: string): boolean {
    return this._list.indexOf(headerNeedle(name, "Headers.has")) >= 0;
  }

  set(name: string, value: string): void {
    const needle = headerNeedle(name, "Headers.set");
    const normalized = headerValue(value, "Headers.set");
    this._list = headerListRemove(this._list, needle) + needle + normalized;
  }

  forEach(callback: (value: string, key: string, parent: Headers) => void, thisArg?: any): void {
    if (typeof callback !== "function") {
      throw new TypeError("Headers.forEach: callback is not a function.");
    }
    const pairs = headerListPairs(this._list);
    for (let i = 0; i < pairs.length; i++) {
      callback.call(thisArg, pairs[i][1], pairs[i][0], this);
    }
  }

  keys(): IterableIterator<string> {
    const pairs = headerListPairs(this._list);
    const keys: string[] = [];
    for (let i = 0; i < pairs.length; i++) keys.push(pairs[i][0]);
    return keys[Symbol.iterator]();
  }

  values(): IterableIterator<string> {
    const pairs = headerListPairs(this._list);
    const values: string[] = [];
    for (let i = 0; i < pairs.length; i++) values.push(pairs[i][1]);
    return values[Symbol.iterator]();
  }

  entries(): IterableIterator<[string, string]> {
    return headerListPairs(this._list)[Symbol.iterator]();
  }

  [Symbol.iterator](): IterableIterator<[string, string]> {
    return headerListPairs(this._list)[Symbol.iterator]();
  }

  get [Symbol.toStringTag](): string {
    return "Headers";
  }
}

// A Headers over a list that is already valid: the engine's, or another object's.
function headersOver(list: string): Headers {
  const headers = new Headers();
  headers._list = list;
  return headers;
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

  stream(): ReadableStream<Uint8Array> {
    const data = this._data;
    let offset = 0;
    return new ReadableStream<Uint8Array>({
      pull(controller) {
        if (offset >= data.length) { controller.close(); return; }
        const end = Math.min(offset + 65536, data.length);
        controller.enqueue(data.slice(offset, end));
        offset = end;
        if (offset >= data.length) controller.close();
      },
    });
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

  // The legacy numeric code of the names that have one, 0 for the rest: a `DataCloneError`
  // from `structuredClone` or `postMessage` is `code === 25` in node, and code tests for it.
  get code(): number {
    const codes: Record<string, number> = {
      IndexSizeError: 1, HierarchyRequestError: 3, WrongDocumentError: 4, InvalidCharacterError: 5,
      NoModificationAllowedError: 7, NotFoundError: 8, NotSupportedError: 9, InvalidStateError: 11,
      SyntaxError: 12, InvalidModificationError: 13, NamespaceError: 14, InvalidAccessError: 15,
      TypeMismatchError: 17, SecurityError: 18, NetworkError: 19, AbortError: 20, URLMismatchError: 21,
      QuotaExceededError: 22, TimeoutError: 23, InvalidNodeTypeError: 24, DataCloneError: 25,
    };
    return Object.prototype.hasOwnProperty.call(codes, this.name) ? codes[this.name] : 0;
  }

  get [Symbol.toStringTag](): string {
    return "DOMException";
  }
}

function formDataValue(value: any, filename?: string): string | File {
  if (!(value instanceof Blob)) {
    if (filename !== undefined) throw new TypeError("FormData filename requires a Blob");
    return String(value);
  }
  if (value instanceof File && filename === undefined) return value;
  return new File([value], filename === undefined ? "blob" : String(filename), {
    type: value.type, lastModified: value instanceof File ? value.lastModified : undefined,
  });
}

class FormData {
  private _entries: [string, string | Blob][] = [];

  append(name: string, value: any, _filename?: string): void {
    const k = String(name);
    const val = formDataValue(value, _filename);
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
    const val = formDataValue(value, _filename);
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

// The `Content-Type` the Fetch standard's "extract a body" gives a body that is a string.
const TEXT_PLAIN_UTF8 = "text/plain;charset=UTF-8";

// The body's own content type, which a Request or Response adds to its headers when they do not
// name one (Fetch, "extract a body"): a string is `text/plain;charset=UTF-8`, a Blob its `type`.
function bodyContentType(body: any): string | null {
  if (typeof body === "string") return TEXT_PLAIN_UTF8;
  if (body instanceof Blob) return body.type !== "" ? body.type : null;
  if (typeof URLSearchParams !== "undefined" && body instanceof URLSearchParams) {
    return "application/x-www-form-urlencoded;charset=UTF-8";
  }
  return null;
}

// HTML multipart encoding retains entry order and raw file bytes; only textual fields
// normalize line endings. The boundary belongs to this extraction, not to the FormData.
function formLineEndings(value: string): string {
  return value.replace(/\r\n|\r|\n/g, "\r\n");
}
function formQuoted(value: string): string {
  return formLineEndings(value).replace(/\r/g, "%0D").replace(/\n/g, "%0A").replace(/"/g, "%22");
}
const formBoundaryRandom = Math.random;
function encodeFormData(form: FormData): { bytes: Uint8Array; type: string } {
  let boundary = "----kilnFormBoundary";
  for (let i = 0; i < 4; i++) boundary += formBoundaryRandom().toString(36).slice(2);
  const parts: Uint8Array[] = [];
  let length = 0;
  function add(bytes: Uint8Array): void { parts.push(bytes); length += bytes.byteLength; }
  for (const [name, value] of form) {
    let head = `--${boundary}\r\nContent-Disposition: form-data; name="${formQuoted(name)}"`;
    if (value instanceof Blob) {
      const filename = value instanceof File ? value.name : "blob";
      head += `; filename="${formQuoted(filename)}"\r\nContent-Type: ${value.type || "application/octet-stream"}`;
    }
    add(getSharedEncoder().encode(head + "\r\n\r\n"));
    add(value instanceof Blob ? value._data : getSharedEncoder().encode(formLineEndings(value)));
    add(getSharedEncoder().encode("\r\n"));
  }
  add(getSharedEncoder().encode(`--${boundary}--\r\n`));
  const bytes = new Uint8Array(length);
  let offset = 0;
  for (let i = 0; i < parts.length; i++) { bytes.set(parts[i], offset); offset += parts[i].byteLength; }
  return { bytes, type: `multipart/form-data; boundary=${boundary}` };
}

// MIME parameters allow quoted strings with escaped characters and semicolons.
function mimeParameter(value: string, wanted: string): string | null {
  let i = value.indexOf(";");
  while (i >= 0 && i < value.length) {
    i++;
    while (value[i] === " " || value[i] === "\t") i++;
    const start = i;
    while (i < value.length && value[i] !== "=" && value[i] !== ";") i++;
    const name = value.slice(start, i).trim().toLowerCase();
    if (value[i] !== "=") continue;
    i++;
    while (value[i] === " " || value[i] === "\t") i++;
    let parameter = "";
    if (value[i] === '"') {
      i++;
      let closed = false;
      while (i < value.length) {
        const c = value[i++];
        if (c === '"') { closed = true; break; }
        if (c === "\\" && i < value.length) parameter += value[i++];
        else parameter += c;
      }
      if (!closed) return null;
      while (i < value.length && value[i] !== ";") i++;
    } else {
      const begin = i;
      while (i < value.length && value[i] !== ";") i++;
      parameter = value.slice(begin, i).trim();
    }
    if (name === wanted) return parameter;
    if (i === value.length) break;
  }
  return null;
}
function findBodyBytes(bytes: Uint8Array, needle: Uint8Array, start: number): number {
  for (let i = start; i <= bytes.byteLength - needle.byteLength; i++) {
    if (bytes[i] !== needle[0]) continue;
    let j = 1;
    while (j < needle.byteLength && bytes[i + j] === needle[j]) j++;
    if (j === needle.byteLength) return i;
  }
  return -1;
}
async function bodyFormData(self: any, contentType: string | null): Promise<FormData> {
  const bytes = await bodyBytes(self);
  const form = new FormData();
  const type = contentType === null ? "" : contentType.split(";", 1)[0].trim().toLowerCase();
  if (type === "application/x-www-form-urlencoded") {
    const params = new URLSearchParams(getSharedDecoder().decode(bytes));
    for (const [name, value] of params) form.append(name, value);
    return form;
  }
  if (type !== "multipart/form-data") throw new TypeError("Body has no supported form content type");
  const boundary = mimeParameter(contentType!, "boundary");
  if (boundary === null || boundary === "" || boundary.indexOf("\r") >= 0 || boundary.indexOf("\n") >= 0) {
    throw new TypeError("Multipart body has no valid boundary");
  }
  const encoder = getSharedEncoder();
  const marker = encoder.encode(`--${boundary}`);
  const delimiter = encoder.encode(`\r\n--${boundary}`);
  const headerEnd = encoder.encode("\r\n\r\n");
  let cursor = findBodyBytes(bytes, marker, 0);
  if (cursor < 0) throw new TypeError("Invalid multipart body");
  cursor += marker.byteLength;
  while (true) {
    if (bytes[cursor] === 45 && bytes[cursor + 1] === 45) return form;
    if (bytes[cursor] !== 13 || bytes[cursor + 1] !== 10) throw new TypeError("Invalid multipart boundary");
    const headerStart = cursor + 2;
    const end = findBodyBytes(bytes, headerEnd, headerStart);
    if (end < 0) throw new TypeError("Invalid multipart headers");
    const headers = getSharedDecoder().decode(bytes.subarray(headerStart, end)).split("\r\n");
    let disposition: string | null = null;
    let partType = "";
    for (let i = 0; i < headers.length; i++) {
      const colon = headers[i].indexOf(":");
      if (colon <= 0) throw new TypeError("Invalid multipart header");
      const name = headers[i].slice(0, colon).trim().toLowerCase();
      const value = headers[i].slice(colon + 1).trim();
      if (name === "content-disposition") disposition = value;
      else if (name === "content-type") partType = value;
    }
    if (disposition === null || disposition.split(";", 1)[0].trim().toLowerCase() !== "form-data") {
      throw new TypeError("Invalid multipart disposition");
    }
    const name = mimeParameter(disposition, "name");
    if (name === null) throw new TypeError("Multipart field has no name");
    const begin = end + 4;
    let next = findBodyBytes(bytes, delimiter, begin);
    while (next >= 0) {
      const suffix = next + delimiter.byteLength;
      if ((bytes[suffix] === 45 && bytes[suffix + 1] === 45) ||
          (bytes[suffix] === 13 && bytes[suffix + 1] === 10)) break;
      next = findBodyBytes(bytes, delimiter, next + 1);
    }
    if (next < 0) throw new TypeError("Unterminated multipart body");
    const part = bytes.subarray(begin, next);
    const filename = mimeParameter(disposition, "filename");
    if (filename === null) form.append(name, getSharedDecoder().decode(part));
    else form.append(name, new File([part], filename, { type: partType || "text/plain" }));
    cursor = next + delimiter.byteLength;
  }
}

// A body as Request and Response keep it: a string (kept as text until something needs bytes),
// bytes, or null for none. A Uint8Array (a Buffer included) is kept as given.
interface BodyReadable {
  readonly locked: boolean;
  readonly _disturbed: boolean;
  getReader(): any;
  tee(): [BodyReadable, BodyReadable];
}
type StoredBody = string | Uint8Array | BodyReadable | null;

function isBodyReadable(body: any): body is BodyReadable {
  const RS = (globalThis as any).ReadableStream;
  return typeof RS === "function" && body instanceof RS;
}

function bodyValue(body: any): Exclude<StoredBody, null> {
  if (typeof body === "string") return body;
  if (body instanceof Uint8Array) return body;
  if (body instanceof ArrayBuffer) return new Uint8Array(body);
  if (body instanceof Blob) return body._data;
  if (isBodyReadable(body)) {
    if (body.locked || body._disturbed) throw new TypeError("Body stream is locked or disturbed");
    return body;
  }
  return String(body);
}

// A body's `Content-Type` field as it is appended to a list, for the type `bodyContentType`
// gives most often: made once rather than per Response.
const TEXT_PLAIN_FIELD = `\ncontent-type:${TEXT_PLAIN_UTF8}`;
const CONTENT_TYPE_NEEDLE = "\ncontent-type:";

// The list with the body's type folded in when it names no Content-Type (Fetch, "extract a
// body" then "initialize a response"/the Request constructor).
function headerListWithContentType(list: string, contentType: string | null): string {
  if (contentType === null || list.indexOf(CONTENT_TYPE_NEEDLE) >= 0) return list;
  if (contentType === TEXT_PLAIN_UTF8) return list === "" ? TEXT_PLAIN_FIELD : list + TEXT_PLAIN_FIELD;
  return list + CONTENT_TYPE_NEEDLE + contentType;
}

// What a Request or Response rarely has, kept out of the object itself so that the common one is
// a handful of stores: every field is here on every path, so every side object shares one shape.
interface BodyExtra {
  // Response only.
  statusText: string;
  type: string;
  url: string;
  redirected: boolean;
  // Request only.
  signal: any;
  redirect: string;
  // Both: the body's state once it has been read or streamed.
  bodyUsed: boolean;
  bodyStream: any;
}

function newBodyExtra(): BodyExtra {
  return {
    statusText: "",
    type: "default",
    url: "",
    redirected: false,
    signal: undefined,
    redirect: "follow",
    bodyUsed: false,
    bodyStream: undefined,
  };
}

// The body of a Request or Response for a read that consumes it: throws once it has been read.
function consumeBody(self: any): StoredBody {
  if (self._body === null) return null;
  let extra: BodyExtra | null = self._extra;
  const stream = extra !== null && extra.bodyStream !== undefined ? extra.bodyStream :
    isBodyReadable(self._body) ? self._body : null;
  if (bodyUsed(self) || (stream !== null && stream.locked)) {
    throw new TypeError("Body has already been consumed or is locked");
  }
  if (extra === null) {
    extra = newBodyExtra();
    self._extra = extra;
  }
  extra.bodyUsed = true;
  return stream === null ? self._body : stream;
}

async function readBodyStream(stream: BodyReadable): Promise<Uint8Array> {
  const reader = stream.getReader();
  const chunks: Uint8Array[] = [];
  let length = 0;
  while (true) {
    const next = await reader.read();
    if (next.done) break;
    if (!(next.value instanceof Uint8Array)) {
      throw new TypeError("Body stream chunks must be Uint8Array values");
    }
    chunks.push(next.value);
    length += next.value.byteLength;
  }
  const bytes = new Uint8Array(length);
  let offset = 0;
  for (const chunk of chunks) {
    bytes.set(chunk, offset);
    offset += chunk.byteLength;
  }
  return bytes;
}

async function bodyText(self: any): Promise<string> {
  const body = consumeBody(self);
  if (body === null) return "";
  if (typeof body === "string") return body;
  const bytes = isBodyReadable(body) ? await readBodyStream(body) : body;
  const dec = getSharedDecoder();
  if (dec) return dec.decode(bytes);
  let res = "";
  for (let i = 0; i < bytes.length; i++) res += String.fromCharCode(bytes[i]);
  return res;
}

async function bodyBytes(self: any): Promise<Uint8Array> {
  const body = consumeBody(self);
  if (body === null) return new Uint8Array(0);
  if (typeof body === "string") {
    const enc = getSharedEncoder();
    return enc ? enc.encode(body) : new Uint8Array(0);
  }
  return isBodyReadable(body) ? readBodyStream(body) : body;
}

function bodyUsed(self: any): boolean {
  const extra: BodyExtra | null = self._extra;
  if (extra !== null && extra.bodyUsed) return true;
  const stream = extra !== null && extra.bodyStream !== undefined ? extra.bodyStream : self._body;
  return isBodyReadable(stream) && stream._disturbed;
}

// A stream over the bytes, made once: node's `body` is a `ReadableStream` of `Uint8Array`
// chunks and reading it is what marks the body used. It answered `null` until 2026-09-19.
function bodyStream(self: any): any {
  let extra: BodyExtra | null = self._extra;
  if (extra !== null && extra.bodyStream !== undefined) return extra.bodyStream;
  if (extra === null) {
    extra = newBodyExtra();
    self._extra = extra;
  }
  if (self._body === null) {
    extra.bodyStream = null;
    return null;
  }
  if (isBodyReadable(self._body)) {
    extra.bodyStream = self._body;
    return self._body;
  }
  const RS = (globalThis as any).ReadableStream;
  if (typeof RS !== "function") return null;
  const state = extra;
  const body = self._body;
  extra.bodyStream = new RS({
    pull(controller: any) {
      const bytes =
        typeof body === "string" ? (getSharedEncoder()?.encode(body) || new Uint8Array(0)) : body;
      if (bytes.length > 0) controller.enqueue(bytes);
      controller.close();
    },
  }, { highWaterMark: 0 });
  if (state.bodyUsed) {
    // The optimized byte/text read already consumed this body before its stream was requested.
    // Materialize its closed, disturbed stream only when this otherwise rare getter is used.
    extra.bodyStream.cancel();
    extra.bodyStream.getReader();
  }
  return extra.bodyStream;
}

// Fetch's body clone tees only an already-materialized stream. Byte sources can retain the
// lazy stream optimization until either branch is read. https://fetch.spec.whatwg.org/#concept-body-clone
function cloneBody(self: any): StoredBody {
  const extra: BodyExtra | null = self._extra;
  const stream = extra !== null && extra.bodyStream !== undefined ? extra.bodyStream : self._body;
  if (bodyUsed(self) || (isBodyReadable(stream) && stream.locked)) {
    throw new TypeError("Body has already been consumed or is locked");
  }
  if (!isBodyReadable(stream)) return self._body;
  const branches = stream.tee();
  self._body = branches[0];
  if (extra !== null) extra.bodyStream = branches[0];
  return branches[1];
}

function transferBody(self: any): StoredBody {
  const body = consumeBody(self);
  if (!isBodyReadable(body)) return body;
  const reader = body.getReader();
  const RS = (globalThis as any).ReadableStream;
  return new RS({
    async pull(controller: any) {
      const next = await reader.read();
      if (next.done) controller.close();
      else controller.enqueue(next.value);
    },
    cancel(reason: any) { return reader.cancel(reason); },
  }, { highWaterMark: 0 });
}

// A `status` the Response constructor accepts: WebIDL's `unsigned short` conversion, then the
// standard's range check.
function responseStatus(given: any, caller: string): number {
  let status = typeof given === "number" ? given : Number(given);
  // ToUint16: NaN and the infinities are 0; anything else truncates toward zero and wraps.
  status = status === status && status !== Infinity && status !== -Infinity ? (status - (status % 1)) % 65536 : 0;
  if (status < 0) status += 65536;
  if (status < 200 || status > 599) {
    throw new RangeError(`${caller}: init["status"] must be in the range of 200 to 599, inclusive.`);
  }
  return status;
}

// A reason phrase (RFC 9112): HTAB, SP, VCHAR and obs-text.
const REASON_PHRASE = /^[\t\x20-\x7e\x80-\xff]*$/;

function isNullBodyStatus(status: number): boolean {
  return status === 204 || status === 205 || status === 304 || status === 101 || status === 103;
}

// The `ResponseInit` just read by `readResponseInit`, handed back through these rather than
// through an allocated result. Read by nothing else.
let initStatus = 200;
let initStatusText = "";
let initList = "";

// A `ResponseInit`: each member read once, in the dictionary's order, and converted.
function readResponseInit(init: any, caller: string): void {
  initStatus = 200;
  initStatusText = "";
  initList = "";
  if (init === undefined || init === null) return;
  if (typeof init !== "object" && typeof init !== "function") {
    throw new TypeError(`${caller}: init must be an object.`);
  }
  const headers = init.headers;
  const status = init.status;
  const statusText = init.statusText;
  if (headers !== undefined) initList = headerListFromInit(headers, caller);
  if (status !== undefined) initStatus = responseStatus(status, caller);
  if (statusText !== undefined) {
    const text = typeof statusText === "string" ? statusText : `${statusText}`;
    if (!REASON_PHRASE.test(text)) throw new TypeError(`${caller}: invalid statusText.`);
    initStatusText = text;
  }
}

// A Response the implementation makes (a fetch result, `Response.error`, `json`, `redirect`,
// `clone`) is handed to the constructor through these, without an init to convert. Set
// immediately before `new Response(MADE_RESPONSE)` and read by nothing else.
const MADE_RESPONSE: any = {};
let madeStatus = 200;
let madeList = "";
let madeBody: StoredBody = null;
let madeExtra: BodyExtra | null = null;

function makeResponse(
  status: number,
  list: string,
  body: StoredBody,
  extra: BodyExtra | null
): Response {
  madeStatus = status;
  madeList = list;
  madeBody = body;
  madeExtra = extra;
  const response = new Response(MADE_RESPONSE);
  madeBody = null;
  madeExtra = null;
  return response;
}

function responseExtra(statusText: string, type: string, url: string, redirected: boolean): BodyExtra {
  const extra = newBodyExtra();
  extra.statusText = statusText;
  extra.type = type;
  extra.url = url;
  extra.redirected = redirected;
  return extra;
}

// Declared here rather than in the class body: a class-body declaration without an initializer
// still defines the property (as undefined) before the constructor runs, which on the compiled
// backend is one more property store per field per object. The constructor assigns every field
// exactly once, in one order on every path, so every Response shares one shape.
//
// Everything the standard exposes (`status`, `headers`, `bodyUsed`...) is an accessor on the
// prototype, as it is in node, so the object holds only these five.
interface Response {
  _status: number;
  // The header list, with the body's Content-Type folded in. What `serve` sends while no
  // `Headers` view exists; once `headers` has been read, the view owns the list.
  _list: string;
  _headers: Headers | null;
  _body: StoredBody;
  // statusText, type, url and redirected when any is not the default, and the body's state
  // once it has been read; null for nearly every Response a server makes.
  _extra: BodyExtra | null;
}

class Response {
  constructor(body?: any, init?: any) {
    let status = 200;
    let list = "";
    let stored: StoredBody = null;
    let extra: BodyExtra | null = null;
    if (body === MADE_RESPONSE) {
      status = madeStatus;
      list = madeList;
      stored = madeBody;
      extra = madeExtra;
    } else {
      if (init !== undefined) {
        readResponseInit(init, "Response constructor");
        status = initStatus;
        list = initList;
        if (initStatusText !== "") extra = responseExtra(initStatusText, "default", "", false);
      }
      if (typeof body === "string") {
        stored = body;
        list = list === "" ? TEXT_PLAIN_FIELD : headerListWithContentType(list, TEXT_PLAIN_UTF8);
      } else if (body instanceof FormData) {
        const encoded = encodeFormData(body);
        stored = encoded.bytes;
        list = headerListWithContentType(list, encoded.type);
      } else if (body !== undefined && body !== null) {
        stored = bodyValue(body);
        list = headerListWithContentType(list, bodyContentType(body));
      }
      if (status !== 200 && stored !== null && isNullBodyStatus(status)) {
        throw new TypeError(`Response constructor: Invalid response status code ${status}`);
      }
    }
    this._status = status;
    this._list = list;
    this._headers = null;
    this._body = stored;
    this._extra = extra;
  }

  get status(): number {
    return this._status;
  }

  get ok(): boolean {
    return this._status >= 200 && this._status <= 299;
  }

  get statusText(): string {
    const extra = this._extra;
    return extra === null ? "" : extra.statusText;
  }

  get type(): string {
    const extra = this._extra;
    return extra === null ? "default" : extra.type;
  }

  get url(): string {
    const extra = this._extra;
    return extra === null ? "" : extra.url;
  }

  get redirected(): boolean {
    const extra = this._extra;
    return extra !== null && extra.redirected;
  }

  get headers(): Headers {
    let headers = this._headers;
    if (headers === null) {
      headers = headersOver(this._list);
      this._headers = headers;
    }
    return headers;
  }

  get body(): any {
    return bodyStream(this);
  }

  get bodyUsed(): boolean {
    return bodyUsed(this);
  }

  async text(): Promise<string> {
    return bodyText(this);
  }

  async json(): Promise<any> {
    return JSON.parse(await bodyText(this));
  }

  async arrayBuffer(): Promise<ArrayBuffer> {
    const bytes = await bodyBytes(this);
    return bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength);
  }

  async bytes(): Promise<Uint8Array> {
    const body = this._body;
    const bytes = await bodyBytes(this);
    // Bytes the Response was given are copied; bytes just encoded are already this call's own.
    return bytes === body ? new Uint8Array(bytes) : bytes;
  }

  async blob(): Promise<Blob> {
    const bytes = await bodyBytes(this);
    const type = headerListGet(responseHeaderList(this), CONTENT_TYPE_NEEDLE) || "";
    return new Blob([bytes], { type });
  }

  async formData(): Promise<FormData> {
    return bodyFormData(this, headerListGet(responseHeaderList(this), CONTENT_TYPE_NEEDLE));
  }

  clone(): Response {
    if (bodyUsed(this)) throw new TypeError("Body has already been consumed");
    const extra = this._extra;
    return makeResponse(
      this._status,
      responseHeaderList(this),
      cloneBody(this),
      extra === null ? null : responseExtra(extra.statusText, extra.type, extra.url, extra.redirected)
    );
  }

  static error(): Response {
    return makeResponse(0, "", null, responseExtra("", "error", "", false));
  }

  static json(data: any, init?: any): Response {
    const body = JSON.stringify(data);
    if (body === undefined) throw new TypeError("Response.json: the data is not JSON serializable.");
    readResponseInit(init, "Response.json");
    if (isNullBodyStatus(initStatus)) {
      throw new TypeError(`Response.json: Invalid response status code ${initStatus}`);
    }
    return makeResponse(
      initStatus,
      headerListWithContentType(initList, "application/json"),
      body,
      initStatusText === "" ? null : responseExtra(initStatusText, "default", "", false)
    );
  }

  static redirect(url: any, status: number = 302): Response {
    if (status !== 301 && status !== 302 && status !== 303 && status !== 307 && status !== 308) {
      throw new RangeError("Invalid status code for redirect");
    }
    return makeResponse(status, `\nlocation:${headerValue(url, "Response.redirect")}`, null, null);
  }

  get [Symbol.toStringTag](): string {
    return "Response";
  }
}

// A Response's header list as it stands: its view's once `headers` has been read.
function responseHeaderList(response: Response): string {
  const headers = response._headers;
  return headers === null ? response._list : headers._list;
}

// A request `serve()` received, handed to the Request constructor through these rather than
// through an init object so that nothing but the Request itself is allocated per request. Set
// immediately before `new Request(SERVED_REQUEST)` and read by nothing else.
const SERVED_REQUEST: any = {};
let servedMethod = "GET";
let servedUrl = "";
let servedHeaders = "";
let servedBody: Uint8Array | null = null;

// The methods the Request constructor uppercases (Fetch, "normalize a method"); any other
// spelling is kept as given.
const NORMALIZED_METHODS: any = {
  DELETE: "DELETE", GET: "GET", HEAD: "HEAD", OPTIONS: "OPTIONS", POST: "POST", PUT: "PUT",
};

function requestMethod(given: any): string {
  const method = typeof given === "string" ? given : `${given}`;
  if (!HEADER_NAME_TOKEN.test(method)) {
    throw new TypeError(`Request constructor: '${method}' is not a valid HTTP method.`);
  }
  const upper = method.toUpperCase();
  if (upper === "CONNECT" || upper === "TRACE" || upper === "TRACK") {
    throw new TypeError(`Request constructor: '${method}' HTTP method is unsupported.`);
  }
  const normalized = NORMALIZED_METHODS[upper];
  return typeof normalized === "string" ? normalized : method;
}

// Set while `clone` constructs its copy, which shares the body rather than taking it.
let requestCloning = false;

// Declared here rather than in the class body, for the reason given above `Response`; as there,
// everything the standard exposes is a prototype accessor.
interface Request {
  _method: string;
  _url: string;
  // The header list; a `Headers` view over it is made when `headers` is first read, and owns
  // it from then on.
  _list: string;
  _headers: Headers | null;
  _body: StoredBody;
  // signal and redirect when either is not the default, and the body's state once read.
  _extra: BodyExtra | null;
}

// A Request's header list as it stands.
function requestHeaderList(request: Request): string {
  const headers = request._headers;
  return headers === null ? request._list : headers._list;
}

class Request {
  #duplex: string = "half";

  constructor(input: any, init?: any) {
    if (input === SERVED_REQUEST) {
      this._method = servedMethod;
      this._url = servedUrl;
      this._list = servedHeaders;
      this._headers = null;
      this._body = servedBody;
      this._extra = null;
      return;
    }
    let url: string;
    let method: string;
    let signal: any;
    let redirect: string;
    let list: string;
    let bodySource: any;
    let contentType: string | null = null;
    const hasInit = init !== undefined && init !== null;
    const initHeaders = hasInit ? init.headers : undefined;
    const initBody = hasInit ? init.body : undefined;
    const initMethod = hasInit ? init.method : undefined;
    const initSignal = hasInit ? init.signal : undefined;
    const initRedirect = hasInit ? init.redirect : undefined;
    const initDuplex = hasInit ? init.duplex : undefined;
    if (initDuplex !== undefined && String(initDuplex) !== "half") {
      throw new TypeError("Request constructor: duplex must be 'half'");
    }
    if (isBodyReadable(initBody) && initDuplex === undefined) {
      throw new TypeError("Request constructor: duplex is required for a stream body");
    }
    if (input instanceof Request) {
      url = input._url;
      method = initMethod !== undefined ? requestMethod(initMethod) : input._method;
      list = initHeaders !== undefined ? headerListFromInit(initHeaders, "Request constructor") : requestHeaderList(input);
      const inputExtra = input._extra;
      signal = initSignal !== undefined ? initSignal : inputExtra === null ? undefined : inputExtra.signal;
      redirect =
        initRedirect !== undefined ? String(initRedirect) : inputExtra === null ? "follow" : inputExtra.redirect;
      if (initBody !== undefined && initBody !== null) {
        bodySource = initBody;
        contentType = bodyContentType(bodySource);
      } else if (input._body !== null && !requestCloning) {
        // The input's body moves to the new Request, which leaves the input used (Fetch, the
        // Request constructor's step 41); only `clone` shares it.
        if (bodyUsed(input)) {
          throw new TypeError("Request constructor: Cannot construct a Request with a Request object that has already been used.");
        }
        bodySource = transferBody(input);
      } else {
        bodySource = requestCloning ? cloneBody(input) : input._body;
      }
    } else {
      url = String(input);
      method = initMethod !== undefined ? requestMethod(initMethod) : "GET";
      list = initHeaders !== undefined ? headerListFromInit(initHeaders, "Request constructor") : "";
      signal = initSignal;
      redirect = initRedirect !== undefined ? String(initRedirect) : "follow";
      bodySource = initBody;
      contentType = bodyContentType(bodySource);
    }
    let body: StoredBody;
    if (bodySource instanceof FormData) {
      const encoded = encodeFormData(bodySource);
      body = encoded.bytes;
      contentType = encoded.type;
    } else {
      body = bodySource === undefined || bodySource === null ? null : bodyValue(bodySource);
    }
    if (body !== null && (method === "GET" || method === "HEAD")) {
      throw new TypeError("Request constructor: Request with GET/HEAD method cannot have body.");
    }
    let extra: BodyExtra | null = null;
    if (signal !== undefined || redirect !== "follow") {
      extra = newBodyExtra();
      extra.signal = signal;
      extra.redirect = redirect;
    }
    this._method = method;
    this._url = url;
    this._list = headerListWithContentType(list, contentType);
    this._headers = null;
    this._body = body;
    this._extra = extra;
  }

  get method(): string {
    return this._method;
  }

  get url(): string {
    return this._url;
  }

  get headers(): Headers {
    let headers = this._headers;
    if (headers === null) {
      headers = headersOver(this._list);
      this._headers = headers;
    }
    return headers;
  }

  get signal(): any {
    const extra = this._extra;
    return extra === null ? undefined : extra.signal;
  }

  get redirect(): string {
    const extra = this._extra;
    return extra === null ? "follow" : extra.redirect;
  }

  get duplex(): string { return this.#duplex; }

  get body(): any {
    return bodyStream(this);
  }

  get bodyUsed(): boolean {
    return bodyUsed(this);
  }

  async text(): Promise<string> {
    return bodyText(this);
  }

  async json(): Promise<any> {
    return JSON.parse(await bodyText(this));
  }

  async arrayBuffer(): Promise<ArrayBuffer> {
    const bytes = await bodyBytes(this);
    return bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength);
  }

  async bytes(): Promise<Uint8Array> {
    const body = this._body;
    const bytes = await bodyBytes(this);
    return bytes === body ? new Uint8Array(bytes) : bytes;
  }

  async blob(): Promise<Blob> {
    const bytes = await bodyBytes(this);
    const type = headerListGet(requestHeaderList(this), CONTENT_TYPE_NEEDLE) || "";
    return new Blob([bytes], { type });
  }

  async formData(): Promise<FormData> {
    return bodyFormData(this, headerListGet(requestHeaderList(this), CONTENT_TYPE_NEEDLE));
  }

  clone(): Request {
    if (bodyUsed(this)) throw new TypeError("Body has already been consumed");
    requestCloning = true;
    try {
      return new Request(this);
    } finally {
      requestCloning = false;
    }
  }

  get [Symbol.toStringTag](): string {
    return "Request";
  }
}

// The header list of a Node-shaped headers object (a client response's `res.headers`): names are
// already lowercase tokens, and an array value (`set-cookie`) is one field per element.
function headerListFromNodeHeaders(headers: any): string {
  let list = "";
  if (headers === null || typeof headers !== "object") return list;
  const keys = Object.keys(headers);
  for (let i = 0; i < keys.length; i++) {
    const name = keys[i];
    const value = headers[name];
    if (Array.isArray(value)) {
      for (let j = 0; j < value.length; j++) list += `\n${name}:${value[j]}`;
    } else if (value !== undefined && value !== null) {
      list += `\n${name}:${value}`;
    }
  }
  return list;
}

function fetch(input: any, init?: any, redirectCount: number = 0, alreadyRedirected: boolean = false): Promise<Response> {
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

    const processObject = typeof process !== "undefined" ? process as any : null;
    // Fetch uses the same public node:http facade as an explicit builtin import. That facade
    // installs the IncomingMessage/Readable prototype relationship used by the transport.
    // Native planning retains node:http lazily when a program demands fetch, Bun, or Kiln.
    let httpModule: any = null;
    if (typeof processObject?.getBuiltinModule === "function") {
      httpModule = processObject.getBuiltinModule("node:http") || null;
    }
    if (!httpModule) {
      httpModule = processObject?.__kilnNativeModules?.http || null;
    }

    let httpReqFn = httpModule?.request;
    if (!httpReqFn) {
      try {
        const h = typeof require !== "undefined" ? require("node:http") : null;
        if (h && typeof h.request === "function") {
          httpModule = h;
          httpReqFn = h.request;
        }
      } catch (_) {}
    }

    if (!httpReqFn) {
      return reject(
        new Error("fetch requires an HTTP host capability that this realm does not provide")
      );
    }

    const headersObj: Record<string, string> = {};
    const pairs = headerListPairs(requestHeaderList(req));
    for (let i = 0; i < pairs.length; i++) headersObj[pairs[i][0]] = pairs[i][1];
    // Fetch knows the length of a materialized body before submitting the request.
    // Publish it explicitly so transport scheduling cannot turn it into chunked framing.
    const storedBody = req._body;
    if (storedBody !== null && !isBodyReadable(storedBody) && headersObj["content-length"] === undefined) {
      headersObj["content-length"] = String(typeof storedBody === "string"
        ? getSharedEncoder().encode(storedBody).byteLength : storedBody.byteLength);
    }

    const options = {
      host: parsed.hostname,
      port: parsed.port,
      method: req.method,
      path: (parsed.pathname || "/") + (parsed.search || ""),
      headers: headersObj,
    };

    let clientReq: any;
    const cancelNativeRequest = processObject?.__kilnNativeModules?.http?.__cancelRequest;
    const cancelTransport = () => {
      if (!clientReq) return;
      if (typeof cancelNativeRequest === "function") cancelNativeRequest(clientReq);
      else if (typeof clientReq.destroy === "function") clientReq.destroy();
    };
    let redirecting = false;
    const cancelAndReject = (error: any) => {
      redirecting = true;
      // Record Fetch's own error before teardown can emit a transport error.
      reject(error);
      cancelTransport();
    };
    let onAbort: any = null;
    let failResponse: any = null;
    const removeAbort = () => {
      if (onAbort !== null && typeof req.signal?.removeEventListener === "function") {
        req.signal.removeEventListener("abort", onAbort);
      }
    };
    let finishResponse: any = removeAbort;
    try {
      clientReq = httpReqFn.call(httpModule, options, (res: any) => {
          res.on("error", (error: any) => {
            if (!redirecting) reject(error);
          });
          const status = res.statusCode || 200;
          const location = res.headers?.location;
          const isRedirect = status === 301 || status === 302 || status === 303 || status === 307 || status === 308;
          if (isRedirect && typeof location === "string" && req.redirect === "error") {
            removeAbort();
            cancelAndReject(new TypeError("fetch: redirect mode is 'error'"));
            return;
          }
          if (isRedirect && typeof location === "string" && req.redirect === "follow") {
            removeAbort();
            if (redirectCount >= 20) {
              cancelAndReject(new TypeError("fetch: redirect limit exceeded"));
              return;
            }
            let target: string;
            try {
              target = new URL(location, req.url).href;
            } catch (err) {
              reject(err);
              return;
            }
            const rewriteToGet = (status === 303 && req.method !== "GET" && req.method !== "HEAD") ||
              ((status === 301 || status === 302) && req.method === "POST");
            const nextMethod = rewriteToGet ? "GET" : req.method;
            const headers = new Headers();
            const nextPairs = headerListPairs(requestHeaderList(req));
            let oldOrigin = "";
            let newOrigin = "";
            try {
              oldOrigin = new URL(req.url).origin;
              newOrigin = new URL(target).origin;
            } catch (_) {}
            for (let i = 0; i < nextPairs.length; i++) {
              const name = nextPairs[i][0].toLowerCase();
              if (rewriteToGet && (name === "content-encoding" || name === "content-language" || name === "content-length" || name === "content-location" || name === "content-type")) continue;
              if (oldOrigin !== newOrigin && (name === "authorization" || name === "cookie" || name === "cookie2" || name === "proxy-authorization")) continue;
              headers.append(nextPairs[i][0], nextPairs[i][1]);
            }
            const nextInit: any = {
              method: nextMethod,
              headers,
              signal: req.signal,
              redirect: req.redirect,
            };
            if (!rewriteToGet && req._body !== null) nextInit.body = req._body;
            // A followed redirect intentionally discards the old response. Ignore errors
            // caused by that teardown; the nested fetch now owns this request's outcome.
            redirecting = true;
            cancelTransport();
            fetch(target, nextInit, redirectCount + 1, true).then(resolve, reject);
            return;
          }
          // Publish headers immediately. A response body can be unbounded, so waiting
          // for its end here prevents callers from reading or cancelling the response.
          let ended = false;
          const stream = new ReadableStream<Uint8Array>({
            start(controller: any) {
              let onSocketClose: any = null;
              const finish = () => {
                removeAbort();
                failResponse = null;
                if (onSocketClose !== null && typeof res.socket?.removeListener === "function") {
                  res.socket.removeListener("close", onSocketClose);
                }
              };
              finishResponse = finish;
              const fail = (error: any) => {
                if (ended) return;
                ended = true;
                finish();
                controller.error(error);
              };
              failResponse = fail;
              res.on("data", (chunk: any) => {
                if (ended) return;
                controller.enqueue(typeof chunk === "string"
                  ? getSharedEncoder().encode(chunk) : new Uint8Array(chunk));
                if (controller.desiredSize <= 0 && typeof res.pause === "function") res.pause();
              });
              res.on("end", () => {
                if (ended) return;
                ended = true;
                finish();
                controller.close();
              });
              res.on("error", fail);
              res.on("aborted", () => fail(new TypeError("Response body was aborted")));
              if (res.socket && typeof res.socket.on === "function") {
                onSocketClose = () => fail(new TypeError("Response body connection closed"));
                res.socket.on("close", onSocketClose);
              }
            },
            pull() {
              if (!ended && typeof res.resume === "function") res.resume();
            },
            cancel() {
              ended = true;
              finishResponse();
              cancelTransport();
            },
          });
          resolve(makeResponse(
            status,
            headerListFromNodeHeaders(res.headers),
            req.method === "HEAD" || status === 204 || status === 205 || status === 304 ? null : stream,
            responseExtra(res.statusMessage || "", "basic", req.url, alreadyRedirected)
          ));
      });
    } catch (err) {
      return reject(err);
    }

    if (clientReq && typeof clientReq.on === "function") {
      clientReq.on("error", (err: any) => {
        if (!redirecting) reject(err);
      });
    }

    let requestReader: any = null;
    if (req.signal) {
      onAbort = () => {
        if (requestReader !== null) {
          requestReader.cancel(req.signal.reason).catch(() => {});
        }
        const reason = req.signal.reason || new Error("The operation was aborted");
        if (failResponse !== null) failResponse(reason);
        cancelTransport();
        removeAbort();
        reject(reason);
      };
      if (typeof req.signal.addEventListener === "function") {
        req.signal.addEventListener("abort", onAbort);
      }
    }

    {
      const sendBody = async () => {
        const body = consumeBody(req);
        if (typeof body === "string") {
          if (clientReq && typeof clientReq.write === "function") {
            clientReq.write(body);
          }
        } else if (isBodyReadable(body)) {
          const reader = body.getReader();
          requestReader = reader;
          try {
            while (true) {
              const next = await reader.read();
              if (req.signal?.aborted) throw req.signal.reason;
              if (next.done) break;
              if (!(next.value instanceof Uint8Array)) {
                throw new TypeError("Body stream chunks must be Uint8Array values");
              }
              if (clientReq.write(next.value) === false) {
                await new Promise<void>((resolve, reject) => {
                  const onDrain = () => { clientReq.off("error", onError); resolve(); };
                  const onError = (error: any) => { clientReq.off("drain", onDrain); reject(error); };
                  clientReq.once("drain", onDrain);
                  clientReq.once("error", onError);
                });
              }
            }
          } catch (error) {
            await reader.cancel(error).catch(() => {});
            throw error;
          } finally {
            requestReader = null;
          }
        } else if (body !== null) {
          const bodyBytes = body;
          if (bodyBytes.length > 0 && clientReq && typeof clientReq.write === "function") {
            const chunk = typeof Buffer !== "undefined"
              ? Buffer.from(bodyBytes.buffer, bodyBytes.byteOffset, bodyBytes.byteLength)
              : bodyBytes;
            clientReq.write(chunk);
          }
        }
        if (clientReq && typeof clientReq.end === "function") {
          clientReq.end();
        }
      };
      sendBody().catch((err: any) => {
        if (clientReq && typeof clientReq.destroy === "function") {
          clientReq.destroy();
        }
        reject(err);
      });
    }
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

// `serve({ fetch, port, hostname }, callback)`, the one implementation behind `Bun.serve`,
// `Kiln.serve` and `node:http`'s `serve`.
//
// It is not built on `createServer`. The host's fetch server hands each request over whole --
// method, URL and the header list's wire form as strings, the body as bytes -- and the handler's
// Response goes back through one `fetchRespond` call carrying status, header list and body. No
// IncomingMessage or ServerResponse exists, and no header is parsed, converted or copied on the
// way through: the Request keeps the engine's list, a Response keeps its own (its body's
// Content-Type folded in), and that list goes back as it is.
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
  // The origin a request's URL names when it carries no Host field (HTTP/1.0): the engine then
  // hands over the bare target, which is the one form that starts with "/" (or is "*"). Made on
  // first need, from the address the server is listening on (`port: 0` picks one).
  let fallbackOrigin = "";
  function origin(): string {
    if (fallbackOrigin === "") {
      const address = server.address();
      const listening = address !== null && typeof address === "object" ? address.port : port;
      const host = hostname === undefined ? "localhost" : hostname.indexOf(":") >= 0 ? `[${hostname}]` : hostname;
      fallbackOrigin = `http://${host}:${listening}`;
    }
    return fallbackOrigin;
  }

  function fail(resource: number, id: number, err: any) {
    respond(resource, id, 500, "", err?.message || "Internal Server Error");
  }

  function send(resource: number, id: number, response: any) {
    if (!(response instanceof Response)) {
      fail(resource, id, new TypeError("serve: the fetch handler must return a Response"));
      return;
    }
    try {
      const body = response._body;
      if (isBodyReadable(body)) {
        bodyBytes(response).then(
          (bytes: Uint8Array) => {
            const headers = response._headers;
            respond(resource, id, response._status,
              headers === null ? response._list : headers._list, bytes);
          },
          (err: any) => fail(resource, id, err)
        );
        return;
      }
      const headers = response._headers;
      respond(
        resource,
        id,
        response._status,
        headers === null ? response._list : headers._list,
        body === null ? "" : body
      );
    } catch (err: any) {
      fail(resource, id, err);
    }
  }

  // Kept out of the request listener: closures there would capture `resource` and `id`, which
  // costs every request -- a synchronous handler's too -- the cells they live in.
  function sendWhenSettled(result: any, resource: number, id: number) {
    result.then(
      (response: any) => send(resource, id, response),
      (err: any) => fail(resource, id, err)
    );
  }

  const server = httpMod.createFetchServer(
    (method: string, url: string, headers: string, body: any, resource: number, id: number) => {
      const first = url.charCodeAt(0);
      servedMethod = method;
      servedUrl = first === 47 /* "/" */ || first === 42 /* "*" */ ? origin() + url : url;
      servedHeaders = headers;
      servedBody = body === undefined ? null : body;
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
        sendWhenSettled(result, resource, id);
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

// `Kiln` and `Bun` are kiln's own globals (ADR 0068): declared in `KILN_EXTENSIONS`, installed only
// when the realm installs extensions, and non-enumerable like every built-in, so a Node-only
// program neither finds them nor sees them in `for...in` over `globalThis`.
function defineServeGlobal(name: string): void {
  const G = globalThis as any;
  if (typeof G[name] === "undefined") {
    Object.defineProperty(G, name, {
      value: { serve },
      writable: true,
      enumerable: false,
      configurable: true,
    });
  } else if (!G[name].serve) {
    G[name].serve = serve;
  }
}
if (kilnExtensions) {
  defineServeGlobal("Kiln");
  defineServeGlobal("Bun");
}
