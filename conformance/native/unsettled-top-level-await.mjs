// An entry whose top-level await never settles, with nothing left in the loop that could
// settle it: node prints `Warning: Detected unsettled top-level await` and exits 13. The
// interpreter exited 0 and the native launcher refused with status 1 until 2026-09-19 (issue
// 09 of .scratch/full-runtime); native.sh compares the exit status, which is the whole point.
console.log("before");
await new Promise(() => {});
console.log("never");
