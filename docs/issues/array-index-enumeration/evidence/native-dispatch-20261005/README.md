# Native array enumeration dispatch evidence

This evidence exercises the native `Object.keys`, `Object.values`, and `Object.entries` dispatch,
including current-descriptor rechecks. The fixture snapshots indices `0`, `1`, and `2`; reading
index `0` makes index `1` enumerable and deletes index `2`. The expected Node output is retained in
`node.log`. The native output bytes in `native-parity.log` and `native-stress.log` match Node.

The dispatch change is in `dispatch_part_a.c`: `Object.keys` delegates to the existing own-key
enumeration path. `Object.values` and `Object.entries` snapshot all own keys, then inspect the
current property-store descriptor or dense element slot before each `Get`. Plain arrays with no
property-store overlay or indexed descriptors retain an allocation-light direct walk; holes are
skipped. The metadata path avoids allocating a descriptor object for every dense array element.

The two-file `scripts/native.sh` run reports 1 match and 1 divergence because the interpreter on
this frozen CLI returns stale array values and entries without invoking the index-0 getter. The
native bytes are the Node-correct values (`zero`, `one`) and entries (`0/zero`, `1/one`); this is
not evidence of a native mismatch. The root is fixing the interpreter dispatch separately. The
existing `array-index-enumeration.js` probe matched. The stressed run reports the same counts and
0 stress skips; native output remains identical to Node.

The CLI was built from base commit `ecfc065baccc5151417ad3d982ca92bd4003dde0` plus the uncommitted
source diff whose SHA-256 was `768ceed8eb464ddde1a621bb0c98a4d98bf4687ecb5bf25e79efdbb3bad1e4b6`. Rust
1.97.1 was pinned with `RUSTUP_TOOLCHAIN=1.97.1`; the build used one MBX/Cargo/Rayon worker and the
shared cache/target roots documented in the parent session. CLI SHA-256: `c4c7bb8707559c8a0bd352a1ba154b420f052c696bc61c13814252be2564d905`. The provider manifest is the frozen
`4b68a2200f148e19ff7fcd146cacd5c8ffb546692d67e52478fb3b266c19621b` manifest.

Raw logs:

- `node.log`: 4 lines, expected output.
- `native-parity.log`: ordinary native run, 1 match / 1 interpreter divergence.
- `native-stress.log`: `KILN_NATIVE_GC_STRESS=1`, 1 match / 1 interpreter divergence / 0 skipped stress probes.

The build used:

```sh
PATH=~/.local/share/mise/installs/rust/1.97.1:~/.local/share/mise/installs/mr-boxington/1.22.0/.mise-bins:/usr/bin:/bin
RUSTUP_TOOLCHAIN=1.97.1 MBX_GC_AUTO=false
MBX_CACHE_DIR=~/Work/kiln/.scratch/effect-sandbox-integration/.mbx-cache
MBX_TARGET_ROOT=~/Work/kiln/.scratch/effect-sandbox-integration/.mbx-targets
MBX_SCHEDULER_CPUS=3 CARGO_BUILD_JOBS=1 KILN_JOBS=1 RAYON_NUM_THREADS=1
mbx build -p kiln-cli --offline
```

Both native runs executed `scripts/native.sh` directly with absolute `KILN_BIN` pointing to the
CLI above and `KILN_NATIVE_PROVIDER_MANIFEST` pointing to the frozen `4b68` manifest. Their probe
arguments were:

```text
conformance/native/array-index-enumeration.js
conformance/native/array-enumeration-descriptor-snapshot.js
```

The stress run added `KILN_NATIVE_GC_STRESS=1`. The Node reference was `node
conformance/native/array-enumeration-descriptor-snapshot.js`.

The earlier low-level `array_index_enumeration_merges_descriptor_overlays_once` backend unit test
passed 1/1 before the final dispatch edit; it exercises the own-key helper, not the application
dispatch, so it is not used as dispatch validation. The dispatch itself was exercised by both native
runs above.
