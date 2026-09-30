// Primitive wrappers keep ordinary object identity while their matching prototype methods and
// primitive conversions see the boxed value. String index descriptors are the wrapper's exotic
// own properties, and Object(string) uses the same representation as `new String`.
const text = new String('A\u{1f600}');
console.log(typeof text, text instanceof String, String(text), text.valueOf());
console.log(Object.prototype.toString.call(text), Object.prototype.valueOf.call(text) === text);
console.log(Object.keys(text).join(','), text.length, text[1], text[2]);
const index = Object.getOwnPropertyDescriptor(text, '0');
const length = Object.getOwnPropertyDescriptor(text, 'length');
console.log(index.value, index.enumerable, index.writable, index.configurable,
  length.value, length.enumerable, length.writable, length.configurable);
console.log(JSON.stringify(Object.assign({}, Object('ab'))));

const number = Object(7);
const boolean = new Boolean(false);
const symbol = Object(Symbol('s'));
const bigint = Object(9n);
console.log(number instanceof Number, number.valueOf(), number.toString());
console.log(boolean instanceof Boolean, Boolean.prototype.valueOf.call(boolean),
  Boolean.prototype.toString.call(boolean), !!boolean);
console.log(symbol instanceof Symbol, Symbol.prototype.valueOf.call(symbol) === symbol.valueOf(),
  Object.prototype.toString.call(symbol));
console.log(bigint instanceof BigInt, BigInt.prototype.valueOf.call(bigint), bigint.toString());
try { String.prototype.valueOf.call({}); } catch (error) { console.log(error instanceof TypeError); }
try { String.prototype.valueOf.call(number); } catch (error) { console.log(error instanceof TypeError); }
try { Number.prototype.valueOf.call(boolean); } catch (error) { console.log(error instanceof TypeError); }
try { Symbol.prototype.valueOf.call(number); }
catch (error) { console.log(error instanceof TypeError); }
try { new String(Symbol('s')); } catch (error) { console.log(error instanceof TypeError); }
