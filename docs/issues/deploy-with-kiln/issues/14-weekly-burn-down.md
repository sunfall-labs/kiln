# 14. The weekly burn-down

Status: ready-for-agent
Blocked by: 02

Once the harness exists, the corpus's refusals and divergences **are** the priority queue:

1. Run the whole corpus weekly and record the date, the commit, matched / refused / diverged per
   app, and the refusal distribution by member and by package.
2. Divergences first (a wrong answer is worse than a stop), then the refusal that unblocks the most
   apps, then the one that unblocks the most requests within an app.
3. Feed the list into `docs/issues/full-runtime/` as its demand order. That PRD already says "demand
   first"; this is the demand.

Each fix follows the rules in the PRD: TypeScript over primitives or a shared Rust engine, no new
C built-ins, no new duplicates.

## Acceptance

- A dated log in this file's comments, one entry per week, with the numbers above.
- The compatibility page (12) regenerated in the same change as each log entry.
