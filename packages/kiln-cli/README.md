# kiln through npm

The package launches kiln's native executable. TypeScript and JavaScript programs run in kiln;
Node or Bun only runs the small launcher.

```sh
npx @sunfall/kiln@0.1.1 run app.ts
bunx @sunfall/kiln@0.1.1 run app.ts
# On a Bun-only machine:
bunx --bun @sunfall/kiln@0.1.1 run app.ts
```

To keep a project on a fixed version:

```sh
npm install --save-dev --save-exact @sunfall/kiln@0.1.1
npx kiln check app.ts
npx kiln build app.ts --output app
```

The platform package contains `bin/kiln` and its complete `lib/kiln/` native-provider directory.
There are no install scripts or runtime downloads. Keep optional dependencies enabled.
Native compilation needs a C compiler, but no Rust toolchain. Running in the interpreter needs
neither compiler. Programs run with the user's authority; kiln is not a sandbox.

Version 0.1.1 packages Linux x86_64 with glibc (Ubuntu 22.04 or newer). The macOS arm64 build is
pending. Linux arm64, musl Linux, Intel macOS and Windows do not have binary packages yet.
The matching macOS dependency is reserved as optional, so fresh installs of the same launcher
version can use it once the pending platform package is published.
Release archives, checksums and security details: https://github.com/sunfall-labs/kiln/releases.

## Maintainers

From a clean release commit, build the archive using `scripts/package-release.sh dist`, then:

```sh
python3 scripts/package-npm.py --output dist/npm dist/kiln-0.1.1-x86_64-unknown-linux-gnu.tar.gz
npm publish dist/npm/kiln-linux-x64 --access public
npm publish dist/npm/kiln --access public
```

Publish all generated platform packages before the launcher. The generator checks archive
versions and checksums, retains the full native-provider directory, and pins every available
platform dependency to the same version. The pending macOS dependency stays in the launcher
manifest. Pass both archives when the macOS build is available.

If 0.1.1's launcher has already shipped when the Mac archive becomes available, generate the
Mac package from that exact tag and publish only `@sunfall/kiln-darwin-arm64@0.1.1`. Do not
republish the immutable launcher version; its matching optional dependency is already reserved.

Run the distribution checks before publishing:

```sh
node --test packages/kiln-cli/test/launcher.test.cjs
python3 scripts/test-package-npm.py
```
