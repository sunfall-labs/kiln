# Preserve the operation behind a native host refusal

Status: ready-for-agent

## Failure

The freshly built ghui executable exits with only `kiln: the host refused an operation`. The
interpreter identifies the later OpenTUI platform boundary precisely, but the native failure does
not identify its module, instruction, or requested host operation. This prevents establishing
whether the native path reaches the same boundary without instrumenting and rebuilding an
approximately fourteen-minute application.

## Required result

Carry a bounded diagnostic through `KILN_C99_HOST_ERROR` and print it from
`kiln_cli_report_failure_in`, with a focused executable test. The diagnostic must name enough of
the operation to distinguish module loading, filesystem access, terminal I/O, and another host
capability without exposing ambient host data.
