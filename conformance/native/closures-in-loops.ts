// A `let` in a loop makes one binding per iteration, so each closure captures its own box.
// Getting this wrong gives every closure the final value, which is the classic bug and is
// silent -- both engines have to agree that it does not happen.
//
// Written without an array because arrays are not native yet: three closures made in three
// iterations, kept in three named slots.
function three(): string {
  let a = (): number => 0;
  let b = a;
  let c = a;
  for (let i = 1; i < 4; i += 1) {
    const f = (): number => i;
    if (i === 1) { a = f; } else if (i === 2) { b = f; } else { c = f; }
  }
  return `${a()} ${b()} ${c()}`;
}
console.log(three());

// `var` has one binding for the whole function, which is the difference that matters.
function shared(): string {
  let a = (): number => 0;
  let b = a;
  for (var j = 1; j < 3; j += 1) {
    const f = (): number => j;
    if (j === 1) { a = f; } else { b = f; }
  }
  return `${a()} ${b()}`;
}
console.log(shared());
