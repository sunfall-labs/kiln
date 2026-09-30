// kiln eval smoke test (authored for kiln, not from porffor).
const {mat4, vec3, quat} = require('gl-matrix');
const m = mat4.create();
mat4.translate(m, m, [1, 2, 3]);
console.log(m[12], m[13], m[14]);
console.log(vec3.length([3, 4, 0]));
const q = quat.create();
quat.setAxisAngle(q, [0, 1, 0], Math.PI / 2);
console.log(q[1].toFixed(4));
const out = vec3.create();
vec3.transformQuat(out, vec3.fromValues(1, 0, 0), q);
console.log(out[0].toFixed(4), out[2].toFixed(4));
const a = vec3.fromValues(1, 2, 3);
const b = vec3.fromValues(4, 5, 6);
console.log(vec3.dot(a, b));
