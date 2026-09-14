// String slicing and split coverage across Latin1 and UTF-16-backed values.
// Observations use UTF-16 code units so Latin1 bytes and lone surrogates stay visible.

function units(text) {
  const result = [];
  for (let index = 0; index < text.length; index += 1) result.push(text.charCodeAt(index));
  return result;
}

function sameUnits(actual, expected, message) {
  const got = JSON.stringify(actual.map(units));
  const want = JSON.stringify(expected);
  if (got !== want) throw new Error(message + ": " + got + " != " + want);
}

const latin = "éA\x80BÿA";
sameUnits(latin.split("A"), [[0xe9], [0x80, 0x42, 0xff], []], "Latin1 split");
sameUnits(latin.split(""), [[0xe9], [0x41], [0x80], [0x42], [0xff], [0x41]],
          "Latin1 split into units");
if (JSON.stringify(units(latin.slice(0, 1))) !== "[233]") {
  throw new Error("Latin1 slice");
}

// A separator sliced from a wide value can contain only ASCII units while retaining the
// wide representation. Exercise both mixed-width directions through the same search semantics.
const wideSeparator = String.fromCharCode(0x0100, 0x41, 0x0100).slice(1, 2);
const wideSubject = String.fromCharCode(0x0100, 0x41, 0x0100, 0x41);
sameUnits(wideSubject.split("A"), [[0x0100], [0x0100], []], "wide subject Latin1 separator");
sameUnits(wideSubject.split(wideSeparator), [[0x0100], [0x0100], []],
          "wide subject wide separator");
sameUnits(latin.split(wideSeparator), [[0xe9], [0x80, 0x42, 0xff], []],
          "Latin1 subject wide separator");
const wideComma = String.fromCharCode(0x0100, 0x2c, 0x0100).slice(1, 2);
sameUnits("é,ÿ".split(wideComma), [[0xe9], [0xff]],
          "Latin1 subject wide comma separator");

console.log("string units slices", JSON.stringify(units(latin)), JSON.stringify(wideSubject));
