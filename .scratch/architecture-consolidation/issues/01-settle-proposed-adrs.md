# 01. Settle the proposed ADRs

Status: resolved

Ten ADRs are marked Proposed. While ADR 0059 and ADR 0060 are unaccepted, ADR 0043's rule that a
built-in member "always remains separately implemented by the two engines" is still the recorded
architecture, and an agent adding a Rust and a C version of a member is following it.

Accept, reject or rewrite each:

| ADR | subject | needed by this plan |
| --- | --- | --- |
| 0060 | one authoritative owner (revised 2026-09-26) | yes: 07, 09, 10, 11 |
| 0065 | declared primitive set | yes: 08, 09, 10, 11 |
| 0066 | crate layers | yes: 05, 06 |
| 0059 | intrinsics are ordinary realm objects | yes: 11 |
| 0061 | bytecode is the source of the value graph | no, but it is what makes 0065 work |
| 0021 | value and heap representation decided before the object model | no |
| 0050 | objects point at shapes | no |
| 0057 | region-isolated actors | no |
| 0058 | Rust-backed native concurrency | no |
| 0064 | io_uring completion engine | no |

A rejected ADR stays in the tree with its status changed and a paragraph saying why.

## Acceptance

- Every ADR above has a status other than Proposed, or a dated note saying what it is waiting on.

## Resolution, 2026-09-26

Decided by the project owner:

| ADR | outcome |
| --- | --- |
| 0060 (as revised) | Accepted |
| 0065 | Accepted |
| 0066 | Accepted, including the move into layer directories |
| 0059 | Accepted |
| 0050, 0058, 0061 | Accepted as built |
| 0021 | Superseded by ADR 0053 |
| 0057, 0064 | Still proposed, with a dated note naming what each is waiting on |

Issues 05, 06, 07 and 08 are unblocked.
