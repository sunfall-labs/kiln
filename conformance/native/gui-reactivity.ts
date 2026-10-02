// kiln:gui reactivity: synchronous writes, glitch-free derivations, ownership and cleanup,
// context, keyed mapping and async memos. No widgets: this is the core every widget binds to.
import {
  batch,
  createContext,
  createEffect,
  createMemo,
  createRoot,
  createSignal,
  isPending,
  mapArray,
  on,
  onCleanup,
  onMount,
  untrack,
  useContext,
} from "kiln:gui";

const log: string[] = [];

// A write outside batch has run its effects by the time it returns.
const [count, setCount] = createSignal(1);
let seen = 0;
createEffect(() => {
  seen = count();
});
setCount(2);
console.log("sync write:", seen);

// Diamond: d depends on b and c, both on a. One write, one recomputation of d, never a mix of
// old and new values.
const [a, setA] = createSignal(1);
const b = createMemo(() => a() * 2);
const c = createMemo(() => a() + 10);
let dRuns = 0;
const d = createMemo(() => {
  dRuns++;
  return `${b()}/${c()}`;
});
const observed: string[] = [];
createEffect(() => {
  observed.push(d());
});
setA(5);
console.log("diamond:", observed.join(" "), "d runs:", dRuns);

// A memo whose value did not change stops propagation.
const [n, setN] = createSignal(2);
const parity = createMemo(() => n() % 2);
let parityEffects = 0;
createEffect(() => {
  parity();
  parityEffects++;
});
setN(4);
setN(6);
console.log("unchanged memo stops:", parityEffects);

// batch defers effects to the end and runs each once.
const [x, setX] = createSignal(0);
const xs: number[] = [];
createEffect(() => {
  xs.push(x());
});
batch(() => {
  setX(1);
  setX(2);
  setX(3);
});
console.log("batch:", xs.join(","));

// Equal writes do nothing; equals: false always notifies.
const [same, setSame] = createSignal("v");
const [always, setAlways] = createSignal("v", { equals: false });
let sameRuns = 0;
let alwaysRuns = 0;
createEffect(() => {
  same();
  sameRuns++;
});
createEffect(() => {
  always();
  alwaysRuns++;
});
setSame("v");
setAlways("v");
console.log("equality:", sameRuns, alwaysRuns);

// Cleanups run before the next run and on disposal, innermost first.
createRoot((dispose) => {
  const [k, setK] = createSignal(0);
  createEffect(() => {
    const v = k();
    createEffect(() => {
      onCleanup(() => log.push(`inner ${v}`));
    });
    onCleanup(() => log.push(`outer ${v}`));
  });
  setK(1);
  dispose();
  setK(2);
});
console.log("cleanup order:", log.join(", "));

// untrack and on() limit what an effect follows.
const [p, setP] = createSignal(1);
const [q, setQ] = createSignal(1);
let tracked = 0;
createEffect(() => {
  p();
  untrack(q);
  tracked++;
});
setQ(2);
setP(2);
const deltas: string[] = [];
createEffect(
  on(p, (value, prev) => {
    deltas.push(`${prev}->${value}`);
  }),
);
setP(3);
console.log("untrack:", tracked, "on:", deltas.join(" "));

// The split form: compute is tracked, apply is not, and apply's return value cleans up.
const [s, setS] = createSignal("a");
const applied: string[] = [];
createEffect(
  () => s(),
  (value) => {
    applied.push(`+${value}`);
    return () => applied.push(`-${value}`);
  },
);
setS("b");
console.log("split effect:", applied.join(" "));

// Context flows down the owner chain.
const Theme = createContext("light");
createRoot(() => {
  const outer = useContext(Theme);
  let inner = "";
  Theme.Provider({
    value: "dark",
    get children() {
      inner = useContext(Theme);
      return null;
    },
  });
  console.log("context:", outer, inner);
});

// onMount waits for the surrounding build to finish.
createRoot(() => {
  const order: string[] = [];
  onMount(() => order.push("mounted"));
  order.push("built");
  queueMicrotask(() => console.log("onMount:", order.join(" then ")));
});

// mapArray keeps the mapping of an item that stays.
createRoot(() => {
  const [list, setList] = createSignal(["a", "b", "c"]);
  let mapCalls = 0;
  const mapped = mapArray(list, (item, index) => {
    mapCalls++;
    return { item, index };
  });
  const first = mapped();
  setList(["c", "a", "d"]);
  const second = mapped();
  console.log(
    "mapArray:",
    mapCalls,
    second[1] === first[0],
    second[0] === first[2],
    second.map((m) => `${m.item}@${m.index()}`).join(" "),
  );
});

// A write cycle is reported instead of hanging.
createRoot(() => {
  const [loop, setLoop] = createSignal(0);
  try {
    createEffect(() => {
      setLoop(loop() + 1);
    });
  } catch (error) {
    console.log("cycle:", String(error).includes("re-triggering"));
  }
});

// An async memo keeps its previous value until the promise settles.
const [id, setId] = createSignal(1);
const user = createMemo(
  () => {
    const current = id();
    return Promise.resolve(`user ${current}`);
  },
  "none",
);
const pendingLog: string[] = [];
createEffect(() => {
  pendingLog.push(`${user()}${isPending(user) ? " (loading)" : ""}`);
});
setId(2);
setTimeout(() => {
  console.log("async memo:", pendingLog.join(" | "));
}, 0);
