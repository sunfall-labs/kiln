`multipart-headers.ts` is an unchanged copy of
`packages/effect/src/http/MultipartParser/internal/headers.ts` from Effect commit
`67ba4e46a11ccda0b6761578bfd22c04ae00167d`.

It reproduces nondeterministic loop hoisting that produced two different generated-C
payloads under one artifact-cache key. The adjacent `.LICENSE` preserves its MIT license.
