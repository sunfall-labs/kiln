# PRD: String.prototype.normalize Unicode Composition / Decomposition

Status: wontfix
Branch: `none`

## Problem Statement

`String.prototype.normalize([form])` validates the form argument (`NFC`, `NFD`, `NFKC`, `NFKD`) but returns the input string unchanged without applying canonical or compatibility decompositions.

## Rationale for Wontfix

Full Unicode normalization tables require substantial static data dependencies in both Rust and C native backends. Bundled JS and modern toolchains ship pre-normalized code. Partial normalization is worse than a clear gap, and adding multi-megabyte tables to the prelude binary is deferred until intl data architecture is finalized.
