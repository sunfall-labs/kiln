// Auto-accessor backing identity is structural. A lone high surrogate and the replacement
// character have different public keys even though lossy display text would make both "�".
class Distinct {
	accessor "\ud800" = 11;
	accessor "\ufffd" = 22;
}

const value = new Distinct();
console.log(value["\ud800"], value["\ufffd"]);
value["\ud800"] = 101;
value["\ufffd"] = 202;
console.log(value["\ud800"], value["\ufffd"]);
