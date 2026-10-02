// Realm-local facade over an immutable graph owned by the shared graph arena.
// The engine's hooks are on the realm's engine-private object, which is readable only while this
// initializer runs, and only when kiln's extensions are installed. Capture them now.
const enginePrivate: any = ((): any => { const G = globalThis as any; return G.__kilnPrivate; })();
if (enginePrivate?.extensions !== true) {
  throw new TypeError("kiln:shared requires enabled Kiln extensions");
}
const nativeShare: (value: any) => any = enginePrivate.share;
const nativeIsShared: (value: any) => boolean = enginePrivate.sharedIs;

declare const sharedValueBrand: unique symbol;
export type Shared<T> = T extends object
  ? { readonly [K in keyof T]: Shared<T[K]> } & { readonly [sharedValueBrand]: true }
  : T;

/** A frozen copy of a plain data graph that every realm -- workers, actors -- reads without a
 *  copy. Sharing a shared value answers it; functions, accessors and class instances refuse. */
export function share<T>(value: T): Shared<T> {
  return nativeShare(value);
}

/** Whether `value` is a graph `share` made. */
export function isShared(value: unknown): boolean {
  return nativeIsShared(value);
}

export default { share, isShared };
