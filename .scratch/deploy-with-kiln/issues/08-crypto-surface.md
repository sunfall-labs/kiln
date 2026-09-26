# 08. The crypto surface services use

Status: ready-for-agent

`node:crypto` today: `createHash`, `createHmac`, `Hash`, `Hmac`, `getHashes`, `randomBytes`,
`randomFillSync`, `randomUUID`, `getRandomValues`, `timingSafeEqual`. Services also need, driven by
the corpus:

- `createSign` / `createVerify`, `sign` / `verify`, `KeyObject`, `createPrivateKey` /
  `createPublicKey` (RS256 and ES256 JWTs, webhook signatures);
- `pbkdf2`, `scrypt`, `hkdf` (Postgres SCRAM-SHA-256 auth, password hashing);
- `createCipheriv` / `createDecipheriv` (AES-GCM, AES-CBC) for encrypted cookies and sessions;
- `generateKeyPair`, and `crypto.subtle` for libraries written against Web Crypto (`jose`, Hono's
  JWT helper).

A shared Rust engine behind the provider (RustCrypto crates or `aws-lc-rs`, chosen with TLS in 07 so
the two share one implementation), with the Node and Web Crypto APIs in TypeScript over it.

Order by corpus demand, not by API completeness.

## Acceptance

- The JWT app (HS256 and RS256) and `pg` with SCRAM match Node natively.
- Every added member is owned once (ADR 0060); the duplicate count does not rise.
