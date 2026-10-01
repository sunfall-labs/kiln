const empty = String();
const wrapped = new String();
const boxed = Object(empty);
if (empty !== '' || empty.length !== 0 || wrapped.valueOf() !== '' ||
    wrapped.toString() !== '' || wrapped.length !== 0 || boxed.valueOf() !== '')
  throw new Error('zero-argument String lost its canonical empty value');
console.log(empty === '', wrapped instanceof String, Object.keys(wrapped).length,
            wrapped.concat('x'), wrapped.slice(0), boxed.length);
