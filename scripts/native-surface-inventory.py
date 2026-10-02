#!/usr/bin/env python3
"""Ask both engines what they implement, one member at a time, and write down the difference.

WHY THIS EXISTS. ADR 0043 put the fifteen table-driven built-ins on a seam: one shared table, a
`const` assertion that fails the build when the two engines drift, and a backlog with reasons.
That inventory is trustworthy and it covers about a fifth of the surface. Everything else --- the
prototype method tables, the `node:` namespaces, the globals that are not table-driven --- has no
inventory at all, so its gaps have been discovered one npm bundle at a time. `node:path` publishes
the interpreter's whole export list and fills five of them, which is how a bundled `postcss` walks
past a feature guard into a wrong answer. Nothing in the repository could have told anyone that
before this script.

THE METHOD IS TO ASK, NOT TO READ. The six hand-written `kiln_c99_is_*_member` lists in the
emitted C answer "is this a member of the language?", which is a different question from "does
this backend implement it?", so they cannot be diffed into a list of what is missing. Instead:
enumerate the interpreter's own surface with `Object.getOwnPropertyNames`, then put each name to
the native backend and classify what comes back. No list in this file names a built-in or a
member. Add a member to either engine and the next run finds it.

THE OBSTACLE IS THAT A REFUSAL ABORTS THE PROCESS. It is not a JavaScript exception --- `try` does
not catch it --- so a probe that walks the surface in one pass reports everything up to the first
gap and nothing after. That is the same trap ADR 0043 describes: `Object` refused on
`getOwnPropertySymbols`, and every name alphabetically later was invisible. The way out is that
output *before* the refusal survives, and the refusal names the member it refused. So each
receiver is probed in rounds: run the whole pending list, keep every answer that printed, record
the one that refused, drop it, run again. A receiver costs one native compile per refusal plus
one. Receivers are independent, so the rounds run in parallel.

AND THE RECEIVER IS ASKED ABOUT ITSELF, not only about its members. A walk that only enumerates
members cannot see a receiver that is the wrong *kind* of thing: `require("node:module")` is a
callable class in node and in the interpreter and was a namespace object in the native backend,
and every member of it agreed, so nothing here reported anything for months. Each receiver
therefore carries two more rows --- `(itself)`, its own `typeof`, and `(itself).name` where either
engine answers one --- classified exactly like a member. A blind spot in the instrument is worse
than a gap in the engine, because it hides an unknown number of gaps.

Three classes come out, and the order they are reported in is the point:

  diverged     native answered, and answered differently from the interpreter. Includes the
               present-but-`undefined` case, which is the dangerous one: `typeof x === "undefined"`
               is how real code feature-detects, so the program takes the other branch and carries
               on with a bad value.
  refused      native stopped and named the member. Honest backlog; a program dies at the cause.
  implemented  the two engines agree.

AND ONE OF THE THREE IS A GATE. `--gate` exits non-zero when a member reads `undefined` natively
where the interpreter has a value. That class is not one gap among many: a refusal stops a program
at the member it lacked, and an `undefined` walks past `typeof x.member === "function"` and dies
three frames later with nothing naming the cause. A bundled `postcss` died on `path.dirname` that
way for weeks. Everything else this tool measures is a report somebody reads; this is the part
that fails, because a report nobody diffs is how thirty of these survived.

`SILENT_UNDEFINED_ACCEPTED` is the only way past it, and a row there is a sentence somebody wrote.

Usage:

    scripts/native-surface-inventory.py                    # full run, writes the report
    scripts/native-surface-inventory.py --gate             # ...and fail on a silent undefined
    scripts/native-surface-inventory.py --stage report --gate   # gate the saved measurement
    scripts/native-surface-inventory.py --only node:       # one slice of the surface
    scripts/native-surface-inventory.py --stage enumerate  # what the interpreter has, no compiles

It needs a release build (`mbx build --release -p kiln-cli`) and it costs several hundred native
compiles, which is several gigabytes of `.kiln/cache`. `--disk-floor-gib` is the pressure valve:
below it the tool reclaims its own build scratch and, if that is not enough, stops rather than
filing a run that ended in ENOSPC. That is deliberate. A run that hits a full disk or a timeout is
not a measurement, and `NOTES.md` has a day in it that says so.

The tool is deliberately re-runnable and its outputs are deterministic (every list is sorted), so
`git diff` on the report is the review after a backend change.
"""

from __future__ import annotations

import argparse
import contextlib
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass, field
from pathlib import Path

# `$262` is the Test262 host hooks the runner installs. They are a statement about what the host
# can do rather than about the language, and they are not the native backend's to carry ---
# `conformance/native/global-surface.ts` skips them for the same reason and says so.
HOST_HOOK_PREFIX = "$262"

# The `node:` specifiers the module resolver knows. This is the one list the script does hold,
# because a namespace is only reachable by naming it: there is no `Object.getOwnPropertyNames` for
# "every module the host will resolve". It is checked against the resolver at run time by the
# resolver refusing anything it does not know, which shows up as an unreachable receiver.
NODE_SPECIFIERS = [
    "node:assert",
    "node:async_hooks",
    "node:buffer",
    "node:crypto",
    "node:diagnostics_channel",
    "node:dns",
    "node:events",
    "node:fs",
    "node:fs/promises",
    "node:http",
    "node:http2",
    "node:https",
    "node:module",
    "node:net",
    "node:os",
    "node:path",
    "node:perf_hooks",
    "node:querystring",
    "node:stream",
    "node:string_decoder",
    "node:tty",
    "node:url",
    "node:util",
    "node:worker_threads",
    "node:zlib",
]

# Printed as the last line of every generated program. It is how the harness tells "ran to
# completion" from "died", which the exit status cannot do here.
SENTINEL = "#END"

IDENTIFIER = re.compile(r"^[A-Za-z_$][A-Za-z0-9_$]*$")

# The two rows a receiver contributes about *itself*. They are spelled as members so that every
# count, every table and the gate treat them like any other row -- a receiver that is an object in
# one engine and a function in the other is a divergence of exactly the kind this tool reports,
# and it was invisible while the walk only ever asked about members. The names are not identifiers,
# so nothing can collide with a real member: `Object.getOwnPropertyNames` never returns these.
SELF_TYPEOF = "(itself)"
SELF_NAME = "(itself).name"

# `kiln: native backend refusal: module=... instruction=41 operation=GetProperty(seal) ...`
REFUSAL = re.compile(r"^kiln: native (?:backend )?(.*)$")
REFUSED_MEMBER = re.compile(r"operation=\w+\(([^)]*)\)")

# Not every `kiln: native ...` line is a refusal, and the ones that are not must never be filed as
# one. This tool hands its own build scratch back after every receiver while other receivers are
# still compiling, so a compile can find the directory it was about to write into already gone and
# say `cache failed: could not install cache artifact ...`. That is this tool tripping over its own
# feet, not the backend declining a member --- and the first full run on `main` filed
# `WeakRef.prototype`'s two members as refusals because of it. A run that says one of these is not
# a measurement: retry it, and if it keeps happening, name it under "Runs that were not
# measurements" rather than folding it into the backlog.
NOT_A_REFUSAL = ("cache failed:",)

# The shared description of a value. It must produce identical bytes in both engines from
# operations both engines have, which rules out `Object.prototype.toString.call` and rules out
# reading `.name` or `.length` off a built-in function --- ADR 0043 records that reading `length`
# on a built-in refuses, and a probe whose own scaffolding refuses measures nothing. The cost is
# that two different functions, or two different objects, both describe as "function"/"object";
# see "What this cannot see" in the report.
DESCRIBE = """
function kilnDescribe(v) {
  var t = typeof v;
  if (t === "undefined") return "undefined";
  if (t === "object") return v === null ? "null" : "object";
  if (t === "function") return "function";
  if (t === "symbol") return "symbol";
  if (t === "string") return "string " + JSON.stringify(v);
  return t + " " + String(v);
}
"""


def js_string(text: str) -> str:
    return json.dumps(text)


def member_access(receiver: str, name: str) -> str:
    """How a program actually spells this read.

    An identifier gets dot access, which lowers to a named `GetProperty` --- so when the backend
    refuses, the diagnostic carries the member's name and the round does not have to infer it from
    how far the output got. A `@@` name is symbol-keyed and reached through `Symbol`, which lowers
    to `GetComputed` and refuses anonymously; that is why the round parser keeps the positional
    fallback.
    """
    if name.startswith("@@"):
        return f"{receiver}[Symbol.{name[2:]}]"
    if IDENTIFIER.match(name):
        return f"{receiver}.{name}"
    return f"{receiver}[{js_string(name)}]"


def receiver_expression(key: str) -> tuple[str, str]:
    """Turn a receiver key back into the JavaScript that reaches it.

    The key is the path the walk took, so the expression is derivable from it and the two stay in
    step without a second table. `globalThis.X` rather than a bare `X` on purpose: a global the
    native backend does not have then refuses by name, which is a measurement, where a bare
    identifier would be a link failure, which is not.
    """
    if key.startswith("node:"):
        return "namespace", f"require({js_string(key)})"
    kind = "static"
    base = key
    wrap = 0
    while base.endswith("^"):
        base = base[:-1]
        wrap += 1
    if base.endswith(".prototype"):
        kind = "prototype"
        expr = f"globalThis.{base[: -len('.prototype')]}.prototype"
    elif base == "globalThis":
        kind = "global"
        expr = "globalThis"
    else:
        expr = f"globalThis.{base}"
    for _ in range(wrap):
        kind = "inherited"
        expr = f"Object.getPrototypeOf({expr})"
    return kind, expr


@dataclass
class Receiver:
    key: str  # stable id, e.g. "Array.prototype" or "node:path"
    kind: str  # "global" | "static" | "prototype" | "namespace"
    expr: str  # JavaScript that evaluates to the receiver
    needs_node: bool
    names: list[str] = field(default_factory=list)  # what the interpreter publishes
    interpreted: dict[str, str] = field(default_factory=dict)
    native: dict[str, str] = field(default_factory=dict)
    refused: dict[str, str] = field(default_factory=dict)  # name -> refusal detail
    native_names: list[str] | None = None  # what native publishes, or None if it refused to say
    self_description: str | None = None  # why it refused to say
    unreachable: str | None = None
    # The receiver's own two readings, keyed by SELF_TYPEOF / SELF_NAME. A receiver is a value
    # before it is a bag of members, and until this existed the tool could not see a receiver that
    # was the wrong *kind* of thing in one engine -- only that its members happened to answer.
    self_interpreted: dict[str, str] = field(default_factory=dict)
    self_native: dict[str, str] = field(default_factory=dict)
    self_refused: dict[str, str] = field(default_factory=dict)
    # Node's answers, from the same enumeration run under the pinned `node`. Node is the target
    # of both engines (ADR 0063); the interpreter-versus-native reading above is the near gap,
    # and this column is what turns the far gap into a number instead of a surprise. `node_names`
    # is None for a receiver node does not have at all.
    node: dict[str, str] = field(default_factory=dict)
    node_names: list[str] | None = None
    self_node: dict[str, str] = field(default_factory=dict)
    # A receiver node has and neither engine publishes. It is never probed natively -- there is
    # nothing to probe -- and counts as missing in both engines, every member of it.
    node_only: bool = False
    # Members that are kiln's own (ADR 0068): published by default and gone under
    # `KILN_EXTENSIONS=0`. Measured the same way as everything else -- the interpreter enumerated a
    # second time with the switch set, and the difference is the list -- so this tool still names
    # no member itself. They are reported in a column of their own, never as "extra".
    kiln_extensions: list[str] = field(default_factory=list)


class Runner:
    def __init__(self, kiln: Path, root: Path, workdir: Path, timeout: int) -> None:
        self.kiln = kiln
        self.root = root
        self.workdir = workdir
        self.timeout = timeout
        self.native_runs = 0
        self.errors: list[str] = []

    def run(
        self, source: str, *, native: bool, node: bool, tag: str, env: dict[str, str] | None = None
    ) -> tuple[list[str], str | None]:
        """Run one generated program, retrying anything that is not a clean answer or a refusal.

        Returns (printed lines, refusal text or None). A refusal is the measurement and is never
        retried. Anything else that fails --- a timeout, a link that ran out of disk, a toolchain
        hiccup under load --- is *not* a measurement, and the note this repository keeps about
        `native.sh` miscounting under load says exactly what happens when such a run is filed as
        one: phantom divergences that nobody can reproduce. So those are retried, and if they
        still fail they are counted as errors and named in the report rather than folded into a
        class.
        """
        for attempt in range(3):
            lines, refusal, ok = self._attempt(source, native=native, node=node, tag=tag, env=env)
            if ok:
                return lines, refusal
        self.errors.append(f"{tag}: {refusal}")
        # `not probed:` is the spelling `classify` routes to *unmeasured*. Saying it here is what
        # keeps a run that failed three times out of the refusal count: the first full run on
        # `main` put `WeakRef.prototype`'s two members in the backlog because a give-up still
        # looked like a refusal to the caller, and two members of a thousand is small enough that
        # nobody would have caught it by reading.
        return lines, f"not probed: {refusal}"

    def run_under_node(self, source: str, *, tag: str) -> list[str]:
        """The same generated program under the reference runtime. Node is not measured for
        refusals; a failure here is a failure of the tool, and says so."""
        path = self.workdir / f"{tag}.node.cjs"
        path.write_text(source)
        try:
            proc = subprocess.run(
                # Reading a getter off a prototype (`ReadableStreamBYOBReader.prototype.closed`)
                # hands back a rejected promise, which node otherwise treats as fatal.
                ["node", "--unhandled-rejections=none", str(path)],
                cwd=self.root, capture_output=True, text=True,
                timeout=self.timeout,
            )
        except FileNotFoundError:
            sys.exit("no `node` on PATH: the inventory measures both engines against it")
        except subprocess.TimeoutExpired:
            sys.exit(f"node timed out enumerating its own surface ({tag})")
        if proc.returncode != 0:
            sys.exit(f"node could not enumerate its own surface ({tag}): {proc.stderr.strip()[:400]}")
        return proc.stdout.splitlines()

    def _attempt(
        self, source: str, *, native: bool, node: bool, tag: str, env: dict[str, str] | None = None
    ) -> tuple[list[str], str | None, bool]:
        suffix = ".cjs" if node else ".js"
        path = self.workdir / f"{tag}{suffix}"
        path.write_text(source)
        argv = [str(self.kiln), "--one-shot"]
        if node:
            argv.append("--node")
        argv.append("run")
        if native:
            argv.append("--native")
        argv.append(str(path))
        if native:
            self.native_runs += 1
        try:
            proc = subprocess.run(
                argv,
                cwd=self.root,
                capture_output=True,
                text=True,
                timeout=self.timeout,
                env={**os.environ, **env} if env else None,
            )
        except subprocess.TimeoutExpired:
            # A timed-out run is not a measurement. Say so rather than filing its partial output.
            return [], f"TIMEOUT after {self.timeout}s", False
        finally:
            if os.environ.get("KILN_INVENTORY_KEEP"):
                print(f"  kept {path}: {' '.join(argv)}", file=sys.stderr)
            else:
                path.unlink(missing_ok=True)
        lines: list[str] = []
        refusal: str | None = None
        for line in (proc.stdout + proc.stderr).splitlines():
            match = REFUSAL.match(line)
            if match is not None and refusal is None:
                refusal = re.sub(r" module=\S+", "", match.group(1))
                continue
            lines.append(line)
        if refusal is not None and refusal.startswith(NOT_A_REFUSAL):
            # A `kiln: native ...` line that is this tool's own scratch going missing under it.
            # Fail the attempt so it is retried, and keep the text so a run that never recovers is
            # named rather than filed as backlog.
            return lines, refusal, False
        # Completion is the sentinel, not the exit status. A `.cjs` entry run through the Node
        # adapter exits non-zero after printing every line it was asked for --- both engines do,
        # so it is the harness and not the backend --- and treating that as a failed run retried
        # every `node:` namespace three times and then filed twenty-five complete measurements as
        # errors. The program says when it finished; believe the program.
        if refusal is not None or SENTINEL in lines:
            return [line for line in lines if line != SENTINEL], refusal, True
        tail = " / ".join((proc.stdout + proc.stderr).splitlines()[-2:]) or "no output"
        return lines, f"exit {proc.returncode}: {tail}", False


# --------------------------------------------------------------------------------------------
# Stage 1: ask the interpreter what its surface is, and what every member of it answers.
# --------------------------------------------------------------------------------------------

# The walk. Receivers are collected by identity so an alias (`global` is `globalThis`) and a
# shared parent (every typed array's prototype inherits from the same `%TypedArray%.prototype`)
# are probed once rather than once per name that reaches them.
#
# A `^` suffix on a key means "the object this one inherits from". That is how the shared
# prototypes get inventoried at all: `%TypedArray%.prototype` carries every typed-array method and
# is not reachable by any name on the global object, so `Uint8Array.prototype` looks like it has
# two members until you follow the chain.
COLLECT = """
%(describe)s
var keys = [];
var vals = [];
function push(key, v) {
  if (v === null || (typeof v !== "object" && typeof v !== "function")) return;
  for (var i = 0; i < vals.length; i++) { if (vals[i] === v) return; }
  keys.push(key); vals.push(v);
}
function wellKnown(sym) {
  var names = Object.getOwnPropertyNames(Symbol);
  for (var i = 0; i < names.length; i++) {
    var candidate;
    try { candidate = Symbol[names[i]]; } catch (e) { continue; }
    if (typeof candidate === "symbol" && candidate === sym) return names[i];
  }
  return null;
}
function report(key, v) {
  var own;
  // The receiver's own answer, before any of its members. It is printed first because it is the
  // one reading that does not depend on the walk getting anywhere: a receiver whose own keys
  // cannot be listed still has a `typeof`, and `typeof require("node:module")` differing between
  // the engines went unmeasured for months precisely because nothing asked this.
  var selfName;
  try { selfName = kilnDescribe(v.name); } catch (e) { selfName = "throws"; }
  console.log("S\\t" + key + "\\t" + kilnDescribe(v) + "\\t" + selfName);
  try { own = Object.getOwnPropertyNames(v); } catch (e) { return; }
  var syms = [];
  try { syms = Object.getOwnPropertySymbols(v); } catch (e) { syms = []; }
  for (var s = 0; s < syms.length; s++) {
    var wk = wellKnown(syms[s]);
    if (wk !== null) own.push("@@" + wk);
  }
  own = own.sort();
  for (var j = 0; j < own.length; j++) {
    var n = own[j];
    var mv;
    try {
      mv = n.indexOf("@@") === 0 ? v[Symbol[n.slice(2)]] : v[n];
    } catch (e) { console.log("M\\t" + key + "\\t" + n + "\\tthrows"); continue; }
    console.log("M\\t" + key + "\\t" + n + "\\t" + kilnDescribe(mv));
  }
}
"""

ENUMERATE_GLOBALS = """
%(collect)s
var hostHook = %(hook)s;
var skip = %(skip)s;
var globals = Object.getOwnPropertyNames(globalThis).filter(function (n) {
  return n.indexOf(hostHook) !== 0 && skip.indexOf(n) === -1;
}).sort();
push("globalThis", globalThis);
for (var i = 0; i < globals.length; i++) {
  try { push(globals[i], globalThis[globals[i]]); } catch (e) {}
}
for (var i2 = 0; i2 < globals.length; i2++) {
  var g = globals[i2];
  try {
    var c = globalThis[g];
    if (typeof c === "function") push(g + ".prototype", c.prototype);
  } catch (e) {}
}
var direct = keys.length;
for (var i3 = 0; i3 < direct; i3++) {
  try { push(keys[i3] + "^", Object.getPrototypeOf(vals[i3])); } catch (e) {}
}
for (var i4 = 0; i4 < keys.length; i4++) report(keys[i4], vals[i4]);
console.log("#END");
"""

# `require` takes a literal here on purpose: the module graph is resolved ahead of time, so a
# specifier held in a variable is not a module reference and the host answers "Cannot find
# module". Ninety seconds went into learning that; the loop is unrolled in Python instead.
ENUMERATE_NAMESPACES = """
%(collect)s
%(requires)s
for (var i = 0; i < keys.length; i++) report(keys[i], vals[i]);
console.log("#END");
"""


@contextlib.contextmanager
def built_outputs(root: Path, workdir: Path):
    """Remove what `kiln run --native` built for the probes under `workdir` once the run is over.

    A native run of `.scratch/surface-x/probe.cjs` links its executable under
    `.kiln/bin/.scratch/surface-x/`, beside nothing this tool reads back. The temporary directory
    goes at the end of the run; its build outputs did not, and nineteen runs had left 62 GiB
    there by 2026-09-19 -- the disk this tool's own `guard_disk` was watching.
    """
    try:
        yield
    finally:
        try:
            relative = workdir.resolve().relative_to(root.resolve())
        except ValueError:
            return
        shutil.rmtree(root / ".kiln" / "bin" / relative, ignore_errors=True)


def enumerate_interpreter(runner: Runner) -> dict[str, Receiver]:
    receivers: dict[str, Receiver] = {}

    def receiver_for(key: str, needs_node: bool) -> Receiver:
        if key in receivers:
            return receivers[key]
        kind, expr = receiver_expression(key)
        receivers[key] = Receiver(key, kind, expr, needs_node)
        return receivers[key]

    def ingest(lines: list[str], needs_node: bool) -> None:
        for line in lines:
            parts = line.split("\t")
            if len(parts) == 4 and parts[0] == "S":
                _, key, described, named = parts
                rec = receiver_for(key, needs_node)
                rec.self_interpreted[SELF_TYPEOF] = described
                rec.self_interpreted[SELF_NAME] = named
                continue
            if len(parts) != 4 or parts[0] != "M":
                continue
            _, key, name, value = parts
            rec = receiver_for(key, needs_node)
            rec.names.append(name)
            rec.interpreted[name] = value

    collect = COLLECT % {"describe": DESCRIBE}
    source = globals_source = ENUMERATE_GLOBALS % {
        "collect": collect, "hook": js_string(HOST_HOOK_PREFIX), "skip": "[]"
    }
    # Under the adapter, because that is what a program sees: node is the target of both engines
    # (ADR 0063) and node has `process`, `Buffer` and `fetch` on every global object. Measured
    # without it until 2026-09-19, the far gap counted those as missing on `globalThis` and never
    # saw `Buffer.prototype` at all.
    lines, refusal = runner.run(source, native=False, node=True, tag="enumerate-globals")
    if refusal is not None:
        sys.exit(f"the interpreter could not enumerate its own globals: {refusal}")
    ingest(lines, True)

    requires = "\n".join(
        f"try {{ push({js_string(spec)}, require({js_string(spec)})); }} catch (e) "
        f'{{ console.log("U\\t" + {js_string(spec)} + "\\t" + String(e)); }}'
        for spec in NODE_SPECIFIERS
    )
    source = namespaces_source = ENUMERATE_NAMESPACES % {"collect": collect, "requires": requires}
    lines, refusal = runner.run(source, native=False, node=True, tag="enumerate-namespaces")
    if refusal is not None:
        sys.exit(f"the interpreter could not enumerate the node namespaces: {refusal}")
    for line in lines:
        parts = line.split("\t")
        if parts[0] == "U" and len(parts) >= 3:
            receiver_for(parts[1], True).unreachable = f"interpreter: {parts[2]}"
    ingest(lines, True)

    for rec in receivers.values():
        rec.names = sorted(set(rec.names))

    # The same two walks with kiln's own members switched off (ADR 0068). What disappears is what
    # the engines declare as an extension; asking rather than reading keeps this file free of
    # member names, and it proves the switch removes exactly those members while it is at it.
    without: dict[str, set[str]] = {}
    for tag, walk in (("globals", globals_source), ("namespaces", namespaces_source)):
        lines, refusal = runner.run(
            walk, native=False, node=True, tag=f"enumerate-{tag}-without-extensions",
            env={"KILN_EXTENSIONS": "0"},
        )
        if refusal is not None:
            sys.exit(f"the interpreter could not enumerate its {tag} without extensions: {refusal}")
        for line in lines:
            parts = line.split("\t")
            if len(parts) == 4 and parts[0] == "M":
                without.setdefault(parts[1], set()).add(parts[2])
    for key, rec in receivers.items():
        if key in without:
            rec.kiln_extensions = sorted(set(rec.names) - without[key])
    return receivers


def node_version() -> str:
    try:
        return subprocess.run(["node", "--version"], capture_output=True, text=True, timeout=20).stdout.strip()
    except (OSError, subprocess.TimeoutExpired):
        return "unknown"


def enumerate_node(runner: Runner, receivers: dict[str, Receiver]) -> None:
    """Node's own answers for every receiver, and the receivers only node has.

    Every receiver is probed under the adapter, so a receiver missing here is one the interpreter
    does not publish to a program at all.
    """
    collect = COLLECT % {"describe": DESCRIBE}

    def ingest(lines: list[str]) -> None:
        for line in lines:
            parts = line.split("\t")
            if len(parts) != 4 or parts[0] not in ("S", "M"):
                continue
            _, key, a, b = parts
            rec = receivers.get(key)
            if rec is None:
                kind, expr = receiver_expression(key)
                rec = Receiver(key, kind, expr, True)
                rec.node_only = True
                receivers[key] = rec
            if rec.node_names is None:
                rec.node_names = []
            if parts[0] == "S":
                rec.self_node[SELF_TYPEOF] = a
                rec.self_node[SELF_NAME] = b
            else:
                rec.node_names.append(a)
                rec.node[a] = b

    # Node v25 dies with a bus error reading `Storage.prototype`'s accessors off the prototype
    # (Web Storage); the walk cannot catch a crash, so those three are left out of node's list.
    node_source = ENUMERATE_GLOBALS % {
        "collect": collect,
        "hook": js_string(HOST_HOOK_PREFIX),
        "skip": '["Storage", "localStorage", "sessionStorage"]',
    }
    ingest(runner.run_under_node(node_source, tag="enumerate-globals"))
    requires = "\n".join(
        f"try {{ push({js_string(spec)}, require({js_string(spec)})); }} catch (e) {{}}"
        for spec in NODE_SPECIFIERS
    )
    source = ENUMERATE_NAMESPACES % {"collect": collect, "requires": requires}
    ingest(runner.run_under_node(source, tag="enumerate-namespaces"))
    for rec in receivers.values():
        if rec.node_names is not None:
            rec.node_names = sorted(set(rec.node_names))


# WinterTC's Minimum Common Web Platform API (Ecma TC55, https://min-common-api.proposal.wintertc.org/),
# as (receiver key, member) pairs the walk above already measures. The spec names the interfaces
# and functions every server-side runtime is expected to share, and defines conformance by the
# Web Platform Tests for each; this table answers the cheaper question first -- is it there at
# all, in each engine, and does node have it -- and `scripts/wpt.sh` answers the rest.
WINTERTC_MINIMUM_COMMON_API: list[tuple[str, str]] = (
    [("globalThis", name) for name in [
        # DOM, HTML, WebIDL
        "AbortController", "AbortSignal", "Event", "EventTarget", "CustomEvent", "ErrorEvent",
        "MessageChannel", "MessageEvent", "MessagePort", "PromiseRejectionEvent", "DOMException",
        # Fetch, XHR, File API, Compression
        "Headers", "Request", "Response", "FormData", "Blob", "File",
        "CompressionStream", "DecompressionStream",
        # Streams
        "ByteLengthQueuingStrategy", "CountQueuingStrategy", "ReadableByteStreamController",
        "ReadableStream", "ReadableStreamBYOBReader", "ReadableStreamBYOBRequest",
        "ReadableStreamDefaultController", "ReadableStreamDefaultReader", "TransformStream",
        "TransformStreamDefaultController", "WritableStream", "WritableStreamDefaultController",
        "WritableStreamDefaultWriter",
        # Encoding, URL, Crypto, hr-time, WebAssembly
        "TextDecoder", "TextDecoderStream", "TextEncoder", "TextEncoderStream",
        "URL", "URLSearchParams", "URLPattern", "Crypto", "CryptoKey", "SubtleCrypto",
        "Performance", "WebAssembly",
        # Global functions and properties
        "globalThis", "atob", "btoa", "clearTimeout", "clearInterval", "queueMicrotask",
        "reportError", "setTimeout", "setInterval", "structuredClone", "fetch", "self",
        "onerror", "onunhandledrejection", "onrejectionhandled",
        "navigator", "console", "crypto", "performance",
    ]]
    + [("WebAssembly", name) for name in [
        "Global", "Instance", "Memory", "Module", "Table", "Tag", "Exception", "CompileError",
        "LinkError", "RuntimeError", "compile", "compileStreaming", "instantiate",
        "instantiateStreaming", "validate", "JSTag",
    ]]
    + [("navigator", "userAgent"), ("crypto", "subtle"), ("crypto", "getRandomValues"),
       ("crypto", "randomUUID"), ("performance", "now"), ("performance", "timeOrigin")]
    + [("console", name) for name in [
        "assert", "clear", "count", "countReset", "debug", "dir", "dirxml", "error", "group",
        "groupCollapsed", "groupEnd", "info", "log", "table", "time", "timeEnd", "timeLog",
        "trace", "warn",
    ]]
)


def wintertc_rows(receivers: dict[str, Receiver]) -> list[tuple[str, str, str, str, str]]:
    """One row per Minimum Common API member: (name, node, interpreter, native, prototype gap).

    A reading is `present`, `missing`, or for the native backend `refused`; the prototype gap
    is how many of node's `X.prototype` members each engine lacks, for an interface both have,
    so a constructor that is there with an empty prototype does not read as done."""
    rows = []
    for key, name in WINTERTC_MINIMUM_COMMON_API:
        rec = receivers.get(key)
        label = name if key == "globalThis" else f"{key}.{name}"
        if rec is None:
            rows.append((label, "?", "missing", "missing", ""))
            continue
        node = rec.node.get(name)
        node_reading = "present" if node not in (None, "undefined") else "missing"
        interp = rec.interpreted.get(name)
        interp_reading = "present" if interp not in (None, "undefined") else "missing"
        native = rec.native.get(name)
        if name in rec.refused:
            native_reading = "refused"
        elif native not in (None, "undefined"):
            native_reading = "present"
        else:
            native_reading = "missing"
        gap = ""
        proto = receivers.get(f"{name}.prototype") if key == "globalThis" else None
        if proto is not None and proto.node_names:
            versus = against_node(proto)
            i = len(versus["interpreter"]["missing"])
            n = len(versus["native"]["missing"])
            if i or n:
                gap = f"{i} / {n} of {len(proto.node_names)}"
        rows.append((label, node_reading, interp_reading, native_reading, gap))
    return rows


def against_node(rec: Receiver) -> dict[str, dict[str, list[str]]]:
    """Each engine against node, per member: `missing` (node has it, the engine does not --
    for the native backend a refusal or an `undefined` both count), `differs` (both answer,
    differently), `extra` (the engine has it and node does not), `extension` (the engine has it,
    node does not, and it is one of kiln's own members, declared as such -- ADR 0068 -- so it is
    intentional rather than drift). A node-only receiver is missing in both engines, whole."""
    out = {
        "interpreter": {"missing": [], "differs": [], "extra": [], "extension": []},
        "native": {"missing": [], "differs": [], "extra": [], "extension": []},
    }
    if rec.node_names is None:
        return out
    node_names = set(rec.node_names)
    for name in sorted(node_names):
        want = rec.node[name]
        got = rec.interpreted.get(name)
        if got is None:
            out["interpreter"]["missing"].append(name)
        elif got != want and not (got == "throws" and want == "throws"):
            out["interpreter"]["differs"].append(name)
        native = rec.native.get(name)
        if native is None or native == "undefined" and want != "undefined":
            out["native"]["missing"].append(name)
        elif native != want:
            out["native"]["differs"].append(name)
    extensions = set(rec.kiln_extensions)
    for name in sorted(set(rec.interpreted) - node_names):
        out["interpreter"]["extension" if name in extensions else "extra"].append(name)
    for name in sorted(set(rec.native) - node_names):
        if rec.native[name] != "undefined":
            out["native"]["extension" if name in extensions else "extra"].append(name)
    return out


# --------------------------------------------------------------------------------------------
# Stage 2: put every name to the native backend, in rounds.
# --------------------------------------------------------------------------------------------

# Three readings in one compile, in the order that loses the least to a refusal. `T` is the
# receiver's own `typeof`, which every receiver has and nothing can refuse once the expression
# evaluates. `P` is its own key list, which refuses on most built-ins. `N` is its `.name`, put
# last because it is the only one of the three whose loss is repaired by a second compile below.
DESCRIBE_SELF = """
%(describe)s
var recv;
try { recv = %(expr)s; } catch (e) { recv = undefined; }
console.log("T\\t" + kilnDescribe(recv));
console.log("P\\t" + Object.getOwnPropertyNames(recv).sort().join(","));
console.log("N\\t" + kilnDescribe(recv.name));
console.log("#END");
"""

# The repair run: `typeof` again (free, and it re-proves the expression evaluates) and the name,
# with nothing between them that can refuse.
DESCRIBE_SELF_NAME = """
%(describe)s
var recv;
try { recv = %(expr)s; } catch (e) { recv = undefined; }
console.log("T\\t" + kilnDescribe(recv));
console.log("N\\t" + kilnDescribe(recv.name));
console.log("#END");
"""


def probe_published(runner: Runner, rec: Receiver) -> None:
    """What native says it *is*, and what it says its own surface is.

    ADR 0043 made `Object.getOwnPropertyNames` answer on a built-in constructor rather than refuse,
    and it turns out to answer on the prototypes and the `node:` namespaces too. That is cheap and
    it is a measurement in its own right: a name native publishes that the interpreter does not is
    an *invented* member, which no per-name read would ever have found because nothing would think
    to ask for it.

    The receiver's own `typeof` and `.name` ride along in the same compile, because they cost
    nothing here and because the walk that follows can only ever ask about members. That blind
    spot is what let `typeof require("node:module")` be "function" in one engine and "object" in
    the other while every member of it agreed: a report of an instrument that cannot see the
    receiver hides an unknown number of gaps, which is worse than a gap it names.
    """
    source = DESCRIBE_SELF % {"describe": DESCRIBE, "expr": rec.expr}
    lines, refusal = runner.run(
        source, native=True, node=rec.needs_node, tag=f"published-{safe_tag(rec.key)}"
    )
    published = None
    for line in lines:
        if line.startswith("T\t"):
            rec.self_native[SELF_TYPEOF] = line[2:]
        elif line.startswith("N\t"):
            rec.self_native[SELF_NAME] = line[2:]
        elif line.startswith("P\t"):
            published = sorted(n for n in line[2:].split(",") if n)
    if rec.self_native.get(SELF_TYPEOF) in ("undefined", "null"):
        # The receiver is not there natively -- `globalThis.CloseEvent` is `undefined` -- which
        # is the measurement, whole: every member of it is missing and there is nothing to read
        # `.name` or the key list off. The TypeError the rest of this compile then raised was
        # being retried three times and filed as a run that was not a measurement.
        rec.unreachable = f"native: receiver is {rec.self_native[SELF_TYPEOF]}"
        rec.native_names = None
        rec.self_description = "the receiver is absent natively"
        for tag in list(runner.errors):
            if tag.startswith(f"published-{safe_tag(rec.key)}:"):
                runner.errors.remove(tag)
        return
    if published is not None:
        rec.native_names = published
    else:
        # Measured, not assumed: `Object.getOwnPropertyNames` answers on the fifteen built-ins ADR
        # 0043 put on the table and on the prototypes and namespaces, and refuses on the rest.
        # Where it refuses the backend cannot describe itself, so an invented member on that
        # receiver is invisible to this tool and the report says so.
        rec.native_names = None
        rec.self_description = refusal or "no answer"
    if SELF_NAME in rec.self_native:
        return
    # The name was lost to whatever refused ahead of it, which is a refusal about *that* reading
    # and not about this one. Asking again without it is one compile and keeps a refusal on the
    # key list from being filed as a refusal on the name.
    lines, name_refusal = runner.run(
        DESCRIBE_SELF_NAME % {"describe": DESCRIBE, "expr": rec.expr},
        native=True,
        node=rec.needs_node,
        tag=f"self-name-{safe_tag(rec.key)}",
    )
    for line in lines:
        if line.startswith("T\t"):
            rec.self_native[SELF_TYPEOF] = line[2:]
        elif line.startswith("N\t"):
            rec.self_native[SELF_NAME] = line[2:]
    if SELF_NAME not in rec.self_native and name_refusal is not None:
        rec.self_refused[SELF_NAME] = name_refusal
    if SELF_TYPEOF not in rec.self_native and name_refusal is not None:
        rec.self_refused[SELF_TYPEOF] = name_refusal


def safe_tag(key: str) -> str:
    return re.sub(r"[^A-Za-z0-9]+", "-", key)


def probe_values(runner: Runner, rec: Receiver, max_rounds: int) -> None:
    """Read every name the interpreter publishes, in as many rounds as there are refusals.

    Each round prints one line per name until the backend refuses. Everything printed is an
    answer; the refusal names what it refused, and the next round asks for the rest. This is the
    only shape that works, because a refusal is not catchable and takes the process with it.
    """
    # The union of what the interpreter and node publish: a member only node has is put to the
    # native backend too, so "missing against node" is a measurement and not an inference from
    # the interpreter's list. `classify` still reads the interpreter's names; a native answer for
    # a name the interpreter lacks is *invented* there and *present* against node.
    if (rec.unreachable or "").startswith("native: receiver is"):
        return
    node_only_names = set(rec.node_names or []) - set(rec.names)
    if rec.native_names is not None:
        # A refusal costs one compile per name, and a receiver that can list its own keys has
        # already said which of node's names it lacks: only the ones it claims are read.
        node_only_names &= set(rec.native_names)
    pending = sorted(set(rec.names) | node_only_names)
    if rec.native_names:
        # Ask for what native publishes first. Those reads answer, so one round harvests all of
        # them and the rounds that follow are spent only on names that are actually missing.
        published = set(rec.native_names)
        pending.sort(key=lambda n: (n not in published, n))
    rounds = 0
    while pending and rounds < max_rounds:
        rounds += 1
        source = build_read_program(rec.expr, pending)
        lines, refusal = runner.run(
            source, native=True, node=rec.needs_node, tag=f"read-{safe_tag(rec.key)}-{rounds}"
        )
        for line in lines:
            parts = line.split("\t")
            if len(parts) == 3 and parts[0] == "A":
                rec.native[parts[1]] = parts[2]
            elif len(parts) >= 2 and parts[0] == "R":
                rec.unreachable = f"native: receiver is {parts[1]}"
                return
        remaining = [n for n in pending if n not in rec.native]
        if refusal is None:
            pending = remaining
            break
        if not remaining:
            break
        if "operation=" in refusal:
            # A refusal at an instruction. Everything before it printed, so the member is either
            # named in the diagnostic or is simply the first one that did not print --- `@@` reads
            # lower to `GetComputed`, which refuses without saying what it refused.
            named = REFUSED_MEMBER.search(refusal)
            culprit = (
                named.group(1)
                if named is not None and named.group(1) in remaining
                else remaining[0]
            )
        else:
            # A refusal *before* emission, which is a different animal: nothing ran, so how far
            # the output got says nothing about which member caused it. `globalThis.ShadowRealm`
            # is one of these, and taking the first pending name would have blamed
            # `globalThis.AbortController` for it.
            culprit = emission_culprit(runner, rec, remaining, refusal)
            if culprit is None:
                # The receiver itself cannot be emitted --- an unmaterialized `node:` builtin, or
                # a facility the backend has no representation for. Every member is refused, and
                # for one reason rather than N.
                for name in remaining:
                    rec.refused.setdefault(name, refusal)
                rec.unreachable = f"native: {refusal}"
                return
        rec.refused[culprit] = refusal
        pending = [n for n in remaining if n != culprit]
        continue
    if pending and rounds >= max_rounds:
        for name in pending:
            rec.refused.setdefault(name, "not probed: round budget exhausted")
    return


def emission_culprit(
    runner: Runner, rec: Receiver, remaining: list[str], refusal: str
) -> str | None:
    """Which member stopped the program from being emitted at all, if it is one member.

    Cheapest first: emission diagnostics name the facility they cannot emit, and that spelling is
    usually the member's own (`host realm facility ShadowRealm`). Failing that, ask whether the
    receiver alone can be emitted --- if it cannot, no member is to blame and the answer is None.
    Failing *that*, bisect, which costs a logarithmic number of compiles rather than one per name.
    """
    for name in remaining:
        if name in refusal:
            return name
    _, bare_refusal = runner.run(
        build_read_program(rec.expr, []),
        native=True,
        node=rec.needs_node,
        tag=f"bare-{safe_tag(rec.key)}",
    )
    if bare_refusal is not None:
        return None
    lo = remaining
    while len(lo) > 1:
        half = lo[: len(lo) // 2]
        _, half_refusal = runner.run(
            build_read_program(rec.expr, half),
            native=True,
            node=rec.needs_node,
            tag=f"bisect-{safe_tag(rec.key)}-{len(lo)}",
        )
        lo = half if half_refusal is not None else lo[len(lo) // 2 :]
    return lo[0] if lo else None


def build_read_program(expr: str, names: list[str]) -> str:
    out = [DESCRIBE]
    out.append("var recv;")
    out.append(f"try {{ recv = {expr}; }} catch (e) {{ recv = undefined; }}")
    out.append('if (recv === undefined || recv === null) { console.log("R\\t" + kilnDescribe(recv)); }')
    out.append("else {")
    for name in names:
        access = member_access("recv", name)
        literal = js_string(name)
        out.append(
            f"  try {{ console.log(\"A\\t\" + {literal} + \"\\t\" + kilnDescribe({access})); }}"
            f" catch (e) {{ console.log(\"A\\t\" + {literal} + \"\\tthrows\"); }}"
        )
    out.append("}")
    out.append('console.log("#END");')
    return "\n".join(out)


# --------------------------------------------------------------------------------------------
# Disk discipline. A run that hits ENOSPC is not a measurement.
# --------------------------------------------------------------------------------------------

RECLAIMABLE = ["generated-c", "native-object", "linked-artifact"]


def guard_disk(root: Path, floor_gib: float, stop_gib: float, always: bool = False) -> None:
    """Give back the build scratch this tool just made.

    Every round is a program no other program is ever byte-identical to, so nothing in
    `.kiln/cache` written by this tool is ever read again --- the content-addressed cache cannot
    hit on a unique input. Holding gigabytes of it for the length of a run buys nothing and takes
    space away from whoever else is on this volume, so the default is to hand it back after each
    receiver rather than to wait for a floor. The floor stays as the hard stop: below it the tool
    reclaims and, if that is still not enough, refuses to keep going, because a run that ends in
    ENOSPC is not a measurement.
    """
    free = shutil.disk_usage("/").free / (1 << 30)
    if free >= floor_gib and not always:
        return
    # Every native compile leaves a C file, an object and a linked artifact behind, and this tool
    # produces one per round. They are content-addressed scratch and regenerate; the runtime
    # library is left alone because rebuilding it is the expensive half.
    cache = root / ".kiln" / "cache"
    for name in RECLAIMABLE:
        shutil.rmtree(cache / name, ignore_errors=True)
    free = shutil.disk_usage("/").free / (1 << 30)
    if not always:
        print(f"  reclaimed native build scratch; {free:.1f} GiB free", file=sys.stderr)
    if free < stop_gib:
        sys.exit(
            f"only {free:.1f} GiB free after handing back every byte this run made: stopping "
            "rather than filing a measurement that ended in ENOSPC. The space is somebody "
            "else's to reclaim; never `mbx gc` on their behalf."
        )


# --------------------------------------------------------------------------------------------
# Stage 3: classify and report.
# --------------------------------------------------------------------------------------------


def self_rows(rec: Receiver) -> list[str]:
    """Which of the receiver's own readings this receiver contributes a row for.

    `typeof` always: every receiver is a value, and the two engines either agree about what kind
    of value it is or they do not. `.name` only where one of the engines answers something --- a
    row saying both agree that a namespace object has no name is not a measurement of anything,
    and 148 of them would bury the ones that matter.
    """
    rows = [SELF_TYPEOF]
    answers = {rec.self_interpreted.get(SELF_NAME), rec.self_native.get(SELF_NAME)}
    if answers - {None, "undefined"} or SELF_NAME in rec.self_refused:
        rows.append(SELF_NAME)
    return rows


def row_count(rec: Receiver) -> int:
    """How many rows this receiver contributes: its members, plus its own readings.

    The classes below have to add up to this, so the receiver's own rows are counted here rather
    than left out of the denominator they are already in the numerator of.
    """
    return len(rec.names) + sum(1 for row in self_rows(rec) if row in rec.self_interpreted)


def classify(rec: Receiver) -> dict[str, list[tuple[str, str, str]]]:
    out: dict[str, list[tuple[str, str, str]]] = {
        "diverged": [],
        "refused": [],
        "implemented": [],
        "invented": [],
        "unmeasured": [],
    }
    # What the receiver itself is, classified exactly like a member of it. A receiver that is a
    # function in one engine and an object in the other is the same class of wrong answer as a
    # member that is --- `typeof require("node:module") === "function"` is a real feature test --
    # and it is one the walk below can never reach, because the walk only asks about members.
    for row in self_rows(rec):
        want = rec.self_interpreted.get(row)
        if want is None:
            continue
        if row in rec.self_refused:
            out["refused"].append((row, want, rec.self_refused[row]))
        elif row in rec.self_native:
            got = rec.self_native[row]
            out["implemented" if got == want else "diverged"].append((row, want, got))
        else:
            out["unmeasured"].append((row, want, "not probed: no answer and no refusal"))
    # A receiver that reads `undefined` natively is not a receiver that refuses. Every member on
    # it would throw `undefined is not an object` at run time, which is the wrong-answer cascade
    # and not the honest stop, so they are counted with the divergence that caused them rather
    # than swelling the backlog with sixteen rows nobody can act on separately.
    absent_receiver = (rec.unreachable or "").startswith("native: receiver is")
    for name in rec.names:
        want = rec.interpreted.get(name, "?")
        if absent_receiver and name not in rec.native and name not in rec.refused:
            out["diverged"].append((name, want, f"unreachable, {rec.unreachable[8:]}"))
        elif name in rec.refused:
            reason = rec.refused[name]
            out["unmeasured" if reason.startswith("not probed") else "refused"].append(
                (name, want, reason)
            )
        elif name in rec.native:
            got = rec.native[name]
            if got == want:
                out["implemented"].append((name, want, got))
            else:
                out["diverged"].append((name, want, got))
        else:
            out["refused"].append((name, want, "not reached"))
    if rec.native_names is not None:
        for name in rec.native_names:
            if name not in rec.interpreted:
                out["invented"].append((name, "absent", rec.native.get(name, "published")))
    return out


# The silent-undefined rows this repository has looked at and decided to keep, with the reason.
#
# A row is an exemption from the one class of gap that fails a run, so it costs a sentence. Two
# things make a row legitimate: the native answer is right *for the object it actually hands back*,
# or the divergence is a shape the two engines disagree about at a level below the member. Anything
# else is a bug, and the fix is the backend rather than a row here.
SILENT_UNDEFINED_ACCEPTED: dict[tuple[str, str], str] = {
    ("BigInt64Array^", "@@species"): (
        "the receiver is not the same object in the two engines. `Object.getPrototypeOf("
        "BigInt64Array)` is the %TypedArray% constructor intrinsic in the interpreter and "
        "`Function.prototype` natively, and `Function.prototype[Symbol.species]` really is "
        "undefined. The gap is the missing intrinsic in the prototype graph, not this member"
    ),
    ("BigInt64Array^", "(itself).name"): (
        "the receiver is not the same object in the two engines. `Object.getPrototypeOf("
        "BigInt64Array)` is the %TypedArray% constructor intrinsic in the interpreter and "
        "`Function.prototype` natively, which really has no own `(itself).name`. The gap is the missing "
        "intrinsic in the prototype graph, not this member; and the interpreter's `name` for it, "
        "\"intrinsic\", is not node's \"TypedArray\" either"
    ),
    ("BigInt64Array^", "length"): (
        "the receiver is not the same object in the two engines. `Object.getPrototypeOf("
        "BigInt64Array)` is the %TypedArray% constructor intrinsic in the interpreter and "
        "`Function.prototype` natively, which really has no own `length`. The gap is the missing "
        "intrinsic in the prototype graph, not this member; and the interpreter's `name` for it, "
        "\"intrinsic\", is not node's \"TypedArray\" either"
    ),
    ("BigInt64Array^", "name"): (
        "the receiver is not the same object in the two engines. `Object.getPrototypeOf("
        "BigInt64Array)` is the %TypedArray% constructor intrinsic in the interpreter and "
        "`Function.prototype` natively, which really has no own `name`. The gap is the missing "
        "intrinsic in the prototype graph, not this member; and the interpreter's `name` for it, "
        "\"intrinsic\", is not node's \"TypedArray\" either"
    ),
    ("BigInt64Array^", "prototype"): (
        "the receiver is not the same object in the two engines. `Object.getPrototypeOf("
        "BigInt64Array)` is the %TypedArray% constructor intrinsic in the interpreter and "
        "`Function.prototype` natively, which really has no own `prototype`. The gap is the missing "
        "intrinsic in the prototype graph, not this member; and the interpreter's `name` for it, "
        "\"intrinsic\", is not node's \"TypedArray\" either"
    ),
}


def gate_rows(receivers: dict[str, Receiver], order: list[str]) -> list[tuple[str, str, str, str]]:
    """Every member that reads `undefined` natively where the interpreter has a value.

    A receiver that is itself `undefined` natively counts too, and counts once per member it
    swallows: each of those reads would throw `undefined is not an object` at run time, which is
    the wrong-answer cascade rather than the honest stop.
    """
    found = []
    for key in order:
        for name, want, got in classify(receivers[key])["diverged"]:
            if got == "undefined" or got.startswith("unreachable, receiver is undefined"):
                if want == "undefined":
                    continue
                found.append((key, name, want, got))
    return found


def report_gate(receivers: dict[str, Receiver], order: list[str]) -> int:
    rows = gate_rows(receivers, order)
    unexplained = [row for row in rows if (row[0], row[1]) not in SILENT_UNDEFINED_ACCEPTED]
    for key, name, want, _ in rows:
        if (key, name) in SILENT_UNDEFINED_ACCEPTED:
            print(f"accepted: {key}.{name} ({SILENT_UNDEFINED_ACCEPTED[(key, name)]})", file=sys.stderr)
    if not unexplained:
        print(f"gate: no silent undefined ({len(rows)} accepted)", file=sys.stderr)
        return 0
    print(
        f"GATE FAILED: {len(unexplained)} member(s) read `undefined` in the native backend where "
        f"the interpreter has a value. That is not a gap a program stops at -- it is the branch "
        f"`typeof x.member === \"function\"` takes when the answer is nothing:",
        file=sys.stderr,
    )
    for key, name, want, got in unexplained:
        print(f"  {key}.{name}: interpreter {want}, native {got}", file=sys.stderr)
    print(
        "Answer it, refuse it by name, or add a SILENT_UNDEFINED_ACCEPTED row saying why the "
        "native answer is right for the object it hands back.",
        file=sys.stderr,
    )
    return 1


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--kiln", default="target/release/kiln")
    parser.add_argument("--jobs", type=int, default=4)
    parser.add_argument("--timeout", type=int, default=180)
    parser.add_argument("--max-rounds", type=int, default=64)
    parser.add_argument("--disk-floor-gib", type=float, default=4.0)
    parser.add_argument("--disk-stop-gib", type=float, default=1.5)
    parser.add_argument("--out", default="docs/issues/native-real-libraries")
    # The measurement and the prose are separated on purpose: rewording the report, or adding a
    # cut of the same numbers, must never cost another hundred native compiles.
    parser.add_argument("--stage", choices=["enumerate", "report", "all"], default="all")
    parser.add_argument("--only", default=None, help="probe only receivers matching this substring")
    parser.add_argument(
        "--gate",
        action="store_true",
        help="exit non-zero when a member reads `undefined` natively and has a value in the "
        "interpreter",
    )
    args = parser.parse_args()

    # A slice must never overwrite the whole inventory. `--only WeakRef` once replaced a
    # 591-compile report with a four-member one, and there is no backup of a measurement.
    stem = "native-surface-inventory" + (".partial" if args.only else "")

    root = Path(__file__).resolve().parent.parent
    kiln = (root / args.kiln).resolve()
    if not kiln.exists():
        sys.exit(f"no kiln binary at {kiln}: build it with `mbx build --release -p kiln-cli`")
    outdir = root / args.out
    outdir.mkdir(parents=True, exist_ok=True)

    if args.stage == "report":
        saved = json.loads((outdir / f"{stem}.json").read_text())
        receivers = {}
        for key, row in saved["receivers"].items():
            rec = Receiver(key, row["kind"], row["expr"], key.startswith("node:"))
            rec.interpreted = row["interpreted"]
            rec.names = sorted(row["interpreted"])
            rec.native = row["native"]
            rec.refused = row["refused"]
            rec.native_names = row["native_names"]
            rec.self_description = row.get("self_description")
            rec.self_interpreted = row.get("self_interpreted", {})
            rec.self_native = row.get("self_native", {})
            rec.self_refused = row.get("self_refused", {})
            rec.unreachable = row["unreachable"]
            rec.node = row.get("node", {})
            rec.node_names = row.get("node_names")
            rec.self_node = row.get("self_node", {})
            rec.node_only = row.get("node_only", False)
            rec.kiln_extensions = row.get("kiln_extensions", [])
            receivers[key] = rec
        runner = Runner(kiln, root, root, args.timeout)
        runner.errors = saved.get("errors", [])
        runner.native_runs = saved.get("native_runs", 0)
        (outdir / f"{stem}.md").write_text(
            render(receivers, sorted(receivers), runner)
        )
        print(f"re-rendered {outdir / (stem + '.md')}", file=sys.stderr)
        return report_gate(receivers, sorted(receivers)) if args.gate else 0

    with tempfile.TemporaryDirectory(dir=str(root / ".scratch"), prefix="surface-") as tmp, built_outputs(root, Path(tmp)):
        runner = Runner(kiln, root, Path(tmp), args.timeout)
        print("enumerating the interpreter's surface ...", file=sys.stderr)
        receivers = enumerate_interpreter(runner)
        print(f"enumerating node's ({node_version()}) ...", file=sys.stderr)
        enumerate_node(runner, receivers)
        order = sorted(receivers)
        if args.only:
            order = [k for k in order if args.only in k]
        members = sum(row_count(receivers[k]) for k in order)
        print(f"  {len(order)} receivers, {members} members", file=sys.stderr)
        if args.stage == "enumerate":
            for key in order:
                print(f"{key}\t{len(receivers[key].names)}")
            return 0

        # Node-only receivers are probed too: the interpreter not publishing `URL.prototype` says
        # nothing about whether the native backend does.
        todo = [receivers[k] for k in order if receivers[k].unreachable is None]
        print(f"asking native what it publishes ({len(todo)} runs) ...", file=sys.stderr)
        guard_disk(root, args.disk_floor_gib, args.disk_stop_gib)
        with ThreadPoolExecutor(max_workers=args.jobs) as pool:
            list(pool.map(lambda r: probe_published(runner, r), todo))

        print("reading every member natively, in rounds ...", file=sys.stderr)
        done = [0]

        def probe(rec: Receiver) -> None:
            probe_values(runner, rec, args.max_rounds)
            done[0] += 1
            print(
                f"  [{done[0]}/{len(todo)}] {rec.key}: "
                f"{len(rec.native)} answered, {len(rec.refused)} refused",
                file=sys.stderr,
            )
            guard_disk(root, args.disk_floor_gib, args.disk_stop_gib, always=True)

        with ThreadPoolExecutor(max_workers=args.jobs) as pool:
            list(pool.map(probe, todo))

    data = {
        key: {
            "kind": receivers[key].kind,
            "expr": receivers[key].expr,
            "unreachable": receivers[key].unreachable,
            "interpreted": receivers[key].interpreted,
            "native": receivers[key].native,
            "refused": receivers[key].refused,
            "native_names": receivers[key].native_names,
            "self_description": receivers[key].self_description,
            "self_interpreted": receivers[key].self_interpreted,
            "self_native": receivers[key].self_native,
            "self_refused": receivers[key].self_refused,
            "node": receivers[key].node,
            "node_names": receivers[key].node_names,
            "self_node": receivers[key].self_node,
            "node_only": receivers[key].node_only,
            "kiln_extensions": receivers[key].kiln_extensions,
        }
        for key in order
    }
    (outdir / f"{stem}.json").write_text(
        json.dumps(
            {"receivers": data, "errors": runner.errors, "native_runs": runner.native_runs,
             "node_version": node_version()},
            indent=1,
            sort_keys=True,
        )
    )
    (outdir / f"{stem}.md").write_text(render(receivers, order, runner))
    print(
        f"wrote {outdir / (stem + '.md')} ({runner.native_runs} native runs)",
        file=sys.stderr,
    )
    return report_gate(receivers, order) if args.gate else 0


def describe_tree() -> str:
    """The commit this was measured at, and whether the tree was dirty.

    A number taken from a tree something else is writing is a clock reading rather than a
    measurement, and this repository has a day in `NOTES.md` to prove it. Saying which commit and
    whether it was clean is what makes the report re-checkable a week later.
    """
    try:
        head = subprocess.run(
            ["git", "rev-parse", "--short", "HEAD"], capture_output=True, text=True, check=True
        ).stdout.strip()
        dirty = subprocess.run(
            ["git", "status", "--porcelain", "--", "crates"],
            capture_output=True,
            text=True,
            check=True,
        ).stdout.strip()
    except (subprocess.CalledProcessError, OSError):
        return "an unknown commit"
    return head + (" with uncommitted engine changes under crates/" if dirty else ", crates/ clean")


def render(receivers: dict[str, Receiver], order: list[str], runner: Runner) -> str:
    per = {key: classify(receivers[key]) for key in order}
    classes = ("diverged", "refused", "implemented", "invented", "unmeasured")
    totals = {k: sum(len(per[key][k]) for key in order) for k in classes}
    members = sum(row_count(receivers[key]) for key in order)

    lines: list[str] = []
    add = lines.append
    add("# What the native backend does not implement")
    add("")
    add(
        "Generated by `scripts/native-surface-inventory.py`. Re-run it after a backend change and "
        "diff this file. Nothing here is hand-written and no built-in is named in the tool: both "
        "engines were asked."
    )
    add("")
    add(f"Measured at `{describe_tree()}`.")
    add("")
    add("## Headline")
    add("")
    add("| Class | Rows | Share |")
    add("| --- | ---: | ---: |")
    for name, label in (
        ("diverged", "**diverged** (native answered, and answered differently)"),
        ("refused", "refused by name (honest backlog)"),
        ("implemented", "implemented (the two engines agree)"),
    ):
        share = 100.0 * totals[name] / members if members else 0.0
        add(f"| {label} | {totals[name]} | {share:.1f}% |")
    if totals["unmeasured"]:
        add(f"| *unmeasured* — a run that failed for a reason that was not a refusal | {totals['unmeasured']} | |")
    add(f"| *(memo)* invented — published natively, absent in the interpreter | {totals['invented']} | |")
    add(f"| **total** | {members} | |")
    add("")
    add(
        f"{len(order)} receivers: the global object, every built-in's statics, every built-in "
        f"prototype, the object each of those inherits from, and the `node:` namespaces. "
        f"{runner.native_runs} native compiles."
    )
    add("")
    # ---- the far gap: each engine against node --------------------------------------------
    versus = {key: against_node(receivers[key]) for key in order}
    measured = [key for key in order if receivers[key].node_names is not None]
    node_only = [key for key in order if receivers[key].node_only]
    node_members = sum(len(receivers[key].node_names or []) for key in measured)
    node_only_members = sum(len(receivers[key].node_names or []) for key in node_only)
    add("## Against node")
    add("")
    add(
        f"Node ({node_version()}) is the target of both engines (ADR 0063); the classes above "
        "compare the two engines with each other, which is the near gap. This is the far one: "
        "the same enumeration run under node, and each engine held to it. *Missing* is a member "
        "node has and the engine does not (natively, a refusal or an `undefined` both count); "
        "*differs* is both answering and not alike; *extra* is a member the engine publishes and "
        "node does not; a *kiln extension* is one of those that kiln declares as its own (ADR "
        "0068) -- intentional, non-enumerable, and gone under `KILN_EXTENSIONS=0` -- so it is "
        "counted apart and never as drift."
    )
    add("")
    add("| | Interpreter | Native |")
    add("| --- | ---: | ---: |")
    for cls, label in (
        ("missing", "missing (node has it)"),
        ("differs", "differs from node"),
        ("extra", "extra (node lacks it)"),
        ("extension", "kiln extension (ADR 0068; intentional)"),
    ):
        i = sum(len(versus[k]["interpreter"][cls]) for k in measured)
        n = sum(len(versus[k]["native"][cls]) for k in measured)
        add(f"| {label} | {i} | {n} |")
    add(f"| node members on shared receivers | {node_members} | {node_members} |")
    add(f"| receivers the interpreter does not publish | {len(node_only)} ({node_only_members} members) | measured above |")
    add("")
    add("Farthest from node, by members missing or differing; the interpreter leads and the native backend follows it, so a row where native is farther is a member native has not built yet, and a row where they are equal is the interpreter's own gap.")
    add("")
    add("| Receiver | Node members | Interpreter missing | Interpreter differs | Native missing | Native differs |")
    add("| --- | ---: | ---: | ---: | ---: | ---: |")
    ranked = sorted(
        measured,
        key=lambda k: (-(len(versus[k]["interpreter"]["missing"]) + len(versus[k]["interpreter"]["differs"]) + len(versus[k]["native"]["missing"]) + len(versus[k]["native"]["differs"])), k),
    )
    shown = 0
    for key in ranked:
        v = versus[key]
        total = sum(len(v[e][c]) for e in ("interpreter", "native") for c in ("missing", "differs"))
        if total == 0 or shown >= 30:
            break
        shown += 1
        add(f"| `{key}` | {len(receivers[key].node_names or [])} | {len(v['interpreter']['missing'])} | {len(v['interpreter']['differs'])} | {len(v['native']['missing'])} | {len(v['native']['differs'])} |")
    add("")
    if node_only:
        add("Receivers node has and the interpreter does not publish (its list taken with the node adapter too), with node's member counts; the native backend was asked about each, and its answers are in the table above:")
        add("")
        add(", ".join(f"`{key}` ({len(receivers[key].node_names or [])})" for key in node_only))
        add("")
    extensions = sorted(
        {(k, n) for k in measured for e in ("interpreter", "native") for n in versus[k][e]["extension"]}
    )
    if extensions:
        add("kiln extensions (ADR 0068): members kiln declares as its own, published by both engines by default and removed by `KILN_EXTENSIONS=0` or `kiln build --no-kiln-extensions`:")
        add("")
        add(", ".join(f"`{k}.{n}`" for k, n in extensions))
        add("")
    extra_i = [(k, n) for k in measured for n in versus[k]["interpreter"]["extra"]]
    if extra_i:
        add("Members the interpreter publishes and node does not (node is the target; each of these is either a deliberate extension with a reason, or debt):")
        add("")
        add(", ".join(f"`{k}.{n}`" for k, n in extra_i[:60]) + (" ..." if len(extra_i) > 60 else ""))
        add("")
    add(
        "A receiver key is the path the walk took. `Array.prototype` is what it says; a trailing "
        "`^` means *the object this one inherits from*, which is the only way the shared "
        "prototypes get inventoried --- every typed array's own prototype has two members and the "
        "thirty-five that matter live one link up, on an object no name on the global object "
        "reaches. Every receiver is probed under the Node adapter, which is what a program runs "
        "with and what node itself is."
    )
    add("")
    self_total = sum(
        1 for key in order for cls in per[key].values() for name, _, _ in cls
        if name in (SELF_TYPEOF, SELF_NAME)
    )
    add(
        f"`(itself)` and `(itself).name` are rows about the receiver rather than about a member "
        f"of it, and there are {self_total} of them. They exist because the walk cannot ask them: "
        "it enumerates members, so a receiver that is a *function* in one engine and an *object* "
        "in the other agrees on every member it has and diverges on what it is. "
        "`typeof require(\"node:module\") === \"function\"` is a real feature test, and it read "
        "two ways in the two engines for months with every member of the module agreeing. A "
        "`.name` row appears only where one of the engines answers a name; a receiver both engines "
        "agree has none is not a measurement of anything."
    )
    add("")

    add("## WinterTC Minimum Common API")
    add("")
    add(
        "The interfaces and globals WinterTC (Ecma TC55) says every server-side runtime shares "
        "(https://min-common-api.proposal.wintertc.org/), read off the same measurement. "
        "*present* is a member that answers something other than `undefined`; the last column "
        "is how many of node's members on `X.prototype` each engine lacks, for an interface both "
        "have, so a constructor with a hollow prototype does not read as done. Conformance beyond "
        "presence is the Web Platform Tests' to say (`scripts/wpt.sh`)."
    )
    add("")
    wintertc = wintertc_rows(receivers)
    in_node = [r for r in wintertc if r[1] == "present"]
    add("| | Node | Interpreter | Native |")
    add("| --- | ---: | ---: | ---: |")
    add(
        f"| present, of {len(wintertc)} named | {len(in_node)} | "
        f"{sum(1 for r in wintertc if r[2] == 'present')} | "
        f"{sum(1 for r in wintertc if r[3] == 'present')} |"
    )
    add(
        f"| missing where node has it | | "
        f"{sum(1 for r in in_node if r[2] != 'present')} | "
        f"{sum(1 for r in in_node if r[3] != 'present')} |"
    )
    add("")
    add("| Member | Node | Interpreter | Native | `.prototype` missing (interp / native) |")
    add("| --- | --- | --- | --- | --- |")
    for label, node_r, interp_r, native_r, gap in wintertc:
        if node_r == "present" and interp_r == "present" and native_r == "present" and not gap:
            continue
        add(f"| `{label}` | {node_r} | {interp_r} | {native_r} | {gap} |")
    add("")
    add("Rows where node, the interpreter and the native backend all have the member with no prototype gap are left out; everything else is listed.")
    add("")
    add("## What to do first")
    add("")
    add(
        "Generated from the counts below, not from anybody's sense of priority. Divergence "
        "outranks refusal at every size, because a refusal stops a program at the cause and a "
        "wrong answer does not stop it at all."
    )
    add("")
    worst_diverged = sorted(
        (k for k in order if per[k]["diverged"]), key=lambda k: (-len(per[k]["diverged"]), k)
    )[:5]
    worst_refused = sorted(
        (k for k in order if per[k]["refused"]), key=lambda k: (-len(per[k]["refused"]), k)
    )[:5]
    for key in worst_diverged:
        n = len(per[key]["diverged"])
        add(f"1. **`{key}` — {n} wrong answers** out of {row_count(receivers[key])} rows.")
    for key in worst_refused:
        n = len(per[key]["refused"])
        add(f"1. `{key}` — {n} refusals out of {row_count(receivers[key])} rows.")
    if not worst_diverged and not worst_refused:
        add("Nothing is missing.")
    add("")

    add("## Diverged — a wrong answer, not a stop")
    add("")
    add(
        "These are the ones that cost money. The backend answers, so nothing stops; a member that "
        "reads `undefined` where the interpreter has a function is exactly what `typeof x === "
        '"undefined"` feature-detection walks past.'
    )
    add("")
    silent = gate_rows(receivers, order)
    unexplained = [row for row in silent if (row[0], row[1]) not in SILENT_UNDEFINED_ACCEPTED]
    add(
        f"**{len(silent)} of these read `undefined`**, which is the class "
        "`scripts/native-surface-inventory.py --gate` fails on: "
        f"{len(unexplained)} unexplained, {len(silent) - len(unexplained)} with a "
        "`SILENT_UNDEFINED_ACCEPTED` row saying why the native answer is right for the object it "
        "hands back. The rest of this report is read; that line is enforced."
    )
    add("")
    add("| Receiver | Member | Interpreter | Native |")
    add("| --- | --- | --- | --- |")
    rows = []
    for key in order:
        for name, want, got in per[key]["diverged"]:
            rows.append((key, name, want, got))
    for key, name, want, got in sorted(rows, key=lambda r: (r[0], r[1])):
        add(f"| `{key}` | `{name}` | {want} | **{got}** |")
    if not rows:
        add("| *(none)* | | | |")
    add("")

    if totals["invented"]:
        add("## Invented — published natively, absent in the interpreter")
        add("")
        add(
            "A name in `Object.getOwnPropertyNames` that the interpreter does not have. No per-name "
            "read would have found these, because nothing would have thought to ask."
        )
        add("")
        add("| Receiver | Member |")
        add("| --- | --- |")
        for key in order:
            for name, _, _ in per[key]["invented"]:
                add(f"| `{key}` | `{name}` |")
        add("")

    add("## Where the gap is concentrated")
    add("")
    add("Receivers with anything missing, worst first. `missing` is diverged plus refused.")
    add("")
    add("| Receiver | Kind | Rows | Diverged | Refused | Implemented | Missing |")
    add("| --- | --- | ---: | ---: | ---: | ---: | ---: |")
    ranked = sorted(
        order,
        key=lambda k: (
            -(len(per[k]["diverged"]) + len(per[k]["refused"])),
            -len(per[k]["diverged"]),
            k,
        ),
    )
    for key in ranked:
        d, r, i = len(per[key]["diverged"]), len(per[key]["refused"]), len(per[key]["implemented"])
        if d + r == 0:
            continue
        add(
            f"| `{key}` | {receivers[key].kind} | {row_count(receivers[key])} | {d} | {r} | {i} | "
            f"{d + r} |"
        )
    add("")

    add("## The same member missing everywhere")
    add("")
    add(
        "A name missing from many receivers at once is one decision, not many gaps, and it is the "
        "cheapest thing to close. Members missing from three or more receivers:"
    )
    add("")
    add("| Member | Receivers missing it | Of which diverged |")
    add("| --- | ---: | ---: |")
    across: dict[str, list[str]] = {}
    across_diverged: dict[str, int] = {}
    for key in order:
        for name, _, _ in per[key]["refused"]:
            across.setdefault(name, []).append(key)
        for name, _, _ in per[key]["diverged"]:
            across.setdefault(name, []).append(key)
            across_diverged[name] = across_diverged.get(name, 0) + 1
    ranked_members = sorted(across.items(), key=lambda kv: (-len(kv[1]), kv[0]))
    for name, keys in ranked_members:
        if len(keys) < 3:
            continue
        add(f"| `{name}` | {len(keys)} | {across_diverged.get(name, 0)} |")
    add("")
    missing_total = totals["diverged"] + totals["refused"]
    top4 = sum(len(keys) for _, keys in ranked_members[:4])
    if missing_total:
        top_names = ", ".join(f"`{name}`" for name, _ in ranked_members[:4])
        add(
            f"The four rows at the top of that table --- {top_names} --- are {top4} of the "
            f"{missing_total} missing, which is {100.0 * top4 / missing_total:.0f}% of everything "
            "this inventory found. Each of them is one decision rather than N omissions: what a "
            "built-in callable *is* (its own `length` and `prototype`, which is also what makes a "
            "receiver unreachable and takes its `(itself)` row with it), and what a built-in is "
            "*called*. The ordering follows: nothing else on this page buys as much."
        )
        add("")

    add("## Fully implemented receivers")
    add("")
    whole = [
        k
        for k in order
        if not per[k]["diverged"] and not per[k]["refused"] and not per[k]["unmeasured"] and receivers[k].names
    ]
    add(", ".join(f"`{k}`" for k in whole) if whole else "*(none)*")
    add("")

    unreachable = [k for k in order if receivers[k].unreachable]
    if unreachable:
        add("## Receivers the probe could not reach")
        add("")
        for key in unreachable:
            add(f"- `{key}` — {receivers[key].unreachable}")
        add("")

    mute = [k for k in order if receivers[k].self_description]
    add("## What this cannot see")
    add("")
    add(
        "- **A function is only ever `function`.** The description both engines print is `typeof` "
        "plus, for a primitive, its value. It deliberately does not read `.name` or `.length` off "
        "the value to tell two functions apart, because those are members this very inventory "
        "finds missing on dozens of receivers --- the scaffolding would report its own gap and "
        "call it the member's. Two different functions therefore compare equal, and so do two "
        "different objects. A member that is present, callable, and *wrong* is outside this "
        "inventory; `conformance/native` is where that is caught. The `(itself).name` row is "
        "the one exception, and it is narrow: a receiver's *own* name is read, because a "
        "receiver is named for what it is rather than for a member it publishes."
    )
    add(
        "- **One level of nesting.** Globals, their statics, their prototypes, the object each of "
        "those inherits from, and the `node:` namespaces. A namespace inside a namespace "
        "(`path.posix`) is measured as one member, not walked."
    )
    add(
        "- **Symbol-keyed members only where they are well known.** A symbol is reported as "
        "`@@name` when it is a property of `Symbol`; a registered or private symbol has no "
        "spelling this probe can put to the other engine."
    )
    add(
        f"- **{len(mute)} receivers cannot describe themselves natively.** "
        "`Object.getOwnPropertyNames` answers on the built-ins ADR 0043 put on the shared table, "
        "on the prototypes and on the namespaces, and refuses elsewhere. On those receivers an "
        "*invented* member --- one native publishes and the interpreter does not --- is invisible, "
        "because nothing would have thought to ask for it by name."
    )
    add(
        "- **Instances are not probed.** Everything here is read off a constructor, a prototype or "
        "a namespace. A member that resolves differently on a live receiver than on the prototype "
        "it came from is not something this method can reach."
    )
    add("")

    if runner.errors:
        add("## Runs that were not measurements")
        add("")
        add("Each of these failed three times for a reason that was not a refusal, so it is named")
        add("rather than counted. Its receiver's numbers above are incomplete.")
        add("")
        for err in runner.errors:
            add(f"- `{err}`")
        add("")

    add("## Every gap, by receiver")
    add("")
    for key in ranked:
        d, r = per[key]["diverged"], per[key]["refused"]
        if not d and not r and not per[key]["unmeasured"]:
            continue
        add(f"### `{key}`")
        add("")
        if d:
            add("Diverged:")
            add("")
            for name, want, got in d:
                add(f"- `{name}` — interpreter `{want}`, native `{got}`")
            add("")
        if r:
            add("Refused: " + ", ".join(f"`{name}`" for name, _, _ in r))
            add("")
            reasons = {reason for _, _, reason in r}
            if len(reasons) == 1 and len(r) > 1:
                # One reason behind every name is one decision, and saying so is the difference
                # between a backlog of twenty-four items and a backlog of one.
                add(f"All for one reason: `{reasons.pop()}`")
                add("")
        u = per[key]["unmeasured"]
        if u:
            add("Unmeasured: " + ", ".join(f"`{name}`" for name, _, _ in u))
            add("")
    return "\n".join(lines) + "\n"


if __name__ == "__main__":
    raise SystemExit(main())
