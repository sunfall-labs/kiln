// kiln eval smoke test (authored for kiln, not from porffor).
import { isString, isNumber, isArray, isObject, isNull, isUndefined, isFunction, isDate, isBoolean } from 'is-what';

console.log(isString('a'), isNumber(1), isArray([1]), isObject({}), isNull(null), isUndefined(undefined), isFunction(() => {}));
console.log(isDate(new Date()), isBoolean(true), isNumber(NaN), isString(1));
console.log(isObject([]), isArray({}));
