# Parser Treats .js as Script While Engine Executes as Module

Status: resolved · Severity: medium

## Comments

Fixed 2026-09-05.
Aligned `parse` default goal with runtime module semantics, preserving Script goal for `.cjs`.
