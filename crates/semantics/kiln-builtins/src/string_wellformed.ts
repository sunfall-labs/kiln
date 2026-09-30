// UTF-16 string well-formedness is shared by both engines, including literal strings.
(function installStringWellFormed() {
  function text(receiver: any): string {
    if (receiver === null || receiver === undefined || typeof receiver === 'symbol') {
      throw new TypeError('Cannot convert receiver to string');
    }
    return typeof receiver === 'string' ? receiver : `${receiver}`;
  }
  const isWellFormed = { isWellFormed(this: any): boolean {
    const value = text(this);
    for (let i = 0; i < value.length; i++) {
      const code = value.charCodeAt(i);
      if (code >= 0xd800 && code <= 0xdbff) {
        const next = value.charCodeAt(++i);
        if (!(next >= 0xdc00 && next <= 0xdfff)) return false;
      } else if (code >= 0xdc00 && code <= 0xdfff) return false;
    }
    return true;
  } }.isWellFormed;
  const toWellFormed = { toWellFormed(this: any): string {
    const value = text(this);
    let result = '';
    for (let i = 0; i < value.length; i++) {
      const code = value.charCodeAt(i);
      if (code >= 0xd800 && code <= 0xdbff) {
        const next = value.charCodeAt(i + 1);
        if (next >= 0xdc00 && next <= 0xdfff) {
          result += value[i] + value[++i];
        } else result += '\ufffd';
      } else result += code >= 0xdc00 && code <= 0xdfff ? '\ufffd' : value[i];
    }
    return result;
  } }.toWellFormed;
  Object.defineProperty(String.prototype, 'isWellFormed', {
    value: isWellFormed, writable: true, configurable: true,
  });
  Object.defineProperty(String.prototype, 'toWellFormed', {
    value: toWellFormed, writable: true, configurable: true,
  });
})();
