// Shared CanonicalizeLocaleList behavior for the native string and Intl facades. ICU4X owns BCP
// 47 validation, aliases, and canonicalization; this layer owns ECMAScript's ToObject, HasProperty,
// getter order, and String/Object checks.
(() => {
  const caseMap = (globalThis as any).__kilnHostStringLocaleCase;
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
      caseMap("", locales, false);
      return [locales];
    }
    if (locales === null) throw new TypeError("Cannot convert undefined or null to object");
    const object = Object(locales);
    const length = toLength(object.length);
    const values: string[] = [];
    for (let index = 0; index < length; index++) {
      if (!(index in object)) continue;
      const item = object[index];
      if (typeof item !== "string" && (item === null || typeof item !== "object")) {
        throw new TypeError("Locales must be strings or objects");
      }
      const tag = typeof item === "string" ? item : `${item}`;
      caseMap("", tag, false);
      values.push(tag);
    }
    return values;
  }

  (globalThis as any).__kilnLocaleTools = { caseMap, localesList };
})();
