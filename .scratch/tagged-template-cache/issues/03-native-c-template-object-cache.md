# Native C Template Object Caching and GC Roots

Status: resolved

## Comments

Resolved in `kiln-backend-c`: lowered `LoadTemplateObject` using per-unit static template site cache slots `{symbol}_template_sites`, marked during GC, materialized lazily using `kiln_c99_own_slot` to set frozen `raw` array and freeze cooked array without modifying the frozen prelude core. Program cache hash includes non-empty template sites.

## Goal

Lower `Instruction::LoadTemplateObject(site_index)` in `kiln-backend-c` with static caching and GC root registration.

## Details

1. In `crates/native/kiln-backend-c/`:
   - Emit a static array or static slots for template objects:
     `static kiln_val kiln_template_sites[N];`
   - Include these static pointers in `render_gc_roots` so they are marked during garbage collection.
   - On lowering `LoadTemplateObject(site)`:
     - Check if `kiln_template_sites[site]` is non-zero/valid.
     - If not, invoke runtime helper `kiln_c99_materialize_template_site(unit, site)` which populates and freezes the array and stores it into `kiln_template_sites[site]`.
     - Push `kiln_template_sites[site]`.
2. Ensure native execution matches interpreter output and passes `./scripts/native.sh`.
