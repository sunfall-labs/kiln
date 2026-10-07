# Identify foreground ghui command timeouts

Status: planned

## Evidence

The live read-only acceptance passed navigation, PR detail rendering, markdown scrolling, Escape
back, terminal restoration, and exit 0. The transcript nevertheless showed one transient
`Timed out after 15000ms` footer while entering PR details. The ghui command runner applies one
15-second timeout to every `process.run` call. The terminal transcript has no request IDs or
timestamps, so it cannot prove whether the selected PR GraphQL query or a neighbor prefetch was the
request that crossed the budget.

## Durable fix

Add an operation label, request ID, elapsed duration, and safe command metadata to timeout telemetry
and user-facing diagnostics. Give foreground detail hydration and list refreshes independently
configurable budgets. Keep silent prefetch failures silent until a prefetch is promoted to the
foreground, and show cached detail while a stale-while-revalidate refresh runs. Preserve explicit
AbortSignal cancellation and add a regression probe that confirms a timed-out child is killed and
reaped with no late result applied to the selected PR.

This issue is app-side follow-up only; it does not authorize changes to the Kiln runtime or GitHub
state.
