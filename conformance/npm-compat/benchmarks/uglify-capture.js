// Repeated representative Uglify calls amplify dynamic-function capture overhead.
// Run from this file's corpus checkout so `uglify-js` resolves through node_modules.
const UglifyJS = require('uglify-js');

const inputs = [
  'function add(a,b){return a+b} function fold(xs){var n=0;for(var i=0;i<xs.length;i++)n=add(n,xs[i]);return n} console.log(fold([1,2,3,4,5]));',
  'var values=[1,2,3,4,5,6,7,8]; function map(fn){var out=[];for(var i=0;i<values.length;i++)out.push(fn(values[i],i));return out} console.log(map(function(value,index){return value*2+index}).join(","));',
  'function make(name){return function(value){return name+":"+value}} var left=make("left"),right=make("right"); console.log(left(1),right(2));',
];
const options = [
  { compress: {}, mangle: false },
  { compress: {}, mangle: { toplevel: true } },
  { compress: { defaults: true }, mangle: { toplevel: true }, output: { beautify: false } },
];

let hash = 2166136261;
let count = 0;
for (let round = 0; round < 8; round++) {
  for (let i = 0; i < inputs.length; i++) {
    const result = UglifyJS.minify(inputs[i], options[(round + i) % options.length]);
    if (result.error) throw result.error;
    const code = result.code || '';
    for (let j = 0; j < code.length; j++) hash = Math.imul(hash ^ code.charCodeAt(j), 16777619);
    hash = Math.imul(hash ^ 10, 16777619);
    count++;
  }
}
console.log(JSON.stringify({ count, hash: hash >>> 0 }));
