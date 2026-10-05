# `Temporal.Now` replaceable descriptor evidence

The interpreter exposed `Temporal.Now` as a non-writable, non-configurable builtin. Node 26.8.1
reports its own descriptor as writable, non-enumerable and configurable. The interpreter metadata
fix is commit `0f09e1d274de920203f105a10a98896894392ef7`; its focused Rust regression and the Atom
fake-timer setup run passed. Atom was 124/125 after that fix, with only its unrelated late
finalization case still failing.

The native runtime had two corresponding gaps: descriptor reflection did not recognize the
custom-read `Temporal.Now` namespace member, and a partial `defineProperty` treated it as a new
property instead of merging against its virtual builtin attributes. The native fixture
`conformance/native/temporal-now-descriptor.js` checks the reported flags, replaces the value with
`{ value: replacement }`, and restores the original descriptor and identity.

Validation used Node `v26.8.1`, provider manifest SHA256
`5eaa7d77a4e17c0d376c48b773a08870de12a0768fb3c3d69f3129fdb6e5fd2e`, and release CLI SHA256
`0a0b638392a50a2aa4c1c2e1c671e8a09746fd577260b162d20d55087078a771`. The CLI was built with:

```sh
CARGO_BUILD_JOBS=1 KILN_JOBS=1 RAYON_NUM_THREADS=1 MBX_GC_AUTO=false mise exec -- mbx build --jobs=1 -p kiln-cli --release
```

Native execution used the pinned provider and one-shot mode:

```sh
KILN_NATIVE_PROVIDER_MANIFEST=~/Work/kiln-effect-next-milestone/.scratch/allocator-full-gui-provider-http-fixed/kiln-native-provider.full-gui.json \
  target/release/kiln --one-shot run --native conformance/native/temporal-now-descriptor.js
```

Output: `Temporal.Now descriptor replacement and restoration passed`. Node ran the identical
fixture successfully. Before the merge fix, the native probe reported the right descriptor flags
but failed restoring the captured descriptor with `Cannot redefine property: Now`.
