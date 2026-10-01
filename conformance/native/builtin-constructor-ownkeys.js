for (const constructor of [Boolean, BigInt, Number, String, Symbol]) {
  console.log(constructor.name, Reflect.ownKeys(constructor).map(String).join(","));
}

Object.defineProperty(Boolean, "length", { value: "overridden" });
console.log(Reflect.ownKeys(Boolean).map(String).join(","));

Boolean.extra = 1;
console.log(Reflect.ownKeys(Boolean).map(String).join(","));
delete Boolean.extra;
console.log(Reflect.ownKeys(Boolean).map(String).join(","));
