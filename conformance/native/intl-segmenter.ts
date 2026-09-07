// Intl.Segmenter's JavaScript object model is compiled source in the native runtime; only Unicode
// boundary discovery crosses into the provider. Exercise UTF-16 indices, iteration, containing,
// and the word-only isWordLike property across that seam.
const graphemes = new Intl.Segmenter("en-US").segment("e\u0301x😀");
console.log(Array.from(graphemes, (part) => `${part.index}:${part.segment}`).join("|"));
console.log(graphemes.containing(0).segment, graphemes.containing(4).segment);

const words = new Intl.Segmenter("en-US", { granularity: "word" }).segment("Hi, 42!");
console.log(Array.from(words, (part) => `${part.segment}:${part.isWordLike}`).join("|"));
console.log(new Intl.Segmenter(undefined, { granularity: "sentence" }).resolvedOptions());
