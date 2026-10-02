// kiln-native: profile=native-release
// A mutation elsewhere on the built-in surface must not make the canonical string iterator
// call itself through the generic iterator protocol. The consumer still resolves @@iterator
// first, so a String.prototype override remains observable.
const text = "A\u{1f600}B";
const originalIterator = String.prototype[Symbol.iterator];
Object.prototype.__stringIteratorNoise = 1;

const spread = [...text].join("|");
let loop = "";
for (const character of text) loop += character;

delete Object.prototype.__stringIteratorNoise;
if (spread !== "A|😀|B" || loop !== text) {
  throw new Error(`canonical string iteration changed: ${spread} / ${loop}`);
}

String.prototype[Symbol.iterator] = function () {
  let done = false;
  return {
    next() {
      if (done) return { done: true };
      done = true;
      return { done: false, value: "override" };
    },
  };
};
const savedIterator = originalIterator.call(text);
let savedValues = "";
for (let step = savedIterator.next(); !step.done; step = savedIterator.next()) {
  savedValues += step.value;
}
const overridden = [..."ignored"].join(",");
let overriddenLoop = "";
for (const character of "ignored") overriddenLoop += character;
String.prototype[Symbol.iterator] = function () {
  return originalIterator.call(this);
};
const delegated = [...text].join("|");
String.prototype[Symbol.iterator] = originalIterator;

if (savedValues !== text || overridden !== "override" || overriddenLoop !== "override" ||
  delegated !== "A|😀|B") {
  throw new Error(
    `String iterator override/delegation was bypassed: ${savedValues} / ${overridden} / ${overriddenLoop} / ${delegated}`,
  );
}
console.log("string iterator survives unrelated builtin mutations and respects overrides");
