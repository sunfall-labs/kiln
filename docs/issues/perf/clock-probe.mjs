const start = Date.now();
setTimeout(() => {
  const waited = Date.now() - start;
  console.log(`epoch-ish ${start > 1_600_000_000_000} waited-ok ${waited >= 50 && waited < 500} ${waited}`);
}, 50);
