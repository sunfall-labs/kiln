const o = JSON.parse('{"a\\u0000b":1}');
console.log(JSON.stringify(Object.keys(o)), o["a\u0000b"], o["a"], Object.keys(o)[0].length);
