async function runMicrotasks() {
  let counter = 0;
  for (let i = 0; i < 20000; i++) {
    await Promise.resolve(i);
    counter += 1;
  }
  console.log("microtasks completed:", counter);
}

runMicrotasks();
