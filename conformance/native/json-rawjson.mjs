const huge = JSON.rawJSON("900719925474099312345");
const quoted = JSON.rawJSON('"quoted\\nvalue"');
const carrier = Object.getOwnPropertyDescriptor(huge, "rawJSON");
console.log(JSON.isRawJSON(huge), JSON.stringify(huge));
console.log(JSON.isRawJSON(quoted), JSON.stringify(quoted));
console.log(JSON.stringify({ value: huge }));
console.log(carrier.value, carrier.writable, carrier.enumerable, carrier.configurable);
console.log(Object.getPrototypeOf(huge) === null, Object.isFrozen(huge));
console.log(JSON.isRawJSON({ rawJSON: "123" }));
console.log(JSON.isRawJSON(new Proxy(huge, {})));
for (const source of ["", " ", "1 ", "{}", "[]", "01", "undefined"]) {
  try {
    JSON.rawJSON(source);
    console.log("accepted invalid");
  } catch (error) {
    console.log(error.name);
  }
}
