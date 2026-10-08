import { bench } from "../harness.mjs";

function mandelbrot(): number {
  const width: number = 500;
  const height: number = 500;
  const maxIter: number = 100;
  let total: number = 0;
  for (let y: number = 0; y < height; y++) {
    const ci: number = (y * 2.0) / height - 1.0;
    for (let x: number = 0; x < width; x++) {
      const cr: number = (x * 3.0) / width - 2.0;
      let zr: number = 0.0;
      let zi: number = 0.0;
      let iter: number = 0;
      while (zr * zr + zi * zi <= 4.0 && iter < maxIter) {
        const nextZr: number = zr * zr - zi * zi + cr;
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
