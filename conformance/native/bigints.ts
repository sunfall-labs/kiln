// BigInt: the one value in this backend whose arithmetic is not the machine's.
//
// A double cannot hold 2^53 + 1, so every check here that an answer is *exact* is a check that
// the linked provider did the work rather than the C. The rest is the type's own shape: it does
// not mix with numbers under an operator, it compares with them freely, and `String` and a
// console disagree about the `n`.

const big = 9007199254740993n;
console.log(big, typeof big, String(big), `${big}`);
console.log(big + 1n, big - 1n, big * 2n, big * big);

// Exact where a double is not.
console.log(big === 9007199254740993n, Number(big) === 9007199254740992);

// Every base the grammar allows, plus separators; all of them normalise to the same digits.
console.log(0xffn, 0o17n, 0b1011n, 1_000_000n, 0xffn === 255n);

// Division truncates toward zero rather than producing a fraction, and `%` keeps the dividend's
// sign -- neither of which is what the same spelling does to a number.
console.log(7n / 2n, -7n / 2n, 7n % 2n, -7n % 2n);
console.log(2n ** 64n, 3n ** 40n);

// Negation and bitwise-not keep the type; `~x` is `-(x + 1)` at any width.
console.log(-big, -(-big), ~5n, ~-1n, ~big);

// The bitwise three, on operands wider than 64 bits.
const wide = 2n ** 100n;
console.log((wide | 1n) - wide, (wide + 7n) & 7n, wide ^ wide);

// Mixing under an arithmetic operator is a TypeError...
for (const attempt of [() => 1n + 1, () => 1 * 2n, () => 2n ** 2, () => 1n - 0.5]) {
  try { attempt(); console.log("no throw"); }
  catch (error: any) { console.log(error.constructor.name); }
}
// ...and dividing by zero is a RangeError, which a number would answer with Infinity.
try { console.log(1n / 0n); } catch (error: any) { console.log(error.constructor.name); }
try { console.log(1n % 0n); } catch (error: any) { console.log(error.constructor.name); }

// ...but comparison across the two is fine, and `==` crosses where `===` does not.
console.log(1n < 2, 2n > 1, 1n <= 1, 1n >= 2, 1n == 1, 1n === 1, 1n != 2);

// Ordering among themselves, at a width no double could tell apart.
console.log(big > big - 1n, big < big + 1n, -big < 0n, 0n === -0n);

// Conversions in both directions. A string that is not an integer has no NaN to become.
console.log(BigInt(42), BigInt("123"), BigInt(-7), BigInt(0), Number(123n));
console.log(BigInt("  9007199254740993  ") === big, BigInt(big) === big);
try { BigInt("nope"); } catch (error: any) { console.log(error.constructor.name); }
try { BigInt(1.5); } catch (error: any) { console.log(error.constructor.name); }

// `toString` takes a radix; `valueOf` hands back the primitive it was called on.
console.log((255n).toString(16), (255n).toString(2), (255n).toString(), (-255n).toString(16));
console.log((5n).valueOf() === 5n, (5n).constructor === BigInt);
try { (255n).toString(1); } catch (error: any) { console.log(error.constructor.name); }

// Truthiness follows zero, not sign.
console.log(Boolean(0n), Boolean(-1n), !0n, !1n, 0n ? "y" : "n");

// A BigInt is a primitive, so two built any way are the same value -- and a collection keyed on
// one finds it again.
const seen = new Set([1n, 1n, 2n]);
console.log(seen.size, seen.has(1n), seen.has(BigInt("1")), [1n, 2n].indexOf(2n));

// The console adds the `n` that `String` does not; inside an array it is the console's form.
console.log([1n, -2n], { a: 3n }, String(1n) + "n" === "1n");
