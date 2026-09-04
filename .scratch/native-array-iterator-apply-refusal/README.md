# `async-concurrency` refuses natively on an array iterator's source

**Status:** open, and **pre-existing on `origin/main`** -- recorded because it was found while
merging and could easily be blamed on the merge.

```
thirdparty (native): 18 matched, 1 refused, 0 diverged
  async-concurrency  operation=Apply
    detail=an array iterator whose source is not an array or a typed array
```

## It is not the merge

Measured on a worktree at `origin/main` (`2cd616c1`) alone, built from scratch:

```
thirdparty (native): 16 matched, 1 refused, 0 diverged
  async-concurrency  operation=Apply  ... same detail
```

Same bundle, same refusal. The merged tree matches more bundles and diverges on none, so the merge
is a strict improvement; this row simply came along with it.

The refusal *site* is older than either branch -- `git log -S` puts it at `4c6f8642`, "Make an
unnamed refusal a build failure". What changed is that this bundle now reaches it, which points at
the ADR 0050 dense-elements work: an array iterator over dense storage is no longer the shape the
`Apply` path recognises.

## Why it is a refusal and not a divergence

The backend stops at the cause and names the member, which is the behaviour this repository asks
for and the reason the row reads `refused` rather than `diverged`. Nothing answers wrongly. It is a
capability regression rather than a correctness one: a bundle that used to run natively now does
not.

## The shape of a fix

`Apply` needs to accept an array iterator whose source is the dense representation, not only one
whose source is a `Value::Array` or a typed array. Worth pairing with a `conformance/native` probe
that spreads an array iterator through `apply`, since none exists -- which is why only a real
bundle caught it.
