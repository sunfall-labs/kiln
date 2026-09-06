# Interpreter Template Object Caching

Status: resolved

## Comments

Resolved in `kiln-interpreter`: template objects cached per realm, unit, and site. Constructs frozen cooked array and raw array with non-enumerable, non-writable, non-configurable `raw` property descriptor. Preserves element enumerability on frozen arrays.

## Goal

Implement `Instruction::LoadTemplateObject(site_index)` in `kiln-interpreter` with per-site lazy caching across evaluations.

## Details

1. In `crates/kiln-interpreter/`:
   - Store initialized template objects per realm and site index: e.g. `template_objects: HashMap<(RealmId, UnitId, u32), Value>` or a per-unit vector of cached `Option<Value>`.
   - On handling `Instruction::LoadTemplateObject(site_index)`:
     - Check if the site is already cached. If so, push the cached `Value` onto the operand stack.
     - If not, build the `cooked` array and `raw` array from the `TemplateSite` metadata.
     - Set `cooked.raw = raw` (non-enumerable, non-writable, non-configurable per ES §13.2.8.5).
     - Freeze both arrays using `Object.freeze`.
     - Store the resulting `Value` in the cache and push it onto the operand stack.
2. Ensure identical object reference (`===`) is returned on all subsequent evaluations across calls, closures, and loops.
