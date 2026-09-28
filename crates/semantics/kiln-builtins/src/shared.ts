// Realm-local facade over an immutable graph owned by the shared graph arena.
// The engine exposes these hooks to reads from this initializer only. Capture them now; no
// property is installed on the global object, including during a later dynamic import.
const G = globalThis as any;
const nativeShare: (value: any) => any = G.__kilnShare;
const nativeIsShared: (value: any) => boolean = G.__kilnSharedIs;

declare const sharedValueBrand: unique symbol;
export type Shared<T> = T extends object
  ? { readonly [K in keyof T]: Shared<T[K]> } & { readonly [sharedValueBrand]: true }
  : T;

if (typeof nativeShare !== "function" || typeof nativeIsShared !== "function") {
  throw new Error("kiln:shared requires enabled Kiln extensions");
}

export function share<T>(value: T): Shared<T> {
  return nativeShare(value);
}

export function isShared(value: unknown): boolean {
  return nativeIsShared(value);
}

export default { share, isShared };
