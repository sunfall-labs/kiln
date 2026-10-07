# 04. `kiln build` on an unbundled project

Status: ready-for-agent

Users do not pre-bundle with esbuild. `kiln build src/server.ts` runs in a project directory with a
`package.json` and a `node_modules` tree, and must resolve the way Node does:

- `exports` and `imports` maps with conditions (`node`, `import`, `require`, `default`), in Node's
  order;
- CommonJS and ESM mixed across packages, including `require` of ESM where Node allows it, and
  named imports from CommonJS through Node's static detection;
- JSON imports, `__dirname` / `__filename` / `import.meta.dirname`, package self-reference;
- `pnpm`'s symlinked layout as well as npm's flat one;
- files a package reads at run time relative to itself (templates, `.wasm`, `.json` data), which a
  closed-world build has to either embed or refuse, by name.

`NodePackageResolver` in `kiln-module-resolver` and `kiln-cli/src/sources.rs` is the starting
point. Measure first: build every tier 1 app unbundled and list what fails, before changing
anything.

## Acceptance

- Every tier 1 app builds from its project directory with no bundler step.
- A table in this file: each resolution feature above, with the probe that proves it in
  `conformance/apps` or `conformance/node`.
