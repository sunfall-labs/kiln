// The bounded-loop analysis keeps an integer candidate for this Phi, but it is not
// proven to have an integer twin. A prepared dynamic method call must therefore pass
// the boxed current value after the loop rather than the stale candidate twin.
function dropWhile<T>(self: Iterable<T>, predicate: (value: T, index: number) => boolean): T[] {
  const input = Array.from(self)
  let i = 0
  while (i < input.length) {
    if (!predicate(input[i], i)) break
    i++
  }
  return input.slice(i)
}

type PredicateResult = { _tag: "Success" } | { _tag: "Failure" }

function isFailure(result: PredicateResult): boolean {
  return result._tag === "Failure"
}

function dropWhileFilter<T>(self: Iterable<T>, predicate: (value: T, index: number) => PredicateResult): T[] {
  const input = Array.from(self)
  let i = 0
  while (i < input.length) {
    if (isFailure(predicate(input[i], i))) break
    i++
  }
  return input.slice(i)
}

console.log(JSON.stringify(dropWhile([1, 2], (_value, index) => index < 2)))
console.log(JSON.stringify(dropWhileFilter([1, 2, 3], (_value, index) => index < 2
  ? { _tag: "Success" }
  : { _tag: "Failure" })))
