// kiln npm functional test: uuid
import { v1, v3, v4, v5, NIL, MAX, parse, stringify, validate, version } from 'uuid';

// 1. Deterministic v4 with explicit random bytes
const randomBytes = [
  0x10, 0x91, 0x56, 0xbe, 0xc4, 0xfb, 0x41, 0xea,
  0xb8, 0xde, 0x5d, 0x43, 0xb6, 0xf0, 0x25, 0x01
];
const deterministicV4 = v4({ random: randomBytes });
console.log('v4-random:', deterministicV4);
console.log('v4-version:', version(deterministicV4));
console.log('v4-valid:', validate(deterministicV4));

// 2. Deterministic v1 with fixed msecs & clockseq
const deterministicV1 = v1({
  node: [0x01, 0x23, 0x45, 0x67, 0x89, 0xab],
  clockseq: 0x1234,
  msecs: 1321520000000
});
console.log('v1-fixed:', deterministicV1);
console.log('v1-version:', version(deterministicV1));

// 3. Name-based v3 (MD5) and v5 (SHA-1) with standard namespaces
console.log('v3-dns:', v3('hello.world', v3.DNS));
console.log('v3-url:', v3('http://example.com', v3.URL));
console.log('v5-dns:', v5('hello.world', v5.DNS));
console.log('v5-url:', v5('http://example.com', v5.URL));

// 4. Binary parse & stringify roundtrip
const parsed = parse(deterministicV4);
console.log('parsed-is-uint8:', parsed instanceof Uint8Array);
console.log('parsed-len:', parsed.length);
const reconstructed = stringify(parsed);
console.log('reconstructed:', reconstructed === deterministicV4);

// 5. Validation boundaries
console.log('validate-nil:', validate(NIL));
console.log('validate-max:', validate(MAX || 'ffffffff-ffff-ffff-ffff-ffffffffffff'));
console.log('validate-invalid-len:', validate('not-a-uuid'));
console.log('validate-invalid-chars:', validate('zzzzzzzz-zzzz-zzzz-zzzz-zzzzzzzzzzzz'));
console.log('validate-uppercase:', validate('109156BE-C4FB-41EA-B8DE-5D43B6F02501'));
