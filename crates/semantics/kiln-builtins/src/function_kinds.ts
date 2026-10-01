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
        writable: false,
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
    }

    // Generator instances share one intrinsic iterator prototype per kind. Read their native
    // methods from temporary instances so the TypeScript-owned objects reuse the runtime's
    // existing generator callables rather than inventing a second dispatch path.
    const generator = (function* () {})();
    const generatorPrototype = Object.create(Iterator.prototype);
    Object.defineProperties(generatorPrototype, {
      constructor: { value: prototypes[1], writable: false, configurable: true },
      next: { value: generator.next, writable: true, configurable: true },
      return: { value: generator.return, writable: true, configurable: true },
      throw: { value: generator.throw, writable: true, configurable: true },
      [Symbol.toStringTag]: { value: "Generator", configurable: true },
    });

    const asyncGenerator = (async function* () {})();
    const asyncIteratorPrototype = Object.create(Object.prototype);
    Object.defineProperty(asyncIteratorPrototype, Symbol.asyncIterator, {
      value: function () { return this; },
      writable: true,
      configurable: true,
    });
    if (Symbol.asyncDispose !== undefined) {
      Object.defineProperty(asyncIteratorPrototype, Symbol.asyncDispose, {
        value: async function () {
          if (typeof this.return === "function") await this.return();
        },
        writable: true,
        configurable: true,
      });
    }
    const asyncGeneratorPrototype = Object.create(asyncIteratorPrototype);
    Object.defineProperties(asyncGeneratorPrototype, {
      constructor: { value: prototypes[2], writable: false, configurable: true },
      next: { value: asyncGenerator.next, writable: true, configurable: true },
      return: { value: asyncGenerator.return, writable: true, configurable: true },
      throw: { value: asyncGenerator.throw, writable: true, configurable: true },
      [Symbol.toStringTag]: { value: "AsyncGenerator", configurable: true },
    });

    Object.defineProperty(prototypes[1], "prototype", {
      value: generatorPrototype,
      writable: false,
      configurable: true,
    });
    Object.defineProperty(prototypes[2], "prototype", {
      value: asyncGeneratorPrototype,
      writable: false,
      configurable: true,
    });
    for (let kind = 0; kind < prototypes.length; kind += 1) {
      installFunctionKindPrototype(kind, prototypes[kind]);
    }
    installFunctionKindPrototype(3, generatorPrototype);
    installFunctionKindPrototype(4, asyncGeneratorPrototype);
  }
})();
