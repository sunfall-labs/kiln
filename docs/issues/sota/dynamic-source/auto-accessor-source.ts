class Distinct {
  accessor "\ud800" = 11;
  accessor "\ufffd" = 22;
}
const value = new Distinct();
console.log(value["\ud800"], value["\ufffd"]);
value["\ud800"] = 101;
value["\ufffd"] = 202;
console.log(value["\ud800"], value["\ufffd"]);
