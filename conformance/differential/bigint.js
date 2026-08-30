// BigInt: a separate numeric type, with its own literals.
const big = 9007199254740993n;
console.log(big, typeof big, String(big));
console.log(big + 1n, big * 2n, big - 1n);

// Exact where a double is not: 2^53 + 1 is representable here and not as a number.
console.log(big === 9007199254740993n, Number(big) === 9007199254740992);

// Every base the grammar allows, plus separators.
console.log(0xffn, 0o17n, 0b1011n, 1_000_000n);

// Division truncates rather than producing a fraction.
console.log(7n / 2n, -7n / 2n, 7n % 2n);
console.log(2n ** 64n);

// Mixing with a number is a TypeError in arithmetic...
try {
  console.log(1n + 1);
} catch (error) {
  console.log(error.constructor.name);
}
// ...but comparisons across the two are fine, and `==` crosses the types.
console.log(1n < 2, 2n > 1, 1n == 1, 1n === 1);

// Conversions in both directions.
console.log(BigInt(42), BigInt("123"), Number(123n));
console.log((255n).toString(16), (255n).toString(2));

// Truthiness follows zero, not sign.
console.log(Boolean(0n), Boolean(-1n), !0n);
