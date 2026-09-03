function nbody(n) {
  let x = 0.0;
  let y = 0.0;
  let z = 0.0;
  let vx = 0.01;
  let vy = 0.02;
  let vz = 0.03;
  const dt = 0.01;
  for (let i = 0; i < n; i++) {
    const d2 = x * x + y * y + z * z + 0.01;
    const mag = dt / (d2 * d2);
    vx -= x * mag;
    vy -= y * mag;
    vz -= z * mag;
    x += vx * dt;
    y += vy * dt;
    z += vz * dt;
  }
  return x + y + z;
}
console.log(Math.round(nbody(50000000) * 1000));
