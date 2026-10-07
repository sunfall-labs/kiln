# Automatic mbx pruning incident

During corrected full-GUI provider packaging, mbx 1.21.0 automatically ran its
disk-pressure collector despite `MBX_GC_AUTO=false` in the command environment.
The packaging log reported 7.1 GiB free below its 50 GiB minimum and removed
14 target directories (all classified as live), plus 4,539 objects and 979
action results. This was automatic tool behavior, not an explicit shared-cache
GC command. The full packaging log remains at
`.scratch/full-gui-http-fixed-packaging.log` in the milestone worktree.

The frozen combined CLI and retained full-gate raw evidence were checked after
the incident and remained present. Previously recorded cleanup of owned native
outputs and owned kiln caches is distinct from this automatic pruning.

The supported configuration interface now disables automatic pruning:

```sh
mise exec -- mbx settings set gc.auto false
mise exec -- mbx settings get gc.auto
```

The previous setting was `true`; the verified new value is `false`. This is a
global, reversible mbx setting. Keep it disabled during concurrent worktree
validation to prevent automatic removal of other worktrees' live build outputs.
Do not restore it implicitly at the end of an individual build.

After packaging, the October 4 resource check showed 47 GiB physical disk free,
9.6 GiB available RAM, and no active compiler, gate, or benchmark processes.
Unrelated idle debugger sessions were left untouched. The next native check is
bounded to two workers, with no concurrent broad benchmark run.
