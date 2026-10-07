// ansi-styles uses these UnicodeSets expressions while assembling its color tables.
if (!/[:;]/v.test(";")) throw new Error("v-mode punctuation class did not match");
if (!/[\da-f]{3}/iv.test("f00")) throw new Error("v-mode case-insensitive class did not match");
console.log("UnicodeSets basic classes ok");
