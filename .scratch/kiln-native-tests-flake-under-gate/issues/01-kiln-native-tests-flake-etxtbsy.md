# ETXTBSY Spurious Failures in kiln-native Tests Under Load

Status: resolved · Severity: medium

## Comments

Fixed 2026-09-04.
The failure was caused by transient descriptor duplication during concurrent compiler fork/execve operations. Extending retry ceiling to a 10s deadline permanently eliminated gate flakes.
