// Dynamic Function string literals use the same escape and UTF-16 rules as ordinary source.
const makeValues = Function(String.raw`return [
  "\u2028", "\u2029", "\u{0000000000000041}", "\u{1F600}",
  "\uD83D\uDE00", "\uD800", "\x41", "\b\f\n\r\t\v\0"
]`);
const values = makeValues();
const units = value => Array.from({ length: value.length }, (_, index) =>
  value.charCodeAt(index).toString(16)).join(",");

const line = "\u2028";
const paragraph = "\u2029";
const rawSeparators = [
  Function("return \"a" + line + "b\"")(),
  Function("return \"a" + paragraph + "b\"")(),
];
const rawLoneSurrogate = String.fromCharCode(0xD800);
const rawSourceValue = Function('return "a' + rawLoneSurrogate + 'b"')();
const rawSourceSurrogate = [rawSourceValue.length, units(rawSourceValue)];
const escapedRawSourceSurrogate = Function('return "\\' + rawLoneSurrogate + '"')();
const escapedRawEuro = Function('return "\\' + String.fromCharCode(0x20AC) + '"')();
const rawSourceRegexp = Function('return /a' + rawLoneSurrogate + 'b/;')();
const nestedFunctionSurrogate = Function(
  'return function(){ return "' + rawLoneSurrogate + '"; }',
)()();
const nestedFunctionSource = Function(
  'return function(){ return "' + rawLoneSurrogate + '"; }',
)().toString();
const reflectedDynamicConstructor = Function('x', 'return "' + rawLoneSurrogate + '";');
const reflectedDynamicExpression = Function(
  'return function(a){return "' + rawLoneSurrogate + '";}',
)();
const reflectedDynamicArrow = Function(
  'return (a)=>"' + rawLoneSurrogate + '";',
)();
const reflectedDynamicMethod = Function(
  'return ({m(a){return "' + rawLoneSurrogate + '";}}).m',
)();
const dynamicReflection = [
  reflectedDynamicConstructor.toString(),
  reflectedDynamicExpression.toString(),
  reflectedDynamicArrow.toString(),
  reflectedDynamicMethod.toString(),
];
const expectedDynamicReflection = [
  'function anonymous(x\n) {\nreturn "' + rawLoneSurrogate + '";\n}',
  'function(a){return "' + rawLoneSurrogate + '";}',
  '(a)=>"' + rawLoneSurrogate + '"',
  'm(a){return "' + rawLoneSurrogate + '";}',
];
if (dynamicReflection.some((source, index) => source !== expectedDynamicReflection[index])) {
  throw new Error('dynamic Function#toString must preserve complete source and UTF-16 units');
}
const methodSurrogate = Function(
  'return ({ method(){ return "' + rawLoneSurrogate + '"; } }).method()',
)();
const arrowSurrogate = Function(
  'return (() => "' + rawLoneSurrogate + '")()',
)();
const escapedSeparators = [
  Function("return \"a\\" + line + "b\"")(),
  Function("return \"a\\" + paragraph + "b\"")(),
];
const continued = Function("return \"a\\" + "\n" + "b\"")();
const template = Function("return `a\nb`")();
const named = Function(String.raw`return ({ "\uD800"() { return 1; } })`)()["\uD800"];

const syntaxErrors = [
  String.raw`return "\xZ"`,
  String.raw`return "\u{110000}"`,
  String.raw`"use strict"; return "\141";`,
].map(source => {
  try {
    Function(source);
    return false;
  } catch (error) {
    return error instanceof SyntaxError;
  }
});

console.log(JSON.stringify({
  units: values.map(units),
  rawSeparators: rawSeparators.map(value => [value.length, units(value)]),
  rawSourceSurrogate,
  escapedRawSourceSurrogate: units(escapedRawSourceSurrogate),
  escapedRawEuro: units(escapedRawEuro),
  rawSourceRegexp: [units(rawSourceRegexp.source), rawSourceRegexp.test(rawSourceValue)],
  nestedFunctionSurrogate: units(nestedFunctionSurrogate),
  nestedFunctionSource: units(nestedFunctionSource),
  dynamicReflection: dynamicReflection.map(units),
  methodSurrogate: units(methodSurrogate),
  arrowSurrogate: units(arrowSurrogate),
  escapedSeparators,
  continued: units(continued),
  template: units(template),
  named: [units(named.name), named()],
  sloppyOctal: units(Function(String.raw`return "\141"`)()),
  syntaxErrors,
}));
