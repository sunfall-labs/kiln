const A = Array, O = Object, J = JSON;
console.log(typeof A.isArray, typeof A.from, typeof O.keys, typeof J.parse);
console.log(A.isArray([1]), O.keys({ x: 1 }).join(','));
