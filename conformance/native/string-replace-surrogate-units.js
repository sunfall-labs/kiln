// String.prototype.replace must preserve exact UTF-16 units in unchanged slices and templates.
const high = String.fromCharCode(0xD800);
const low = String.fromCharCode(0xDC00);
const units = (value) => Array.from({ length: value.length }, (_, index) => value.charCodeAt(index));
const cases = [
  ["literal-no-match-high", `a${high}b`.replace("missing", "x")],
  ["literal-no-match-low", `a${low}b`.replace("missing", "x")],
  ["literal-match-suffix", `x${high}y`.replace("x", "")],
  ["literal-match-prefix", `${low}xy`.replace("y", "")],
  ["literal-match-lone-needle", `${high}x`.replace(high, "!")],
  ["literal-lone-replacement", "x".replace("x", low)],
  ["literal-template-suffix", `x${high}y`.replace("x", "$'")],
  ["regex-no-match-high", `a${high}b`.replace(/missing/g, "x")],
  ["regex-match-suffix", `x${low}y`.replace(/x/g, "")],
  ["regex-lone-replacement", "x".replace(/x/, high)],
  ["regex-lone-capture", high.replace(/(.)/, "$1")],
  ["regex-named-lone-capture", high.replace(/(?<unit>.)/, "$<unit>")],
  ["regex-function-replacement", high.replace(/(.)/, (matched) => matched)],
  ["json-pointer-token", high.replace(/~/g, "~0").replace(/\//g, "~1")],
];
console.log(JSON.stringify(cases.map(([name, value]) => [name, units(value)])));
