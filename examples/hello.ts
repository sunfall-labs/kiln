let total: number = 0;
while (total < 3) {
  total += 1;
}

const state = total === 3 ? "ready" : "wrong";
console.log("kiln", state, total);
