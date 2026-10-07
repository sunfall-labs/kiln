const object = Function(`return {
  "fn": function (value) { return value + 1; },
  "inverse": function () { return "empty"; },
  "é": "unicode",
  "méthod"() { return this["é"]; },
  "": 7
};`)();

if (object.fn(8) !== 9) throw new Error("quoted callback key");
if (object.inverse() !== "empty") throw new Error("quoted inverse key");
if (object["é"] !== "unicode") throw new Error("UTF-8 quoted key");
if (object.méthod() !== "unicode" || object.méthod.name !== "méthod") {
  throw new Error("UTF-8 concise method key and name");
}
if (object[""] !== 7) throw new Error("empty quoted key");
console.log("quoted keys:", object.fn(8), object.inverse(), object["é"], object[""]);
