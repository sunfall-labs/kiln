// Annex B RegExp capture statics follow the most recent native match, including the bool-only
// RegExp.test fast path used by libraries such as Showdown.
const re = /(\w+)-(\d+)/;
console.log(re.test("part-42"), RegExp.$1, RegExp.$2);
console.log("xray-7".match(re)?.[0], RegExp.$1, RegExp.$2);
console.log(/never/.test("here"), JSON.stringify(RegExp.$1), JSON.stringify(RegExp.$2));
console.log(/(a)(b)?/.test("a"), JSON.stringify(RegExp.$1), JSON.stringify(RegExp.$2));
console.log(/(?:z)/.test("z"), JSON.stringify(RegExp.$1), JSON.stringify(RegExp.$2));
const legacy = Object.getOwnPropertyDescriptor(RegExp, "$1");
console.log(legacy.get.name, legacy.set.name, legacy.enumerable, legacy.configurable);
RegExp.$1 = "ignored";
console.log(JSON.stringify(RegExp.$1));
/(\u00e9)(\u03bb)/.test("\u00e9\u03bb");
console.log(JSON.stringify([RegExp.$1, RegExp.$2]));
/(\ud83d)(\ude00)/.test("\ud83d\ude00");
console.log(JSON.stringify([RegExp.$1, RegExp.$2]));
