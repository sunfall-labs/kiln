// Without an `unhandledRejection` listener the default mode throws, and an
// `uncaughtException` listener takes it with the origin "unhandledRejection".
process.on("uncaughtException", (error, origin) => {
  console.log("uncaughtException", origin, error.name, error.code, error instanceof Error);
  if (error.code === "ERR_UNHANDLED_REJECTION") {
    console.log(error.message.slice(error.message.indexOf("The promise")));
  }
});
Promise.reject(Symbol("s"));
Promise.reject(new (class Custom {})());
Promise.reject({ plain: true });
Promise.reject([1, 2]);
async function inner() { throw new TypeError("inner"); }
inner();
(async () => {
  try { await inner(); } catch (error) { console.log("caught", error.message); }
})();
setTimeout(() => console.log("still running"), 1);
