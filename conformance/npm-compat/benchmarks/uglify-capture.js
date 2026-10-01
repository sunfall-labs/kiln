// Build this file with `kiln build --node --profile native-release` so the
// realm has a host-backed monotonic clock. Module setup is outside call timers.
const processStart = performance.now();
const UglifyJS = require('uglify-js');
const moduleReady = performance.now();

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
let calls = 0;
function minifyAndHash(index) {
  const result = UglifyJS.minify(inputs[index % inputs.length], options[index % options.length]);
  if (result.error) throw result.error;
  const code = result.code || '';
  for (let j = 0; j < code.length; j++) hash = Math.imul(hash ^ code.charCodeAt(j), 16777619);
  hash = Math.imul(hash ^ 10, 16777619);
  calls++;
}

const firstStart = performance.now();
minifyAndHash(0);
const firstCallMs = performance.now() - firstStart;

const batchStart = performance.now();
for (let i = 0; i < 24; i++) minifyAndHash(i + 1);
const warmed24Ms = performance.now() - batchStart;
const moduleInitMs = moduleReady - processStart;
if (!(moduleInitMs > 0 && firstCallMs > 0 && warmed24Ms > 0)) {
  throw new Error('native benchmark clock did not advance');
}

console.log(JSON.stringify({
  calls,
  checksum: hash >>> 0,
  moduleInitMs,
  firstCallMs,
  warmed24Ms,
}));
