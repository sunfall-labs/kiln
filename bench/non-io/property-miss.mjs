import { bench } from "../harness.mjs";

// A read of a member the receiver does not have: the optional-member probe every options bag
// gets (`init.status`, `opts.signal`). Hit reads ride along so a miss-path change that slows
// the hit path shows up in the same row's neighbours.
class Point {
  constructor(x, y) {
    this.x = x;
    this.y = y;
  }
}

const literals = [];
const points = [];
for (let i = 0; i < 64; i++) {
  literals.push({ headers: { "content-type": "text/plain" }, n: i });
  points.push(new Point(i, i + 1));
}

function probeLiteral(o) {
  return o.status === undefined ? 1 : 0;
}
function probePoint(p) {
  return p.z === undefined ? 1 : 0;
}
function hitLiteral(o) {
  return o.n;
}

bench("property-miss", () => {
  let count = 0;
  for (let r = 0; r < 500; r++) {
    for (let i = 0; i < 64; i++) {
      count += probeLiteral(literals[i]);
      count += probePoint(points[i]);
      count += hitLiteral(literals[i]);
    }
  }
  return count;
});
