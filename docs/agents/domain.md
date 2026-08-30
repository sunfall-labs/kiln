# Domain documentation

Kiln is a single-context repository. Engineering skills consume its domain documentation using the
rules below.

## Before exploring

- Read `CONTEXT.md` at the repository root when it exists.
- Read the decisions under `docs/adr/` that touch the area being explored.
- If `CONTEXT.md` does not exist, proceed silently. A producer skill creates it when domain terms
  are actually resolved.

## Use the glossary's vocabulary

When an issue, proposal, hypothesis or test names a domain concept, use the term defined in
`CONTEXT.md`. Do not drift to synonyms the glossary explicitly avoids.

If a needed concept is absent, first reconsider whether the proposal is inventing language the
project does not use. If the concept is real, note the gap so the domain documentation can be
updated when the term is resolved.

## Flag ADR conflicts

If work contradicts an existing ADR, surface the conflict explicitly rather than silently
overriding the decision. Name the ADR and explain why reopening it may be justified.
