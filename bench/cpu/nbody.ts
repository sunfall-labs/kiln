import { bench } from "../harness.mjs";

function nbody(n: number): number {
  let x: number = 0.0;
  let y: number = 0.0;
  let z: number = 0.0;
  let vx: number = 0.01;
  let vy: number = 0.02;
  let vz: number = 0.03;
  const dt: number = 0.01;
  for (let i: number = 0; i < n; i++) {
    const d2: number = x * x + y * y + z * z + 0.01;
    const mag: number = dt / (d2 * d2);
    vx -= x * mag;
    vy -= y * mag;
    vz -= z * mag;
    x += vx * dt;
    y += vy * dt;
    z += vz * dt;
  }
  return x + y + z;
}

bench("nbody", () => Math.round(nbody(50000000) * 1000));
