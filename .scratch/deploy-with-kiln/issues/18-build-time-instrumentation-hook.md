# 18. A build-time instrumentation hook

Status: needs-triage

ADR 0067 §5 refuses module patching (OpenTelemetry auto-instrumentation, APM agents using
`require-in-the-middle` / `import-in-the-middle`) and names a build-time instrumentation hook as the
intended answer: instrument a program statically, as the closed-world equivalent of patching modules
as they load.

Write that ADR when the work is scheduled. It needs to say how an instrumentation package declares
what it wraps, how the build applies it, and how the result stays equivalent to what the same
package does under Node. The OpenTelemetry stretch app (issue 15) is its test.

## Acceptance

- An ADR, and the OpenTelemetry stretch app producing the same spans as under Node.
