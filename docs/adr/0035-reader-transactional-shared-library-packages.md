# ADR 0035: Shared-library packages publish reader-transactional generations

## Status

Accepted.

## Context

ADR 0009 publishes a canonical dynamic-loader file, a generated header, and a requested output
alias. Those files are useful compatibility paths, but replacing them independently lets an
embedding host observe a header from one build with a library from another build. A library path
is therefore not a coherent readiness signal for a reader that needs both declarations and code.

## Decision

Every shared-library output also has a package beside it:

```text
<output-parent>/.kiln-packages/<output-leaf>/
  CURRENT
  generations/<sha256-generation>/
    manifest.json
    header.h
    <canonical-loader-leaf>
```

Writers keep the existing process and output-directory advisory lock. While holding it, they
install the immutable canonical loader, create and validate a complete generation, and update the
legacy header and output aliases. They then atomically replace `CURRENT`; that replacement is the
linearization point for readers. Existing generations are never deleted by publication.

The manifest is strict canonical JSON with no unknown fields. Its format tag, header and library
SHA-256 digests, canonical loader leaf, legacy entry symbol, and ABI-v2 accessor symbol define the
generation identity. Every generation file is a regular file and every package directory is a
real directory; symlinks and special files are rejected. A reader reads one `CURRENT`, validates
the selected generation and manifest, and verifies both content digests before returning paths.
Because generations are immutable and retained, a reader can safely use the returned header and
loader paths after another writer publishes a later generation.

The public Rust API is `shared_publication::read_shared_library_package` (and
`SharedLibraryPackage::read`) together with `shared_library_package_path`. The existing native
report fields, protocol fields, cache keys, and legacy aliases remain unchanged.

## Consequences

- Embedders can select one validated header/loader pair without coordinating with a writer.
- A malformed pointer, manifest, generation, symlink, or digest is a hard reader error rather than
  a best-effort fallback.
- Compatibility consumers keep using the requested output/header paths while new readers use the
  package generation.
- Retained generations require a future explicit package garbage-collection policy.
