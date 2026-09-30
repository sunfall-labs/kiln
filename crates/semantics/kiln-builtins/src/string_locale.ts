// Public locale case methods stay in TypeScript. Locale grammar, aliases, Unicode mappings,
// and contextual rules are owned by kiln-intl through the private native provider seam.
(function installStringLocaleCase(): void {
  const localeTools = (globalThis as any).__kilnLocaleTools;
  const hostCase = localeTools.caseMap;
  const localesList = localeTools.localesList;

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
