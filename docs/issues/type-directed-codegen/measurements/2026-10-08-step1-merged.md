# Step 1 merged (work/typed-codegen 852572f3), Linux 7.2.6-1-cachyos

Native corpus on this commit: 668 matched, 5 refused, 0 diverged, both normally and with KILN_SPECULATE=exit (refusals: kiln-extensions-off, kiln-private-channel, native-callable-reflection, regexp-surface-keys, shadow-realm).

load average 1m: 11.58 before timing, 2.24 after; 7 passes per binary, median of the harness's warm_median (json-transform has no .mjs original and reports whole-run time)

| program | untyped .mjs (ms) | typed, types used (ms) | typed, types withheld (ms) | typed/withheld | typed vs untyped | checksum |
| --- | --- | --- | --- | --- | --- | --- |
| nbody | 594.000 | 587.000 | 587.000 | 1.00x | 0.99x | agree |
| mandelbrot | 18.500 | 18.500 | 18.500 | 1.00x | 1.00x | agree |
| class-poly | 51.000 | 51.000 | 51.000 | 1.00x | 1.00x | agree |
| object-literal | 4.875 | 4.812 | 4.875 | 0.99x | 0.99x | agree |
| string-ops | 94.000 | 93.000 | 92.000 | 1.01x | 0.99x | agree |
| array-functional | 551.000 | 552.000 | 551.000 | 1.00x | 1.00x | agree |
| array-param | 5.375 | 3.312 | 5.312 | 0.62x | 0.62x | agree |
| json-transform | n/a | 16.921 | 16.888 | 1.00x | n/a | agree |
Speculations the emitter made, by evidence (`KILN_NATIVE_TRACE_SPECULATION=1`):
| program | untyped .mjs | typed, types used | typed, types withheld |
| --- | --- | --- | --- |
| nbody | fns_without_facts=1, number_param:inference=1, typed_entry=1, typed_return=1 | number_param:annotation=1, typed_entry=1, typed_return=1 | fns_without_facts=1, number_param:inference=1, typed_entry=1, typed_return=1 |
| mandelbrot | fns_without_facts=1, typed_return=1, unversioned_fns=1 | typed_return=1, unversioned_fns=1 | fns_without_facts=1, typed_return=1, unversioned_fns=1 |
| class-poly | fns_without_facts=3, spec_arrays=2, spec_calls=1, spec_fields=11, typed_entry=2 | fns_without_facts=3, spec_arrays=2, spec_calls=1, spec_fields=11, typed_entry=2 | fns_without_facts=3, spec_arrays=2, spec_calls=1, spec_fields=11, typed_entry=2 |
| object-literal | fns_without_facts=5, spec_fields=4, typed_entry=4, unversioned_fns=4 | fns_without_facts=1, number_param:annotation=4, spec_fields=4, typed_entry=4 | fns_without_facts=5, spec_fields=4, typed_entry=4, unversioned_fns=4 |
| string-ops | none | none | none |
| array-functional | fns_without_facts=3, number_param:inference=4, typed_entry=3, typed_return=2 | fns_without_facts=3, number_param:inference=4, typed_entry=3, typed_return=2 | fns_without_facts=3, number_param:inference=4, typed_entry=3, typed_return=2 |
| array-param | fns_without_facts=2, spec_fields=2, typed_return=1 | array_param:annotation=2, fns_without_facts=1, spec_fields=2, typed_return=1 | fns_without_facts=2, spec_fields=2, typed_return=1 |
| json-transform | n/a | fns_without_facts=2, number_param:annotation=1, spec_fields=5, typed_entry=1, unversioned_fns=1 | fns_without_facts=2, spec_fields=4, typed_entry=1, unversioned_fns=1 |

## Against node 26.8.1 (warm_median ms, load ~2, same checksums)

| program | kiln typed | node |
| --- | --- | --- |
| nbody | 587 | 519 |
| mandelbrot | 18.5 | 18.25 |
| class-poly | 51 | 1.11 |
| object-literal | 4.81 | 0.020 |
| string-ops | 93 | 13.75 |
| array-functional | 552 | 8.63 |
| array-param | 3.31 | 0.875 |
