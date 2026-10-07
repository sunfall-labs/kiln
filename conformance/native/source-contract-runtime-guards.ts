// Resolved source contracts guide speculation; callers still cross runtime guards.
type Scalar = number;
function addOne(value: Scalar): Scalar { return value + 1; }
console.log(addOne(4));
console.log(addOne("a" as any));
console.log(addOne(true as any));
