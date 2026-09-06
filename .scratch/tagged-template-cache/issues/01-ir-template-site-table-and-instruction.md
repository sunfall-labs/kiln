# IR Template Site Table and `LoadTemplateObject` Instruction

Status: ready-for-agent

## Goal

Define the template site table in `Program` and add the `LoadTemplateObject` instruction to `kiln-ir`.

## Details

1. In `crates/kiln-ir/src/types.rs` (or `program.rs`):
   - Add `TemplateSite` struct holding `cooked: Vec<Option<String>>` and `raw: Vec<String>`.
   - Add `template_sites: Vec<TemplateSite>` to `Program`.
2. In `crates/kiln-ir/src/instruction.rs`:
   - Add `Instruction::LoadTemplateObject(u32)` (where `u32` is the site index).
3. In `crates/kiln-ir/src/compile.rs`:
   - In `compile_tagged_template`:
     - Register the template site into `self.program.template_sites` (or a per-module collector).
     - Emit `Instruction::LoadTemplateObject(site_index)` instead of emitting instructions that construct fresh arrays and freeze them on every evaluation.
