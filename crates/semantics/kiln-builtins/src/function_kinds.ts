/* Callable constructors and their prototype identities are semantic builtins. The private
 * bridge only associates these TypeScript-created objects with native closure kinds. */
(function installFunctionKinds() {
  const installFunctionKindPrototype = (globalThis as any).__kilnFunctionKindPrototype;
  const constructFunctionKind = (globalThis as any).__kilnFunctionKindConstructor;
  delete (globalThis as any).__kilnFunctionKindPrototype;
  delete (globalThis as any).__kilnFunctionKindConstructor;

  if (typeof installFunctionKindPrototype === "function") {
    function AsyncFunction(...args: any[]): any {
      return constructFunctionKind(0, ...args);
    }
    function GeneratorFunction(...args: any[]): any {
      return constructFunctionKind(1, ...args);
    }
    function AsyncGeneratorFunction(...args: any[]): any {
      return constructFunctionKind(2, ...args);
    }

    const constructors = [AsyncFunction, GeneratorFunction, AsyncGeneratorFunction];
    const prototypes = [
      Object.create(Function.prototype),
      Object.create(Function.prototype),
      Object.create(Function.prototype),
    ];
    const tags = ["AsyncFunction", "GeneratorFunction", "AsyncGeneratorFunction"];
    for (let kind = 0; kind < constructors.length; kind += 1) {
      const constructor = constructors[kind];
      const prototype = prototypes[kind];
      Object.setPrototypeOf(constructor, Function);
      Object.setPrototypeOf(prototype, Function.prototype);
      Object.defineProperty(prototype, "constructor", {
        value: constructor,
        writable: true,
        configurable: true,
      });
      Object.defineProperty(prototype, Symbol.toStringTag, {
        value: tags[kind],
        configurable: true,
      });
      Object.defineProperty(constructor, "prototype", {
        value: prototype,
        writable: false,
        configurable: false,
      });
      Object.defineProperty(constructor, "length", {
        value: 1,
        writable: false,
        configurable: true,
      });
      installFunctionKindPrototype(kind, prototype);
    }
  }
})();
