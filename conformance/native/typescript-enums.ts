enum Fruits {
  Apple,
  Banana,
  Golden = 7,
  Pear,
  Label = "fruit",
}

console.log(Fruits.Apple, Fruits.Banana, Fruits.Golden, Fruits.Pear, Fruits.Label);
console.log(Fruits[0], Fruits[1], Fruits[7], Fruits[8], Fruits["fruit"]);

let calls = 0;
function computedString(): string {
  calls += 1;
  return "fruit";
}
enum ComputedString { Value = computedString() }
function computedNumber(): number {
  calls += 1;
  return 12;
}
enum ComputedNumber { Value = computedNumber() }
console.log(calls, ComputedString["fruit"], ComputedNumber[12]);

enum References { First = 3, Second = First + 1 }
const enum ConstReferences { First = 5, Second = First + 1 }
console.log(References.Second, ConstReferences.Second, ConstReferences[5]);

enum Merged { First = 9 }
enum Merged { Second = 10 }
enum Shadowed {
  First = 1,
  Second = ((First: number) => First)(4),
}
console.log(Merged.First, Merged.Second, Shadowed.Second);
