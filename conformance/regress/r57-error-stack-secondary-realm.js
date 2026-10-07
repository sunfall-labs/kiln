if (typeof ShadowRealm === "function") {
  const expectAccessor = typeof Bun === "undefined";
  const realm = new ShadowRealm();
  const realmCheck = realm.evaluate(`(() => {
    const error = new TypeError("secondary realm");
    const descriptor = Object.getOwnPropertyDescriptor(error, "stack");
    return Error.isError(error) && typeof error.stack === "string" &&
      (!${expectAccessor} || (typeof descriptor.get === "function" &&
        Object.getOwnPropertyDescriptor(Error.prototype, "stack") === undefined)) &&
      Object.getPrototypeOf(error) === TypeError.prototype &&
      Object.getPrototypeOf(TypeError.prototype) === Error.prototype;
  })()`);
  if (realmCheck !== true) {
    throw new Error("secondary realm used the wrong Error stack accessors or prototypes");
  }
}
console.log("secondary realm Error accessors passed");
