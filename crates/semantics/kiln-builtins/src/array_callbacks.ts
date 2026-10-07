// The callback-taking Array methods are ordinary shared TypeScript built-ins. In particular,
// callbacks are called from the VM's normal instruction loop instead of recursively re-entering
// the interpreter from a Rust method implementation.
//
// `ArraySpeciesCreate` is the one internal fact TypeScript cannot derive without observing
// user code: GetFunctionRealm on a constructable value, followed by identity comparison with
// that realm's intrinsic Array constructor. The private predicate implements only that metadata
// operation. Constructor/@@species reads and construction remain visible here in specification
// order.
(function installArrayCallbackMethods() {
  const global = globalThis as any;
  const isConstructor: (value: any) => boolean = global.__kilnIsConstructor;
  const isOtherRealmArrayConstructor: (value: any) => boolean =
    global.__kilnIsOtherRealmArrayConstructor;
  const nextPresentArrayIndex: (
    value: any,
    cursor: number,
    limit: number,
    reverse: boolean,
  ) => number | null | false = global.__kilnArrayNextPresentIndex;
  delete global.__kilnIsConstructor;
  delete global.__kilnIsOtherRealmArrayConstructor;
  delete global.__kilnArrayNextPresentIndex;

  const ArrayCtor = Array;
  const arrayIsArray = ArrayCtor.isArray;
  const ObjectCtor = Object;
  const defineProperty = ObjectCtor.defineProperty;
  const TypeErrorCtor = TypeError;
  const ReflectCtor = Reflect;
  const construct = ReflectCtor.construct;
  const speciesKey = Symbol.species;
  const safeCall: any = (Function.call as any).bind(Function.call);
  const maxSafeInteger = 9007199254740991;
  const mathMin = Math.min;
  const mathTrunc = Math.trunc;

  function toObject(receiver: any): any {
    if (receiver === null || receiver === undefined) {
      throw new TypeErrorCtor("Array method called on null or undefined");
    }
    return ObjectCtor(receiver);
  }

  // ToLength uses ToNumber, which rejects BigInt and Symbol. Unary plus has those semantics;
  // Number(value) would incorrectly accept BigInt.
  function toLength(value: any): number {
    const number = +value;
    if (number !== number || number <= 0) return 0;
    if (number === Infinity) return maxSafeInteger;
    return mathMin(mathTrunc(number), maxSafeInteger);
  }

  function createArray(length: number): any[] {
    // The captured intrinsic performs ArrayCreate's length validation and starts with holes.
    return new ArrayCtor(length);
  }

  function arraySpeciesCreate(original: any, length: number): any {
    if (!arrayIsArray(original)) return createArray(length);

    let constructor = original.constructor;
    if (isConstructor(constructor) && isOtherRealmArrayConstructor(constructor)) {
      constructor = undefined;
    }
    if (constructor !== null &&
        (typeof constructor === "object" || typeof constructor === "function")) {
      constructor = constructor[speciesKey];
      if (constructor === null) constructor = undefined;
    }
    if (constructor === undefined) return createArray(length);
    if (!isConstructor(constructor)) {
      throw new TypeErrorCtor("Array species is not a constructor");
    }
    return construct(constructor, [length]);
  }

  function requireCallback(callback: any, name: string): void {
    if (typeof callback !== "function") {
      throw new TypeErrorCtor(`Array.prototype.${name} callback is not a function`);
    }
  }

  // The private query searches the runtime's dense array slots without spending one VM step per
  // hole. `false` means the receiver/prototype chain needs ordinary HasProperty (notably a
  // Proxy); `null` means no indexed property remains. It is re-run after each callback so a
  // callback that adds or deletes a future index is observed in the usual order.
  function nextIndex(object: any, cursor: number, limit: number, reverse = false): number | null | false {
    return nextPresentArrayIndex(object, cursor, limit, reverse);
  }

  const map = {
    map(this: any, callback: any, thisArg: any = undefined): any {
      const object = toObject(this);
      const length = toLength(object.length);
      requireCallback(callback, "map");
      const result = arraySpeciesCreate(object, length);
      // Object.defineProperty converts this private input into a fresh internal descriptor
      // snapshot before dispatching to any proxy trap. Reuse one descriptor within this invocation
      // without exposing its identity or later value updates to user code.
      let dataDescriptor: PropertyDescriptor | undefined;
      let cursor = 0;
      while (cursor < length) {
        const candidate = nextIndex(object, cursor, length);
        if (candidate === false) {
          for (; cursor < length; cursor += 1) {
            if (!(cursor in object)) continue;
            const value = object[cursor];
            const mapped = safeCall(callback, thisArg, value, cursor, object);
            if (dataDescriptor === undefined) {
              dataDescriptor = { value: mapped, writable: true, enumerable: true, configurable: true };
            } else dataDescriptor.value = mapped;
            defineProperty(result, cursor, dataDescriptor);
            dataDescriptor.value = undefined;
          }
          break;
        }
        if (candidate === null) break;
        const index = candidate;
        cursor = index + 1;
        {
          const value = object[index];
          const mapped = safeCall(callback, thisArg, value, index, object);
          if (dataDescriptor === undefined) {
            dataDescriptor = {
              value: mapped,
              writable: true,
              enumerable: true,
              configurable: true,
            };
          } else {
            dataDescriptor.value = mapped;
          }
          // DefineProperty performs the spec's numeric ToPropertyKey conversion internally.
          defineProperty(result, index, dataDescriptor);
          // A trap may accept a definition without storing its value, so don't keep a mapped
          // result alive as the next callback begins.
          dataDescriptor.value = undefined;
        }
      }
      return result;
    },
  }.map;

  const reduce = {
    reduce(this: any, callback: any, initialValue: any = undefined): any {
      const object = toObject(this);
      const length = toLength(object.length);
      requireCallback(callback, "reduce");

      let index = 0;
      let accumulator: any;
      if (arguments.length > 1) {
        accumulator = initialValue;
      } else {
        const first = nextIndex(object, 0, length);
        if (first === false) {
          while (index < length && !(index in object)) index += 1;
        } else if (first !== null) {
          index = first;
        } else {
          index = length;
        }
        if (index >= length) {
          throw new TypeErrorCtor("Reduce of empty array with no initial value");
        }
        accumulator = object[index];
        index += 1;
      }

      while (index < length) {
        const candidate = nextIndex(object, index, length);
        if (candidate === false) {
          for (; index < length; index += 1) {
            if (index in object) {
              accumulator = safeCall(callback, undefined, accumulator, object[index], index, object);
            }
          }
          break;
        }
        if (candidate === null) break;
        index = candidate;
        {
          accumulator = safeCall(callback, undefined, accumulator, object[index], index, object);
        }
        index += 1;
      }
      return accumulator;
    },
  }.reduce;

  const reduceRight = {
    reduceRight(this: any, callback: any, initialValue: any = undefined): any {
      const object = toObject(this);
      const length = toLength(object.length);
      requireCallback(callback, "reduceRight");

      let index = length - 1;
      let accumulator: any;
      if (arguments.length > 1) {
        accumulator = initialValue;
      } else {
        const first = nextIndex(object, index, -1, true);
        if (first === false) {
          while (index >= 0 && !(index in object)) index -= 1;
        } else if (first !== null) {
          index = first;
        } else {
          index = -1;
        }
        if (index < 0) {
          throw new TypeErrorCtor("Reduce of empty array with no initial value");
        }
        accumulator = object[index];
        index -= 1;
      }

      while (index >= 0) {
        const candidate = nextIndex(object, index, -1, true);
        if (candidate === false) {
          for (; index >= 0; index -= 1) {
            if (index in object) {
              accumulator = safeCall(callback, undefined, accumulator, object[index], index, object);
            }
          }
          break;
        }
        if (candidate === null) break;
        index = candidate;
        {
          accumulator = safeCall(callback, undefined, accumulator, object[index], index, object);
        }
        index -= 1;
      }
      return accumulator;
    },
  }.reduceRight;

  const methodDescriptor: PropertyDescriptor = {
    writable: true,
    enumerable: false,
    configurable: true,
  };
  methodDescriptor.value = map;
  defineProperty(ArrayCtor.prototype, "map", methodDescriptor);
  methodDescriptor.value = reduce;
  defineProperty(ArrayCtor.prototype, "reduce", methodDescriptor);
  methodDescriptor.value = reduceRight;
  defineProperty(ArrayCtor.prototype, "reduceRight", methodDescriptor);
})();
