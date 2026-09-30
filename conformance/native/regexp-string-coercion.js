let conversions = 0;
const subject = { toString() { conversions++; return 'ab'; }, valueOf() { throw new Error('number hint'); } };
console.log(/^ab$/.test(subject), /a(b)/.exec(subject)[1], conversions);
const specifier = { toString() { return ' >-,.0f'; } };
console.log(/^(?:(.)?([<>=^]))?([+\-( ])?([$#])?(0)?(\d+)?(,)?(\.\d+)?(~)?([a-z%])?$/i.exec(specifier)[2]);
try { /x/.exec(Symbol('x')); } catch (error) { console.log(error.name); }
try { RegExp.prototype.exec.call({}, subject); } catch (error) { console.log(error.name, conversions); }
