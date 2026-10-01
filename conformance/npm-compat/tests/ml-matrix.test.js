// kiln eval smoke test (authored for kiln, not from porffor).
const { Matrix, determinant } = require('ml-matrix');
const m = new Matrix([[1, 2], [3, 4]]);
console.log(JSON.stringify(m.mmul(new Matrix([[5], [6]])).to2DArray()));
console.log(JSON.stringify(m.transpose().to2DArray()));
console.log(determinant(m));
console.log(JSON.stringify(Matrix.eye(2).to2DArray()));
console.log(m.get(0, 1), m.rows, m.columns);
