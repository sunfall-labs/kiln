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
  function anchor(this: any, name: any): string {
    requireObjectCoercible(this);
    return `<a name="${escapeQuotes(name)}">${String(this)}</a>`;
  }

  function big(this: any): string {
    requireObjectCoercible(this);
    return `<big>${String(this)}</big>`;
  }

  function blink(this: any): string {
    requireObjectCoercible(this);
    return `<blink>${String(this)}</blink>`;
  }

  function bold(this: any): string {
    requireObjectCoercible(this);
    return `<b>${String(this)}</b>`;
  }

  function fixed(this: any): string {
    requireObjectCoercible(this);
    return `<tt>${String(this)}</tt>`;
  }

  function fontcolor(this: any, color: any): string {
    requireObjectCoercible(this);
    return `<font color="${escapeQuotes(color)}">${String(this)}</font>`;
  }

  function fontsize(this: any, size: any): string {
    requireObjectCoercible(this);
    return `<font size="${escapeQuotes(size)}">${String(this)}</font>`;
  }

  function italics(this: any): string {
    requireObjectCoercible(this);
    return `<i>${String(this)}</i>`;
  }

  function link(this: any, url: any): string {
    requireObjectCoercible(this);
    return `<a href="${escapeQuotes(url)}">${String(this)}</a>`;
  }

  function small(this: any): string {
    requireObjectCoercible(this);
    return `<small>${String(this)}</small>`;
  }

  function strike(this: any): string {
    requireObjectCoercible(this);
    return `<strike>${String(this)}</strike>`;
  }

  function sub(this: any): string {
    requireObjectCoercible(this);
    return `<sub>${String(this)}</sub>`;
  }

  function sup(this: any): string {
    requireObjectCoercible(this);
    return `<sup>${String(this)}</sup>`;
  }

  // Object.prototype Annex B.2 methods:
  function __defineGetter__(this: any, prop: any, getter: any): void {
    if (typeof getter !== "function") {
      throw new TypeError("Object.prototype.__defineGetter__: Expecting function");
    }
    Object.defineProperty(this, prop, {
      get: getter,
      enumerable: true,
      configurable: true,
    });
  }

  function __defineSetter__(this: any, prop: any, setter: any): void {
    if (typeof setter !== "function") {
      throw new TypeError("Object.prototype.__defineSetter__: Expecting function");
    }
    Object.defineProperty(this, prop, {
      set: setter,
      enumerable: true,
      configurable: true,
    });
  }

  function __lookupGetter__(this: any, prop: any): any {
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
  }

  function __lookupSetter__(this: any, prop: any): any {
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
  }

  // Array.fromAsync (ES2024)
  async function fromAsync(this: any, asyncItems: any, mapfn?: any, thisArg?: any): Promise<any[]> {
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
          let value = await nextResult.value;
          if (mapping) {
            value = await mapfn.call(thisArg, value, k);
          }
          A[k] = value;
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
    const rawLen = Number(arrayLike.length);
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
      A[k] = value;
    }

    A.length = len;
    return A;
  }

  function defineMethod(target: any, name: string | symbol, fn: any): void {
    Object.defineProperty(target, name, {
      value: fn,
      writable: true,
      enumerable: false,
      configurable: true,
    });
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
