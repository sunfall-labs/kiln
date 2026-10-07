// Realms issue 24: exercise concurrent realm teardown after a virtual 3 s timer.
console.log("hi");
setTimeout(() => {}, 3000);
