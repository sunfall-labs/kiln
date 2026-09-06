// Dynamic import rejection settling
console.log("start");

import("./nonexistent-module-to-reject.mjs")
  .then(
    () => console.log("unexpected resolve"),
    err => {
      console.log("caught reject:", typeof err === "object" && err !== null);
      console.log("error message present:", typeof err.message === "string" && err.message.length > 0);
    }
  );

async function testAwait() {
  try {
    await import("./nonexistent-module-to-reject-2.mjs");
    console.log("unexpected await resolve");
  } catch (err) {
    console.log("caught await reject:", typeof err === "object" && err !== null);
  }
}
testAwait();

console.log("end");
