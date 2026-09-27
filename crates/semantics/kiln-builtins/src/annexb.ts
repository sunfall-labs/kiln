// kiln-builtins: ECMAScript Annex B (String HTML methods, Object getter/setter) & Array.fromAsync
// ADR 0005, ADR 0063 rule 4

(function installAnnexB() {
  function requireObjectCoercible(val: any): any {
    if (val === null || val === undefined) {
      throw new TypeError("Cannot convert undefined or null to object");
    }
    return val;
  }

  function escapeQuotes(s: any): string {
    return String(s).replace(/"/g, "&quot;");
  }

  // String.prototype Annex B.3 HTML methods:
  const anchor = { anchor(this: any, name: any): string {
    requireObjectCoercible(this);
    return `<a name="${escapeQuotes(name)}">${String(this)}</a>`;
  } }.anchor;

  const big = { big(this: any): string {
    requireObjectCoercible(this);
    return `<big>${String(this)}</big>`;
  } }.big;

  const blink = { blink(this: any): string {
    requireObjectCoercible(this);
    return `<blink>${String(this)}</blink>`;
  } }.blink;

  const bold = { bold(this: any): string {
    requireObjectCoercible(this);
    return `<b>${String(this)}</b>`;
  } }.bold;

  const fixed = { fixed(this: any): string {
    requireObjectCoercible(this);
    return `<tt>${String(this)}</tt>`;
  } }.fixed;

  const fontcolor = { fontcolor(this: any, color: any): string {
    requireObjectCoercible(this);
    return `<font color="${escapeQuotes(color)}">${String(this)}</font>`;
  } }.fontcolor;

  const fontsize = { fontsize(this: any, size: any): string {
    requireObjectCoercible(this);
    return `<font size="${escapeQuotes(size)}">${String(this)}</font>`;
  } }.fontsize;

  const italics = { italics(this: any): string {
    requireObjectCoercible(this);
    return `<i>${String(this)}</i>`;
  } }.italics;

  const link = { link(this: any, url: any): string {
    requireObjectCoercible(this);
    return `<a href="${escapeQuotes(url)}">${String(this)}</a>`;
  } }.link;

  const small = { small(this: any): string {
    requireObjectCoercible(this);
    return `<small>${String(this)}</small>`;
  } }.small;

  const strike = { strike(this: any): string {
    requireObjectCoercible(this);
    return `<strike>${String(this)}</strike>`;
  } }.strike;

  const sub = { sub(this: any): string {
    requireObjectCoercible(this);
    return `<sub>${String(this)}</sub>`;
  } }.sub;

  const sup = { sup(this: any): string {
    requireObjectCoercible(this);
    return `<sup>${String(this)}</sup>`;
  } }.sup;

  // Object.prototype Annex B.2 methods:
  const __defineGetter__ = { __defineGetter__(this: any, prop: any, getter: any): void {
    if (typeof getter !== "function") {
      throw new TypeError("Object.prototype.__defineGetter__: Expecting function");
    }
    Object.defineProperty(this, prop, {
      get: getter,
      enumerable: true,
      configurable: true,
    });
  } }.__defineGetter__;

  const __defineSetter__ = { __defineSetter__(this: any, prop: any, setter: any): void {
    if (typeof setter !== "function") {
      throw new TypeError("Object.prototype.__defineSetter__: Expecting function");
    }
    Object.defineProperty(this, prop, {
      set: setter,
      enumerable: true,
      configurable: true,
    });
  } }.__defineSetter__;

  const __lookupGetter__ = { __lookupGetter__(this: any, prop: any): any {
    requireObjectCoercible(this);
    let O = Object(this);
    while (O !== null) {
      const desc = Object.getOwnPropertyDescriptor(O, prop);
      if (desc !== undefined) {
        return "get" in desc ? desc.get : undefined;
      }
      O = Object.getPrototypeOf(O);
    }
    return undefined;
  } }.__lookupGetter__;

  const __lookupSetter__ = { __lookupSetter__(this: any, prop: any): any {
    requireObjectCoercible(this);
    let O = Object(this);
    while (O !== null) {
      const desc = Object.getOwnPropertyDescriptor(O, prop);
      if (desc !== undefined) {
        return "set" in desc ? desc.set : undefined;
      }
      O = Object.getPrototypeOf(O);
    }
    return undefined;
  } }.__lookupSetter__;

  // `CreateDataPropertyOrThrow`: a define, not a `Set` -- a setter on the target is not called, and
  // a property that cannot be redefined (non-configurable, or the target not extensible) is a
  // TypeError rather than a write that silently lands or is dropped. With an iterator that never
  // ends, the write that cannot land is the only thing that stops the loop.
  function createDataPropertyOrThrow(target: any, key: number, value: any): void {
    Object.defineProperty(target, key, { value, writable: true, enumerable: true, configurable: true });
  }

  // Array.fromAsync (ES2024)
  // `Array.fromAsync` is an ordinary built-in function -- its prototype is `Function.prototype`,
  // not `AsyncFunction.prototype`, and it has no [[Construct]] -- whose body runs as an async
  // function, so every abrupt completion becomes a rejection.
  const fromAsync = { fromAsync(this: any, asyncItems: any, mapfn: any = undefined, thisArg: any = undefined): Promise<any[]> {
    return fromAsyncBody.call(this, asyncItems, mapfn, thisArg);
  } }.fromAsync;

  async function fromAsyncBody(this: any, asyncItems: any, mapfn?: any, thisArg?: any): Promise<any[]> {
    const C = this;
    const mapping = mapfn !== undefined;
    if (mapping && typeof mapfn !== "function") {
      throw new TypeError(mapfn + " is not a function");
    }
    if (asyncItems === null || asyncItems === undefined) {
      throw new TypeError("Cannot convert undefined or null to object");
    }

    let usingAsyncIterator: any = undefined;
    let usingSyncIterator: any = undefined;

    const asyncIterFn = asyncItems[Symbol.asyncIterator];
    if (asyncIterFn !== undefined && asyncIterFn !== null) {
      if (typeof asyncIterFn !== "function") {
        throw new TypeError(asyncIterFn + " is not a function");
      }
      usingAsyncIterator = asyncIterFn;
    } else {
      const syncIterFn = asyncItems[Symbol.iterator];
      if (syncIterFn !== undefined && syncIterFn !== null) {
        if (typeof syncIterFn !== "function") {
          throw new TypeError(syncIterFn + " is not a function");
        }
        usingSyncIterator = syncIterFn;
      }
    }

    if (usingAsyncIterator !== undefined || usingSyncIterator !== undefined) {
      const iterator = usingAsyncIterator !== undefined
        ? usingAsyncIterator.call(asyncItems)
        : usingSyncIterator.call(asyncItems);

      if (typeof iterator !== "object" || iterator === null) {
        throw new TypeError("Iterator must be an object");
      }

      let A: any;
      if (typeof C === "function") {
        try {
          A = new C();
        } catch {
          A = [];
        }
      } else {
        A = [];
      }

      let k = 0;
      try {
        while (true) {
          if (k >= Number.MAX_SAFE_INTEGER) {
            throw new TypeError("Array.fromAsync: iterable exceeded max safe integer length");
          }
          const nextPromise = iterator.next();
          const nextResult = await nextPromise;
          if (typeof nextResult !== "object" || nextResult === null) {
            throw new TypeError("Iterator result is not an object");
          }
          if (nextResult.done) {
            break;
          }
          // A sync iterator is wrapped as `CreateAsyncFromSyncIterator`, which awaits each
          // value; an async iterator's values are taken as they are.
          let value = usingAsyncIterator !== undefined ? nextResult.value : await nextResult.value;
          if (mapping) {
            value = await mapfn.call(thisArg, value, k);
          }
          createDataPropertyOrThrow(A, k, value);
          k++;
        }
      } catch (err) {
        if (typeof iterator.return === "function") {
          try {
            await iterator.return();
          } catch {
            // Keep original err
          }
        }
        throw err;
      }

      A.length = k;
      return A;
    }

    // Array-like fallback
    const arrayLike = Object(asyncItems);
    // `ToLength`, whose `ToNumber` refuses a BigInt -- which `Number()` would convert.
    const rawLen = +arrayLike.length;
    const len = Number.isNaN(rawLen) ? 0 : Math.min(Math.max(Math.trunc(rawLen), 0), Number.MAX_SAFE_INTEGER);

    let A: any;
    if (typeof C === "function") {
      try {
        A = new C(len);
      } catch {
        A = new Array(len);
      }
    } else {
      A = new Array(len);
    }

    for (let k = 0; k < len; k++) {
      let value = await arrayLike[k];
      if (mapping) {
        value = await mapfn.call(thisArg, value, k);
      }
      createDataPropertyOrThrow(A, k, value);
    }

    A.length = len;
    return A;
  }

  // Built-in methods are not constructors, so each is a method definition (`{ m() {} }.m`)
  // rather than a function declaration, which would have [[Construct]].
  // One descriptor for every member, not a literal per call: `defineProperty` reads its fields and
  // keeps nothing, and a literal per member was the largest share of the garbage every realm's
  // bootstrap made (realms issue 08).
  const methodDescriptor: PropertyDescriptor = { value: undefined, writable: true, enumerable: false, configurable: true };
  function defineMethod(target: any, name: string | symbol, fn: any): void {
    methodDescriptor.value = fn;
    Object.defineProperty(target, name, methodDescriptor);
    methodDescriptor.value = undefined;
  }

  // Install String.prototype Annex B.3 methods
  defineMethod(String.prototype, "anchor", anchor);
  defineMethod(String.prototype, "big", big);
  defineMethod(String.prototype, "blink", blink);
  defineMethod(String.prototype, "bold", bold);
  defineMethod(String.prototype, "fixed", fixed);
  defineMethod(String.prototype, "fontcolor", fontcolor);
  defineMethod(String.prototype, "fontsize", fontsize);
  defineMethod(String.prototype, "italics", italics);
  defineMethod(String.prototype, "link", link);
  defineMethod(String.prototype, "small", small);
  defineMethod(String.prototype, "strike", strike);
  defineMethod(String.prototype, "sub", sub);
  defineMethod(String.prototype, "sup", sup);

  // Install Object.prototype Annex B.2 methods
  defineMethod(Object.prototype, "__defineGetter__", __defineGetter__);
  defineMethod(Object.prototype, "__defineSetter__", __defineSetter__);
  defineMethod(Object.prototype, "__lookupGetter__", __lookupGetter__);
  defineMethod(Object.prototype, "__lookupSetter__", __lookupSetter__);

  // Install Array.fromAsync
  defineMethod(Array, "fromAsync", fromAsync);
})();
