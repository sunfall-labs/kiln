function nested() {
  try {
    try {
      throw 1;
    } catch (e) {
      throw e + 1;
    } finally {
      console.log("inner-finally");
    }
  } catch (e) {
    return "outer:" + e;
  }
}
console.log(nested());
