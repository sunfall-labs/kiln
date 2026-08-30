import _ from "lodash";
console.log(JSON.stringify(_.chunk([1, 2, 3, 4, 5], 2)));
console.log(JSON.stringify(_.groupBy([1.2, 2.4, 2.9], Math.floor)));
console.log(JSON.stringify(_.merge({ a: { b: 1 } }, { a: { c: 2 } })));
console.log(JSON.stringify(_.cloneDeep({ x: [1, { y: 2 }] })), _.camelCase("foo bar"));
console.log(_.sum([1, 2, 3]), _.uniq([1, 1, 2]).join(","), _.range(3).join("-"));
