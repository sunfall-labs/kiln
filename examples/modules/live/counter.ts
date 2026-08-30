// A binding an exported function changes. The importer must see the new value, not a copy of the
// one this module finished with.
export let count = 0;
// A `const` cannot change, so it is not boxed -- which is what keeps a module of these compilable
// by the scalars-only native backend.
export const limit = 10;

export function bump(): void {
  count += 1;
}
