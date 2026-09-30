/* Callable constructors and their prototype identities are semantic builtins. The private
 * bridge only associates these TypeScript-created objects with native closure kinds. */
const installFunctionKindPrototype = (globalThis as any).__kilnFunctionKindPrototype;
delete (globalThis as any).__kilnFunctionKindPrototype;

if (typeof installFunctionKindPrototype === "function") {
  function AsyncFunction(): never {
    throw new TypeError("AsyncFunction constructor is not implemented");
  }
  function GeneratorFunction(): never {
    throw new TypeError("GeneratorFunction constructor is not implemented");
  }
  function AsyncGeneratorFunction(): never {
    throw new TypeError("AsyncGeneratorFunction constructor is not implemented");
  }

  const constructors = [AsyncFunction, GeneratorFunction, AsyncGeneratorFunction];
  for (let kind = 0; kind < constructors.length; kind += 1) {
    const constructor = constructors[kind];
    const prototype = Object.create(Function.prototype);
    Object.defineProperty(prototype, "constructor", {
      value: constructor,
      writable: true,
      configurable: true,
    });
    Object.defineProperty(constructor, "prototype", {
      value: prototype,
      writable: false,
      configurable: false,
    });
    installFunctionKindPrototype(kind, prototype);
  }
}
