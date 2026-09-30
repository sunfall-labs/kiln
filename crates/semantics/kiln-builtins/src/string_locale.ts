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
    if (typeof locales === "string") {
      hostCase("", locales, false);
      return [locales];
    }
    if (locales === null) throw new TypeError("Cannot convert undefined or null to object");
    // CanonicalizeLocaleList applies ToObject to every non-string, non-null input. That makes a
    // numeric or boolean primitive an empty array-like list, rather than a type error.
    const object = Object(locales);
    const length = toLength(object.length);
    const values: string[] = [];
    for (let index = 0; index < length; index++) {
      // `in` performs the spec's HasProperty before Get, preserving inherited indices and proxy
      // trap order while distinguishing a hole from a present undefined (which is a TypeError).
      if (!(index in object)) continue;
      const item = object[index];
      if (typeof item !== "string" && (item === null || typeof item !== "object")) {
        throw new TypeError("String.prototype.toLocaleLowerCase locales must be strings or objects");
      }
      const tag = typeof item === "string" ? item : `${item}`;
      // Validate before touching the next index: an invalid earlier tag prevents later getters.
      // BCP 47 parsing/canonicalization belongs to kiln-intl, not a bootstrap regex.
      hostCase("", tag, false);
      values.push(tag);
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
    return hostCase(value, tags.length === 0 ? undefined : tags[0], false);
  }

  function toLocaleUpperCase(this: any, locales?: any): string {
    const value = receiverString(this, "toLocaleUpperCase");
    const tags = localesList(locales);
    return hostCase(value, tags.length === 0 ? undefined : tags[0], true);
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
