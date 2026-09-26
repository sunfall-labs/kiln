# 07. TLS

Status: ready-for-agent

There is no `node:tls`, and `node:https` throws "requires a TLS host capability that this realm does
not provide". This blocks outbound HTTPS, managed databases that require TLS, and HTTPS servers.

Implement TLS as a shared Rust engine in the runtime (ADR 0058's pattern: Rust behind a coarse
provider ABI), most likely `rustls` on the existing Tokio transport, with the Node API written once
in TypeScript over it:

1. `tls.connect` and the `TLSSocket` a client needs, including SNI, ALPN and the system trust store
   plus `ca` overrides; the upgrade of an existing socket (`tls.connect({ socket })`), which is how
   Postgres and Redis drivers start TLS;
2. `https.request` / `https.get` and `fetch` to `https:` URLs (`web.ts` parses the scheme; verify
   whether any of it actually negotiates TLS today);
3. `tls.createServer` and `https.createServer`.

Client first: far more services call HTTPS than serve it, because a load balancer usually
terminates TLS.

## Acceptance

- The corpus's outbound HTTPS app and `pg` over TLS match Node natively and interpreted.
- An expired or wrong-host certificate fails with Node's error code (`CERT_HAS_EXPIRED`,
  `ERR_TLS_CERT_ALTNAME_INVALID`).
- Binary size cost measured and linked on demand (ADR 0062): an app that never uses TLS does not
  carry it.
