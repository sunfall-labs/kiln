import { bench } from "../harness.mjs";

function mandelbrot() {
  const width = 500;
  const height = 500;
  const maxIter = 100;
  let total = 0;
  for (let y = 0; y < height; y++) {
    const ci = (y * 2.0) / height - 1.0;
    for (let x = 0; x < width; x++) {
      const cr = (x * 3.0) / width - 2.0;
      let zr = 0.0;
      let zi = 0.0;
      let iter = 0;
      while (zr * zr + zi * zi <= 4.0 && iter < maxIter) {
        const nextZr = zr * zr - zi * zi + cr;
        zi = 2.0 * zr * zi + ci;
        zr = nextZr;
        iter++;
      }
      total += iter;
    }
  }
  return total;
}

bench("mandelbrot", mandelbrot);
