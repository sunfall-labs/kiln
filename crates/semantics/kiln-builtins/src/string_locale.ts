// Public locale case methods stay in TypeScript. Locale grammar, aliases, Unicode mappings,
// and contextual rules are owned by kiln-intl through the private native provider seam.
(function installStringLocaleCase(): void {
  const hostCase = (globalThis as any).__kilnHostStringLocaleCase;
  delete (globalThis as any).__kilnHostStringLocaleCase;

  function toLength(value: any): number {
    const number = +value;
    if (!(number > 0)) return 0;
    if (number === Infinity) return Number.MAX_SAFE_INTEGER;
    return Math.min(Math.floor(number), Number.MAX_SAFE_INTEGER);
  }

  function localesList(locales: any): string[] {
    if (locales === undefined) return [];
    let values: string[] = [];
    if (typeof locales === "string") {
      values = [locales];
    } else {
      if (locales === null || (typeof locales !== "object" && typeof locales !== "function")) {
        throw new TypeError("Cannot convert undefined or null to object");
      }
      const object = Object(locales);
      const length = toLength(object.length);
      for (let index = 0; index < length; index++) {
        const item = object[index];
        if (item === undefined) continue;
        if (typeof item === "symbol") throw new TypeError("Cannot convert a Symbol value to a string");
        values.push(`${item}`);
      }
    }
    // The provider is the single owner of BCP 47 parsing/canonicalization. Calling it with an
    // empty input validates each supplied tag without inventing a TypeScript locale parser.
    for (let index = 0; index < values.length; index++) {
      hostCase("", values[index], false);
    }
    return values;
  }

  function receiverString(value: any, name: string): string {
    if (value === null || value === undefined) {
      throw new TypeError(`String.prototype.${name} called on null or undefined`);
    }
    return `${value}`;
  }

  function toLocaleLowerCase(this: any, locales?: any): string {
    const value = receiverString(this, "toLocaleLowerCase");
    const tags = localesList(locales);
    return hostCase(value, tags.length === 0 ? "" : tags[0], false);
  }

  function toLocaleUpperCase(this: any, locales?: any): string {
    const value = receiverString(this, "toLocaleUpperCase");
    const tags = localesList(locales);
    return hostCase(value, tags.length === 0 ? "" : tags[0], true);
  }

  Object.defineProperty(String.prototype, "toLocaleLowerCase", {
    value: toLocaleLowerCase,
    writable: true,
    configurable: true,
  });
  Object.defineProperty(String.prototype, "toLocaleUpperCase", {
    value: toLocaleUpperCase,
    writable: true,
    configurable: true,
  });
})();
