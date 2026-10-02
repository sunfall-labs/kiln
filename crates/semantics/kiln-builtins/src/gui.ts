// kiln:gui — native desktop UI for kiln programs (ADR 0074).
//
// Layers, bottom to top:
//
//   1. Reactivity    signals, memos and effects with owners; glitch-free (push-pull)
//   2. Control flow  Show, For, Index, Switch/Match, Dynamic, context
//   3. Hosts         where widgets live: the native library (AppKit today) or an in-memory
//                    headless host for tests and for engines without FFI
//   4. Widgets       Node and its subclasses; layout semantics are decided here, once
//   5. JSX           lazy elements, insert(), render(), runApp()
//   6. Event loop    drains host events in batches between bounded native waits
//   7. OS services   dialogs, clipboard, appearance, notifications, menus, tray, shortcuts

// ─────────────────────────────────────────────────────────────────────────────────────────────
// 1. Reactivity
//
// Signals push "maybe stale" marks down the graph; effects pull from their sources when they
// run. A memo recomputes only when a source really changed, and an effect never sees two
// sources from different moments (no glitches). A write outside `batch` flushes before it
// returns, so `setCount(1); label.text` reads the new text.

export type Accessor<T> = () => T;
export type Setter<T> = (value: T | ((prev: T) => T)) => T;
export type MaybeAccessor<T> = T | Accessor<T>;
type Cleanup = () => void;

const CLEAN = 0;
const CHECK = 1;
const DIRTY = 2;

interface Owner {
  owner: Owner | null;
  owned: Computation<any>[] | null;
  cleanups: Cleanup[] | null;
  context: Map<object, unknown> | null;
}

interface Source {
  observers: Set<Computation<any>>;
}

let currentOwner: Owner | null = null;
let listener: Computation<any> | null = null;
let batchDepth = 0;
let rootDepth = 0;
let flushing = false;
let nextComputationId = 1;
const effectQueue: Computation<any>[] = [];
const mountQueue: Cleanup[] = [];

class SignalState<T> implements Source {
  observers = new Set<Computation<any>>();
  value: T;
  equals: ((a: T, b: T) => boolean) | false;
  constructor(value: T, equals: ((a: T, b: T) => boolean) | false) {
    this.value = value;
    this.equals = equals;
  }

  read(): T {
    track(this);
    return this.value;
  }

  write(next: T): void {
    if (this.equals && this.equals(this.value, next)) return;
    this.value = next;
    for (const o of this.observers) mark(o, DIRTY);
    commit();
  }
}

class Computation<T> implements Owner, Source {
  readonly id = nextComputationId++;
  owner: Owner | null;
  owned: Computation<any>[] | null = null;
  cleanups: Cleanup[] | null = null;
  context: Map<object, unknown> | null = null;
  observers = new Set<Computation<any>>();
  sources = new Set<Source>();
  state = DIRTY;
  disposed = false;
  epoch = 0;
  pending: SignalState<boolean> | null = null;
  settled = false;
  fn: (prev: T) => T | PromiseLike<T>;
  value: T;
  readonly memo: boolean;
  equals: ((a: T, b: T) => boolean) | false;
  apply: ((value: T) => void | Cleanup) | null;

  constructor(
    fn: (prev: T) => T | PromiseLike<T>,
    value: T,
    memo: boolean,
    equals: ((a: T, b: T) => boolean) | false,
    apply: ((value: T) => void | Cleanup) | null,
  ) {
    this.fn = fn;
    this.value = value;
    this.memo = memo;
    this.equals = equals;
    this.apply = apply;
    this.owner = currentOwner;
    if (currentOwner) (currentOwner.owned || (currentOwner.owned = [])).push(this);
  }

  read(): T {
    if (this.state !== CLEAN && !this.disposed) update(this);
    track(this);
    return this.value;
  }
}

function track(source: Source): void {
  if (listener && !listener.disposed) {
    source.observers.add(listener);
    listener.sources.add(source);
  }
}

function mark(c: Computation<any>, state: number): void {
  if (c.state >= state || c.disposed) return;
  const wasClean = c.state === CLEAN;
  c.state = state;
  if (!wasClean) return;
  if (c.memo) {
    for (const o of c.observers) mark(o, CHECK);
  } else {
    effectQueue.push(c);
  }
}

function update(c: Computation<any>): void {
  if (c.state === CHECK) {
    for (const s of c.sources) {
      if (s instanceof Computation) {
        if (s.state !== CLEAN) update(s);
        if ((c.state as number) === DIRTY) break;
      }
    }
  }
  if (c.state === DIRTY) run(c);
  else c.state = CLEAN;
}

function isThenable(v: unknown): v is PromiseLike<unknown> {
  return v !== null && (typeof v === "object" || typeof v === "function") && typeof (v as any).then === "function";
}

function run<T>(c: Computation<T>): void {
  cleanNode(c);
  // Clean before running: a write to something this computation already read marks it again.
  c.state = CLEAN;
  const prevOwner = currentOwner;
  const prevListener = listener;
  currentOwner = listener = c;
  const epoch = ++c.epoch;
  let next: T | PromiseLike<T>;
  try {
    next = c.fn(c.value);
  } finally {
    currentOwner = prevOwner;
    listener = prevListener;
  }
  if (isThenable(next)) {
    setPending(c, true);
    next.then(
      (value) => {
        if (c.disposed || c.epoch !== epoch) return;
        batch(() => {
          setPending(c, false);
          settle(c, value);
        });
      },
      (error) => {
        if (c.disposed || c.epoch !== epoch) return;
        setPending(c, false);
        reportError(error);
      },
    );
    return;
  }
  if (c.pending && c.pending.value) setPending(c, false);
  settle(c, next);
}

function setPending(c: Computation<any>, on: boolean): void {
  if (!c.pending) {
    if (!on) return;
    c.pending = new SignalState(false, (a, b) => a === b);
  }
  c.pending.write(on);
}

function settle<T>(c: Computation<T>, value: T): void {
  const prev = c.value;
  const first = !c.settled;
  c.settled = true;
  c.value = value;
  if (c.memo) {
    if (first || !c.equals || !c.equals(prev, value)) {
      for (const o of c.observers) mark(o, DIRTY);
    }
    return;
  }
  if (c.apply) {
    const apply = c.apply;
    const prevOwner = currentOwner;
    currentOwner = c;
    try {
      const cleanup = untrack(() => apply(value));
      if (typeof cleanup === "function") onCleanup(cleanup);
    } finally {
      currentOwner = prevOwner;
    }
  }
}

function cleanNode(o: Owner): void {
  if (o.owned) {
    const owned = o.owned;
    o.owned = null;
    for (let i = owned.length - 1; i >= 0; i--) dispose(owned[i]);
  }
  if (o.cleanups) {
    const cleanups = o.cleanups;
    o.cleanups = null;
    for (let i = cleanups.length - 1; i >= 0; i--) cleanups[i]();
  }
  if (o instanceof Computation) {
    for (const s of o.sources) s.observers.delete(o);
    o.sources.clear();
  }
}

function dispose(c: Computation<any>): void {
  if (c.disposed) return;
  c.disposed = true;
  cleanNode(c);
  c.state = CLEAN;
}

function reportError(error: unknown): void {
  console.error("kiln:gui:", error);
}

function commit(): void {
  if (batchDepth === 0 && !flushing) flush();
}

function runMounts(): void {
  while (mountQueue.length > 0) {
    const mounts = mountQueue.splice(0, mountQueue.length);
    for (const m of mounts) m();
  }
}

/** Runs every pending effect now. Writes outside `batch` already do this. */
export function flush(): void {
  if (flushing) return;
  flushing = true;
  let failure: { error: unknown } | null = null;
  try {
    let rounds = 0;
    while (effectQueue.length > 0 || mountQueue.length > 0) {
      if (++rounds > 100000) {
        for (const e of effectQueue) e.state = CLEAN;
        effectQueue.length = 0;
        throw new Error("kiln:gui: effects keep re-triggering each other (a write cycle)");
      }
      if (effectQueue.length > 0) {
        const queue = effectQueue.splice(0, effectQueue.length);
        // Parents were created first; running them first disposes children they replace.
        queue.sort((a, b) => a.id - b.id);
        for (const e of queue) {
          if (e.disposed || e.state === CLEAN) continue;
          try {
            update(e);
          } catch (error) {
            e.state = CLEAN;
            if (!failure) failure = { error };
          }
        }
      } else if (rootDepth === 0) {
        runMounts();
      } else {
        break;
      }
    }
  } finally {
    flushing = false;
  }
  if (failure) throw failure.error;
}

export interface SignalOptions<T> {
  /** `false` notifies on every write; the default skips writes of an identical value. */
  equals?: false | ((prev: T, next: T) => boolean);
}

const sameValue = (a: unknown, b: unknown) => a === b || (a !== a && b !== b);

export function createSignal<T>(value: T, options?: SignalOptions<T>): [Accessor<T>, Setter<T>] {
  const equals = options && options.equals !== undefined ? options.equals : sameValue;
  const s = new SignalState<T>(value, equals);
  return [
    () => s.read(),
    (next) => {
      const value = typeof next === "function" ? (next as (prev: T) => T)(s.value) : next;
      s.write(value);
      return value;
    },
  ];
}

/**
 * A derived value, recomputed only when a source it read changed. `fn` may return a promise:
 * the memo keeps its previous value until it settles, and `isPending` reports the wait.
 */
export function createMemo<T>(fn: (prev: T | undefined) => T | PromiseLike<T>, initial?: T, options?: SignalOptions<T>): Accessor<T> {
  const equals = options && options.equals !== undefined ? options.equals : sameValue;
  const c = new Computation<T>(fn as any, initial as T, true, equals, null);
  batch(() => update(c));
  return () => c.read();
}

/**
 * Runs `fn` now and again whenever what it read changes. With two arguments, `compute` is
 * tracked and `apply` receives its result untracked; a function returned from `apply` runs as
 * the cleanup before the next run. Returns a function that stops the effect.
 */
export function createEffect<T>(compute: (prev: T | undefined) => T | PromiseLike<T>, apply?: (value: T) => void | Cleanup): Cleanup {
  const c = new Computation<T>(compute as any, undefined as T, false, false, apply || null);
  // The first run is batched: a write inside it re-runs it afterwards, never inside itself.
  batch(() => update(c));
  return () => dispose(c);
}

/** Runs `fn` with a fresh owner that lives until `dispose` is called. */
export function createRoot<T>(fn: (dispose: Cleanup) => T): T {
  const root: Owner = { owner: currentOwner, owned: null, cleanups: null, context: null };
  const prevOwner = currentOwner;
  const prevListener = listener;
  currentOwner = root;
  listener = null;
  rootDepth++;
  try {
    return fn(() => cleanNode(root));
  } finally {
    currentOwner = prevOwner;
    listener = prevListener;
    if (--rootDepth === 0 && batchDepth === 0 && !flushing) runMounts();
  }
}

export function untrack<T>(fn: () => T): T {
  const prev = listener;
  listener = null;
  try {
    return fn();
  } finally {
    listener = prev;
  }
}

/** Defers effects until `fn` returns, then runs each affected effect once. */
export function batch<T>(fn: () => T): T {
  batchDepth++;
  let result: T;
  try {
    result = fn();
  } finally {
    batchDepth--;
  }
  commit();
  return result;
}

/** Registers `fn` to run when the current owner (component, effect, root) is disposed. */
export function onCleanup(fn: Cleanup): Cleanup {
  if (currentOwner) (currentOwner.cleanups || (currentOwner.cleanups = [])).push(fn);
  return fn;
}

/** Runs `fn` once, after the tree being built right now is in place. */
export function onMount(fn: () => void): void {
  const owner = currentOwner;
  mountQueue.push(() => runWithOwner(owner, () => untrack(fn)));
  if (rootDepth === 0) commit();
}

export function getOwner(): Owner | null {
  return currentOwner;
}

export function runWithOwner<T>(owner: Owner | null, fn: () => T): T {
  const prev = currentOwner;
  currentOwner = owner;
  try {
    return fn();
  } finally {
    currentOwner = prev;
  }
}

/** Tracks only `deps`; `fn` runs untracked with their values (and the previous ones). */
export function on<D, T>(deps: Accessor<D>, fn: (value: D, prev: D | undefined) => T, options?: { defer?: boolean }): (prev?: T) => T | undefined {
  let prevValue: D | undefined;
  let defer = !!(options && options.defer);
  return () => {
    const value = deps();
    if (defer) {
      defer = false;
      prevValue = value;
      return undefined;
    }
    const result = untrack(() => fn(value, prevValue));
    prevValue = value;
    return result;
  };
}

/** True while an async memo that `accessor` reads is waiting for its promise. Tracked. */
export function isPending(accessor: Accessor<unknown>): boolean {
  const probe = runWithOwner(null, () => new Computation<void>(() => undefined, undefined, true, false, null));
  const prev = listener;
  listener = probe;
  try {
    accessor();
  } finally {
    listener = prev;
  }
  let pending = false;
  for (const s of probe.sources) {
    if (s instanceof Computation) {
      if (!s.pending) s.pending = new SignalState(false, (a, b) => a === b);
      if (s.pending.read()) pending = true;
    }
  }
  dispose(probe);
  return pending;
}

export interface Context<T> {
  readonly id: object;
  readonly defaultValue: T;
  Provider: (props: { value: T; children?: unknown }) => unknown;
}

export function createContext<T>(defaultValue: T): Context<T> {
  const id = {};
  return {
    id,
    defaultValue,
    Provider: (props) =>
      createMemo(() => {
        const owner = currentOwner!;
        owner.context = new Map([[id, props.value as unknown]]);
        return renderChildren(props.children);
      }),
  };
}

export function useContext<T>(context: Context<T>): T {
  for (let o = currentOwner; o; o = o.owner) {
    if (o.context && o.context.has(context.id)) return o.context.get(context.id) as T;
  }
  return context.defaultValue;
}

const read = <T,>(v: MaybeAccessor<T>): T => (typeof v === "function" ? (v as Accessor<T>)() : v);

/**
 * Maps a list by item identity: an item that stays keeps its mapped result (and its widgets),
 * a new item is mapped once, a removed item's result is disposed.
 */
export function mapArray<T, U>(list: Accessor<readonly T[] | null | undefined | false>, map: (item: T, index: Accessor<number>) => U): Accessor<U[]> {
  let items: T[] = [];
  let mapped: U[] = [];
  let disposers: Cleanup[] = [];
  let indexes: Setter<number>[] = [];
  const owner = currentOwner;
  onCleanup(() => {
    for (const d of disposers) d();
  });
  return () => {
    const next = (list() || []) as T[];
    return untrack(() => {
      const reuse = new Map<T, number[]>();
      for (let i = 0; i < items.length; i++) {
        const queue = reuse.get(items[i]);
        if (queue) queue.push(i);
        else reuse.set(items[i], [i]);
      }
      const nextMapped: U[] = [];
      const nextDisposers: Cleanup[] = [];
      const nextIndexes: Setter<number>[] = [];
      for (let j = 0; j < next.length; j++) {
        const queue = reuse.get(next[j]);
        if (queue && queue.length > 0) {
          const i = queue.shift()!;
          nextMapped[j] = mapped[i];
          nextDisposers[j] = disposers[i];
          nextIndexes[j] = indexes[i];
          indexes[i](j);
          disposers[i] = () => {};
        } else {
          const item = next[j];
          runWithOwner(owner, () =>
            createRoot((d) => {
              const [index, setIndex] = createSignal(j);
              nextDisposers[j] = d;
              nextIndexes[j] = setIndex;
              nextMapped[j] = map(item, index);
            }),
          );
        }
      }
      for (const d of disposers) d();
      items = next.slice();
      mapped = nextMapped;
      disposers = nextDisposers;
      indexes = nextIndexes;
      return mapped.slice();
    });
  };
}

/** Maps a list by position: each slot keeps its result and receives its item as a signal. */
export function indexArray<T, U>(list: Accessor<readonly T[] | null | undefined | false>, map: (item: Accessor<T>, index: number) => U): Accessor<U[]> {
  let mapped: U[] = [];
  let setters: Setter<T>[] = [];
  let disposers: Cleanup[] = [];
  const owner = currentOwner;
  onCleanup(() => {
    for (const d of disposers) d();
  });
  return () => {
    const next = (list() || []) as T[];
    return untrack(() => {
      for (let i = 0; i < Math.min(next.length, mapped.length); i++) setters[i](() => next[i]);
      for (let i = mapped.length; i < next.length; i++) {
        runWithOwner(owner, () =>
          createRoot((d) => {
            const [item, setItem] = createSignal(next[i], { equals: false });
            setters[i] = setItem;
            disposers[i] = d;
            mapped[i] = map(item, i);
          }),
        );
      }
      for (let i = next.length; i < mapped.length; i++) disposers[i]();
      mapped.length = setters.length = disposers.length = next.length;
      return mapped.slice();
    });
  };
}

// ─────────────────────────────────────────────────────────────────────────────────────────────
// 2. Control flow
//
// JSX here is evaluated without a compiler, so a dynamic expression is written as a function:
// `<text>{() => count()}</text>`, `<Show when={visible}>`. Components receive props as given.

export function Show<T>(props: {
  when: MaybeAccessor<T | null | undefined | false>;
  fallback?: unknown;
  keyed?: boolean;
  children?: unknown | ((value: T) => unknown);
}): Accessor<unknown> {
  const condition = createMemo<T | null | undefined | false>(() => read(props.when), undefined, {
    equals: props.keyed ? sameValue : (a, b) => !a === !b,
  });
  return createMemo(() => {
    const value = condition();
    if (!value) return renderChildren(props.fallback);
    const children = props.children;
    if (typeof children === "function" && children.length > 0) {
      return renderChildren(untrack(() => (children as (v: T) => unknown)(value as T)));
    }
    return renderChildren(children);
  });
}

export function For<T>(props: {
  each: MaybeAccessor<readonly T[] | null | undefined | false>;
  fallback?: unknown;
  children: (item: T, index: Accessor<number>) => unknown;
}): Accessor<unknown> {
  const mapped = mapArray(() => read(props.each), (item, index) => renderChildren(props.children(item, index)));
  const fallback = createMemo(() => (props.fallback !== undefined && mapped().length === 0 ? renderChildren(props.fallback) : null));
  return () => fallback() || mapped();
}

export function Index<T>(props: {
  each: MaybeAccessor<readonly T[] | null | undefined | false>;
  fallback?: unknown;
  children: (item: Accessor<T>, index: number) => unknown;
}): Accessor<unknown> {
  const mapped = indexArray(() => read(props.each), (item, index) => renderChildren(props.children(item, index)));
  const fallback = createMemo(() => (props.fallback !== undefined && mapped().length === 0 ? renderChildren(props.fallback) : null));
  return () => fallback() || mapped();
}

interface MatchCase {
  readonly kilnMatch: true;
  when: MaybeAccessor<unknown>;
  children?: unknown;
}

export function Match<T>(props: { when: MaybeAccessor<T | null | undefined | false>; children?: unknown | ((value: T) => unknown) }): MatchCase {
  return { kilnMatch: true, when: props.when, children: props.children };
}

export function Switch(props: { fallback?: unknown; children?: unknown }): Accessor<unknown> {
  const cases = (flatten(props.children) as unknown[])
    .map((c) => (c instanceof JsxElement ? c.render() : c))
    .filter((c): c is MatchCase => !!c && (c as MatchCase).kilnMatch === true);
  const chosen = createMemo(
    () => {
      for (let i = 0; i < cases.length; i++) {
        const value = read(cases[i].when);
        if (value) return [i, value] as [number, unknown];
      }
      return [-1, undefined] as [number, unknown];
    },
    undefined,
    { equals: (a, b) => a[0] === b[0] && !a[1] === !b[1] },
  );
  return createMemo(() => {
    const [i, value] = chosen();
    if (i < 0) return renderChildren(props.fallback);
    const children = cases[i].children;
    return renderChildren(typeof children === "function" && children.length > 0 ? untrack(() => (children as (v: unknown) => unknown)(value)) : children);
  });
}

/** Renders `component` (a tag, widget class or function) with the remaining props. */
export function Dynamic(props: { component: MaybeAccessor<Component | string | null | undefined> } & Record<string, unknown>): Accessor<unknown> {
  const component = createMemo(() => {
    const c = props.component;
    return typeof c === "function" && !isWidgetClass(c) && c.length === 0 ? (c as Accessor<Component | string>)() : c;
  });
  return createMemo(() => {
    const c = component();
    if (!c) return undefined;
    const rest: Record<string, unknown> = {};
    for (const key of Object.keys(props)) if (key !== "component") rest[key] = props[key];
    return renderChildren(new JsxElement(c as Component, rest));
  });
}

// ─────────────────────────────────────────────────────────────────────────────────────────────
// 3. Hosts

/** An event from a host: which widget, what happened, and its payload. */
export interface HostEvent {
  handle: number;
  kind: number;
  a: number;
  b: number;
  text: string;
}

const EVENT_PRESS = 1;
const EVENT_INPUT = 2;
const EVENT_SUBMIT = 3;
const EVENT_CHANGE = 4;
const EVENT_CLOSE = 5;
const EVENT_RESIZE = 6;
const EVENT_MENU = 7;
const EVENT_HOTKEY = 8;
const EVENT_TRAY = 9;
const EVENT_QUIT = 10;

const EVENT_NAMES: Record<number, string> = {
  [EVENT_PRESS]: "press",
  [EVENT_INPUT]: "input",
  [EVENT_SUBMIT]: "submit",
  [EVENT_CHANGE]: "change",
  [EVENT_CLOSE]: "close",
  [EVENT_RESIZE]: "resize",
};

/** Where widgets live. The native library and the headless host implement the same contract. */
export interface Host {
  /** `"appkit"`, `"headless"`. */
  readonly name: string;
  /** True when widgets are real windows that need the event loop. */
  readonly interactive: boolean;
  create(kind: string): number;
  set(handle: number, name: string, value: unknown): void;
  insert(parent: number, child: number, before: number): void;
  remove(parent: number, child: number): void;
  destroy(handle: number): void;
  call(handle: number, command: string, arg?: string): void;
  /** What the toolkit holds for a widget's assistive technology: label, hint, whether it is
   * announced at all (`element`) and whether it has keyboard focus. */
  describe(handle: number): Omit<AccessibilityDescription, "role">;
  /** Processes OS events for up to `ms`, returning early when any arrive. */
  wait(ms: number): void;
  nextEvent(): HostEvent | null;
  /** How many tree or property operations the host has seen; the event loop reads activity. */
  readonly operations: number;
  readonly services: Services;
}

export interface AccessibilityDescription {
  role: string;
  label: string;
  hint: string;
  element: boolean;
  focused: boolean;
}

export interface FileFilter {
  name: string;
  extensions: string[];
}

export interface Services {
  dialog(kind: "open" | "save" | "folder" | "message" | "confirm", options: Record<string, unknown>): unknown;
  clipboardRead(): string;
  clipboardWrite(text: string): void;
  clipboardClear(): void;
  isDark(): boolean;
  notify(title: string, body: string, appName: string): void;
  appPath(name: string): string;
  singleInstance(appId: string): boolean;
  setMenu(spec: unknown[]): void;
  trayCreate(spec: unknown): number;
  trayUpdate(id: number, spec: unknown): void;
  trayDestroy(id: number): void;
  hotkeyRegister(accelerator: string): number;
  hotkeyUnregister(id: number): void;
}

interface HeadlessWidget {
  handle: number;
  kind: string;
  props: Record<string, unknown>;
  children: number[];
  parent: number;
}

/**
 * Keeps the widget tree in memory. Used for tests (`useHeadless()`), and whenever no native
 * backend is available. `inspect()` prints what a native host would have built.
 */
export class HeadlessHost implements Host {
  readonly name = "headless";
  readonly interactive = false;
  operations = 0;
  readonly widgets = new Map<number, HeadlessWidget>();
  readonly notifications: { title: string; body: string }[] = [];
  /** Answers for dialogs, by kind; a dialog without one is cancelled. */
  readonly dialogAnswers: Record<string, unknown> = {};
  menu: unknown[] = [];
  private next = 1;
  private clipboard = "";
  private events: HostEvent[] = [];
  private focused = 0;
  dark = false;

  create(kind: string): number {
    const handle = this.next++;
    this.widgets.set(handle, { handle, kind, props: {}, children: [], parent: 0 });
    this.operations++;
    return handle;
  }
  set(handle: number, name: string, value: unknown): void {
    const w = this.widget(handle);
    w.props[name] = value;
    this.operations++;
  }
  insert(parent: number, child: number, before: number): void {
    const p = this.widget(parent);
    const c = this.widget(child);
    if (c.parent) this.remove(c.parent, child);
    const at = before ? p.children.indexOf(before) : -1;
    if (at >= 0) p.children.splice(at, 0, child);
    else p.children.push(child);
    c.parent = parent;
    this.operations++;
  }
  remove(parent: number, child: number): void {
    const p = this.widget(parent);
    const at = p.children.indexOf(child);
    if (at >= 0) p.children.splice(at, 1);
    this.widget(child).parent = 0;
    this.operations++;
  }
  destroy(handle: number): void {
    const w = this.widgets.get(handle);
    if (!w) return;
    if (w.parent && this.widgets.has(w.parent)) this.remove(w.parent, handle);
    this.widgets.delete(handle);
    this.operations++;
  }
  call(handle: number, command: string, arg?: string): void {
    const w = this.widget(handle);
    if (command === "focus") this.focused = handle;
    if (command === "show") w.props.visible = true;
    if (command === "close" || command === "hide") w.props.visible = false;
    void arg;
  }
  wait(_ms: number): void {}
  describe(handle: number): Omit<AccessibilityDescription, "role"> {
    const w = this.widget(handle);
    const label = String(w.props.accessibilityLabel ?? "");
    const pressable = w.props.pressable === true;
    return {
      label,
      hint: String(w.props.accessibilityHint ?? ""),
      element: w.kind === "image" ? label !== "" : w.kind !== "container" || pressable,
      focused: this.focused === handle,
    };
  }
  nextEvent(): HostEvent | null {
    return this.events.shift() || null;
  }
  /** Queues an event as if the OS had delivered it; the next drain dispatches it. */
  post(event: HostEvent): void {
    this.events.push(event);
  }

  private widget(handle: number): HeadlessWidget {
    const w = this.widgets.get(handle);
    if (!w) throw new Error(`kiln:gui: headless host has no widget ${handle}`);
    return w;
  }

  /** The widget tree under `handle` as indented text, props sorted, layout omitted. */
  inspect(handle: number, depth = 0): string {
    const w = this.widget(handle);
    const props = Object.keys(w.props)
      .filter((k) => k !== "layout")
      .sort()
      .map((k) => `${k}=${JSON.stringify(w.props[k])}`)
      .join(" ");
    let out = `${"  ".repeat(depth)}<${w.kind}${props ? " " + props : ""}>`;
    for (const c of w.children) out += "\n" + this.inspect(c, depth + 1);
    return out;
  }

  readonly services: Services = {
    dialog: (kind) => (kind in this.dialogAnswers ? this.dialogAnswers[kind] : kind === "message" ? true : null),
    clipboardRead: () => this.clipboard,
    clipboardWrite: (text) => {
      this.clipboard = text;
    },
    clipboardClear: () => {
      this.clipboard = "";
    },
    isDark: () => this.dark,
    notify: (title, body) => {
      this.notifications.push({ title, body });
    },
    appPath: (name) => fallbackPath(name),
    singleInstance: () => true,
    setMenu: (spec) => {
      this.menu = spec;
    },
    trayCreate: () => this.next++,
    trayUpdate: () => {},
    trayDestroy: () => {},
    hotkeyRegister: () => this.next++,
    hotkeyUnregister: () => {},
  };
}

function env(name: string): string | undefined {
  return typeof process !== "undefined" && process.env ? process.env[name] : undefined;
}

function fallbackPath(name: string): string {
  const home = env("HOME") || env("USERPROFILE") || "/";
  switch (name) {
    case "home":
      return home;
    case "temp":
      return env("TMPDIR") || env("TEMP") || "/tmp";
    case "desktop":
      return home + "/Desktop";
    case "documents":
      return home + "/Documents";
    case "downloads":
      return home + "/Downloads";
    default:
      return home + "/." + name;
  }
}

const ABI_VERSION = 3;

const NATIVE_FUNCTIONS: Record<string, { arguments: string[]; return: string }> = {
  kiln_gui_abi_version: { arguments: [], return: "u32" },
  kiln_gui_result: { arguments: ["pointer", "u64"], return: "i32" },
  kiln_gui_init: { arguments: [], return: "i32" },
  kiln_gui_create: { arguments: ["pointer"], return: "u32" },
  kiln_gui_set_text: { arguments: ["u32", "pointer", "pointer"], return: "i32" },
  kiln_gui_set_number: { arguments: ["u32", "pointer", "f64"], return: "i32" },
  kiln_gui_insert: { arguments: ["u32", "u32", "u32"], return: "i32" },
  kiln_gui_remove: { arguments: ["u32", "u32"], return: "i32" },
  kiln_gui_destroy: { arguments: ["u32"], return: "i32" },
  kiln_gui_call: { arguments: ["u32", "pointer", "pointer"], return: "i32" },
  kiln_gui_describe: { arguments: ["u32"], return: "i32" },
  kiln_gui_wait: { arguments: ["f64"], return: "i32" },
  kiln_gui_next_event: { arguments: ["pointer"], return: "i32" },
  kiln_gui_dialog: { arguments: ["pointer", "pointer"], return: "i32" },
  kiln_gui_clipboard_read: { arguments: [], return: "i32" },
  kiln_gui_clipboard_write: { arguments: ["pointer"], return: "i32" },
  kiln_gui_clipboard_clear: { arguments: [], return: "i32" },
  kiln_gui_is_dark: { arguments: [], return: "i32" },
  kiln_gui_notify: { arguments: ["pointer", "pointer", "pointer"], return: "i32" },
  kiln_gui_app_path: { arguments: ["pointer"], return: "i32" },
  kiln_gui_single_instance: { arguments: ["pointer"], return: "i32" },
  kiln_gui_set_menu: { arguments: ["pointer"], return: "i32" },
  kiln_gui_tray_create: { arguments: ["pointer"], return: "u32" },
  kiln_gui_tray_update: { arguments: ["u32", "pointer"], return: "i32" },
  kiln_gui_tray_destroy: { arguments: ["u32"], return: "i32" },
  kiln_gui_hotkey_register: { arguments: ["pointer"], return: "u32" },
  kiln_gui_hotkey_unregister: { arguments: ["u32"], return: "i32" },
};

/** The native library (`libkiln_host_gui`), bound through FFI. Every failure throws its reason. */
class NativeHost implements Host {
  readonly interactive = true;
  operations = 0;
  private header = new ArrayBuffer(32);
  private header32 = new Uint32Array(this.header);
  private header64 = new Float64Array(this.header);
  private textBuffer = new Uint8Array(4096);
  private decoder = new TextDecoder();

  readonly name: string;
  private fn: Record<string, (...args: unknown[]) => any>;

  constructor(name: string, fn: Record<string, (...args: unknown[]) => any>) {
    this.name = name;
    this.fn = fn;
  }

  /** Reads the text a call stored (a result or an error message) of `length` bytes. */
  private result(length: number): string {
    if (length <= 0) return "";
    if (length > this.textBuffer.length) this.textBuffer = new Uint8Array(length * 2);
    const n = this.fn.kiln_gui_result(this.textBuffer, length);
    return this.decoder.decode(this.textBuffer.subarray(0, n));
  }

  private check(code: number, what: string): number {
    if (code === -2) throw new Error(`kiln:gui: ${what}: ${this.result(1 << 16)}`);
    return code;
  }

  create(kind: string): number {
    this.operations++;
    const handle = this.fn.kiln_gui_create(kind);
    if (!handle) throw new Error(`kiln:gui: create <${kind}>: ${this.result(1 << 16)}`);
    return handle;
  }
  set(handle: number, name: string, value: unknown): void {
    this.operations++;
    if (typeof value === "number") this.check(this.fn.kiln_gui_set_number(handle, name, value), `set ${name}`);
    else if (typeof value === "boolean") this.check(this.fn.kiln_gui_set_number(handle, name, value ? 1 : 0), `set ${name}`);
    else if (value === null || value === undefined) this.check(this.fn.kiln_gui_set_text(handle, name, ""), `set ${name}`);
    else if (typeof value === "object") this.check(this.fn.kiln_gui_set_text(handle, name, JSON.stringify(value)), `set ${name}`);
    else this.check(this.fn.kiln_gui_set_text(handle, name, String(value)), `set ${name}`);
  }
  insert(parent: number, child: number, before: number): void {
    this.operations++;
    this.check(this.fn.kiln_gui_insert(parent, child, before), "insert");
  }
  remove(parent: number, child: number): void {
    this.operations++;
    this.check(this.fn.kiln_gui_remove(parent, child), "remove");
  }
  destroy(handle: number): void {
    this.operations++;
    this.check(this.fn.kiln_gui_destroy(handle), "destroy");
  }
  call(handle: number, command: string, arg = ""): void {
    this.operations++;
    this.check(this.fn.kiln_gui_call(handle, command, arg), command);
  }
  wait(ms: number): void {
    this.check(this.fn.kiln_gui_wait(ms), "wait");
  }
  describe(handle: number): Omit<AccessibilityDescription, "role"> {
    const n = this.check(this.fn.kiln_gui_describe(handle), "describe");
    return JSON.parse(this.result(n));
  }
  nextEvent(): HostEvent | null {
    if (this.fn.kiln_gui_next_event(this.header) !== 1) return null;
    const textLength = this.header32[6];
    return {
      handle: this.header32[0],
      kind: this.header32[1],
      a: this.header64[1],
      b: this.header64[2],
      text: this.result(textLength),
    };
  }

  readonly services: Services = {
    dialog: (kind, options) => {
      const n = this.check(this.fn.kiln_gui_dialog(kind, JSON.stringify(options)), `${kind} dialog`);
      return n < 0 ? null : JSON.parse(this.result(n));
    },
    clipboardRead: () => {
      const n = this.check(this.fn.kiln_gui_clipboard_read(), "clipboard");
      return n < 0 ? "" : this.result(n);
    },
    clipboardWrite: (text) => {
      this.check(this.fn.kiln_gui_clipboard_write(text), "clipboard");
    },
    clipboardClear: () => {
      this.check(this.fn.kiln_gui_clipboard_clear(), "clipboard");
    },
    isDark: () => this.check(this.fn.kiln_gui_is_dark(), "appearance") === 1,
    notify: (title, body, appName) => {
      this.check(this.fn.kiln_gui_notify(title, body, appName), "notification");
    },
    appPath: (name) => this.result(this.check(this.fn.kiln_gui_app_path(name), "path")),
    singleInstance: (appId) => this.check(this.fn.kiln_gui_single_instance(appId), "single instance") === 1,
    setMenu: (spec) => {
      this.check(this.fn.kiln_gui_set_menu(JSON.stringify(spec)), "menu");
    },
    trayCreate: (spec) => {
      const id = this.fn.kiln_gui_tray_create(JSON.stringify(spec));
      if (!id) throw new Error(`kiln:gui: tray: ${this.result(1 << 16)}`);
      return id;
    },
    trayUpdate: (id, spec) => {
      this.check(this.fn.kiln_gui_tray_update(id, JSON.stringify(spec)), "tray");
    },
    trayDestroy: (id) => {
      this.check(this.fn.kiln_gui_tray_destroy(id), "tray");
    },
    hotkeyRegister: (accelerator) => {
      const id = this.fn.kiln_gui_hotkey_register(accelerator);
      if (!id) throw new Error(`kiln:gui: shortcut ${accelerator}: ${this.result(1 << 16)}`);
      return id;
    },
    hotkeyUnregister: (id) => {
      this.check(this.fn.kiln_gui_hotkey_unregister(id), "shortcut");
    },
  };
}

function dirname(path: string): string {
  const at = path.lastIndexOf("/");
  return at > 0 ? path.slice(0, at) : ".";
}

/** Finds and binds the native library, or says why it could not. */
function openNative(): NativeHost | string {
  const modules = typeof process !== "undefined" ? (process as any).__kilnNativeModules : undefined;
  const ffi = modules && modules.ffi;
  if (!ffi) return "this engine has no FFI (the interpreter); run natively with `kiln run --native` or `kiln build`";
  const fs = modules.fs;
  const file = "libkiln_host_gui" + (ffi.suffix || ".dylib");
  const exe = typeof process.execPath === "string" ? dirname(process.execPath) : ".";
  const cwd = typeof process.cwd === "function" ? process.cwd() : ".";
  const explicit = env("KILN_GUI_LIB");
  const candidates = explicit
    ? [explicit]
    : [exe + "/" + file, exe + "/../lib/" + file, exe + "/../Frameworks/" + file, cwd + "/target/release/" + file, cwd + "/target/debug/" + file];
  const path = candidates.find((p) => !fs || fs.existsSync(p));
  if (!path) return `${file} not found (looked in ${candidates.join(", ")}); set KILN_GUI_LIB to its path`;
  let lib: any;
  try {
    lib = ffi.dlopen(path, NATIVE_FUNCTIONS);
  } catch (error) {
    return `could not load ${path}: ${error}`;
  }
  const fn = lib.functions;
  const version = fn.kiln_gui_abi_version();
  if (version !== ABI_VERSION) return `${path} speaks ABI ${version}, this library needs ${ABI_VERSION}; rebuild kiln-host-gui`;
  const host = new NativeHost(process.platform === "darwin" ? "appkit" : process.platform, fn);
  if (fn.kiln_gui_init() !== 0) {
    const scratch = new Uint8Array(4096);
    const n = fn.kiln_gui_result(scratch, scratch.length);
    return new TextDecoder().decode(scratch.subarray(0, n));
  }
  return host;
}

let activeHost: Host | null = null;
let fallbackReason: string | null = null;
let warnedHeadless = false;

/** The host widgets are created in. Chosen on first use: native when possible, else headless. */
export function currentHost(): Host {
  if (activeHost) return activeHost;
  if (env("KILN_GUI") === "headless") return (activeHost = new HeadlessHost());
  const native = openNative();
  if (typeof native === "string") {
    fallbackReason = native;
    activeHost = new HeadlessHost();
  } else {
    activeHost = native;
  }
  return activeHost;
}

/** Switches to a fresh in-memory host (for tests) and returns it. Call before creating widgets. */
export function useHeadless(): HeadlessHost {
  const host = new HeadlessHost();
  activeHost = host;
  fallbackReason = null;
  warnedHeadless = true;
  return host;
}

function warnIfHeadless(): void {
  if (warnedHeadless || !fallbackReason) return;
  warnedHeadless = true;
  console.error(`kiln:gui: no native window system (${fallbackReason}); running headless. Set KILN_GUI=headless to silence this.`);
}

// ─────────────────────────────────────────────────────────────────────────────────────────────
// 4. Widgets
//
// A Node mirrors one native widget. It exists in JavaScript from construction; its native
// widget exists only while it is attached to a shown window ("mounted"). Removing a node keeps
// it; disposing it (directly, or when the reactive owner that created it is disposed) destroys
// it and its subtree.
//
// Layout is decided here: each widget class supplies flexbox defaults, the `style` object and
// individual props refine them, and the host receives the result as one `layout` value.

type Handler = (...args: any[]) => void;

/** Props that feed the layout object rather than going to the widget directly. */
const LAYOUT_PROPS = new Set([
  "width", "height", "minWidth", "minHeight", "maxWidth", "maxHeight", "aspectRatio",
  "grow", "shrink", "basis", "alignSelf", "margin", "padding", "gap", "rowGap", "columnGap",
  "align", "justify", "wrap", "direction", "columns", "rows", "gridColumn", "gridRow", "style", "hidden",
]);

/** Props every widget forwards to its host. */
const COMMON_NATIVE = ["hidden", "disabled", "tooltip", "opacity", "accessibilityLabel", "accessibilityHint"];

const ALIGN: Record<string, string> = { start: "start", leading: "start", top: "start", center: "center", end: "end", trailing: "end", bottom: "end", stretch: "stretch", baseline: "baseline" };
const JUSTIFY: Record<string, string> = { start: "start", center: "center", end: "end", between: "space-between", around: "space-around", evenly: "space-evenly" };

/** Accessibility roles by widget kind, in AppKit's vocabulary without the `AX` prefix. */
const ROLES: Record<string, string> = {
  window: "window",
  scroll: "scrollarea",
  divider: "separator",
  text: "statictext",
  button: "button",
  textinput: "textfield",
  secureinput: "textfield",
  textarea: "textarea",
  slider: "slider",
  progress: "progressindicator",
  spinner: "busyindicator",
  toggle: "switch",
  checkbox: "checkbox",
  select: "popupbutton",
  segmented: "radiogroup",
  image: "image",
};

let nextNodeId = 1;
const mounted = new Map<number, Node>(); // host handle -> node

export class Node {
  readonly id = nextNodeId++;
  parent: Node | null = null;
  readonly children: Node[] = [];
  /** The current value of every prop, as last set or as last reported by the widget. */
  readonly props: Record<string, any> = {};
  /** The host handle while mounted, else 0. */
  handle = 0;
  disposed = false;
  private bindings: Map<string, { stop: Cleanup; value: unknown }> | null = null;
  private listeners: Map<string, Handler[]> | null = null;

  /** The host widget kind (`"text"`, `"container"`); empty for nodes with no widget. */
  get kind(): string {
    return "container";
  }

  /** Prop names this widget forwards to the host, beyond the common ones. */
  protected nativeProps(): readonly string[] {
    return [];
  }

  /** Whether children go inside this widget. */
  get holdsChildren(): boolean {
    return false;
  }

  constructor(props?: Record<string, unknown> | null, children?: unknown) {
    if (currentOwner) onCleanup(() => this.dispose());
    if (props) this.assign(props);
    if (children !== undefined) this.append(children);
  }

  /** Sets several props (and `children`, and `on…` handlers) at once. */
  assign(props: Record<string, unknown>): this {
    batch(() => {
      for (const key of Object.keys(props)) {
        if (key === "children") this.append(props.children);
        else if (key === "ref") (props.ref as (node: this) => void)(this);
        else this.set(key, props[key]);
      }
    });
    return this;
  }

  /** A prop's current value (plain, untracked: wrap reads in your own signals to react). */
  get(name: string): any {
    return this.props[name];
  }

  /**
   * Sets a prop. A function binds it: the prop follows the function's reactive value until the
   * prop is set again or the node is disposed. `onX` props set the handler for event `x`.
   */
  set(name: string, value: unknown): this {
    if (name.length > 2 && name.charCodeAt(0) === 111 && name.charCodeAt(1) === 110 && name.charCodeAt(2) < 91) {
      this.props[name] = value;
      this.handlerChanged(name, value);
      return this;
    }
    const bound = this.bindings && this.bindings.get(name);
    if (bound) {
      bound.stop();
      this.bindings!.delete(name);
    }
    if (typeof value === "function") {
      const binding = { stop: () => {}, value: undefined as unknown };
      (this.bindings || (this.bindings = new Map())).set(name, binding);
      binding.stop = runWithOwner(null, () =>
        createEffect(
          () => (value as Accessor<unknown>)(),
          (v) => {
            binding.value = v;
            this.write(name, v);
          },
        ),
      );
      return this;
    }
    this.write(name, value);
    return this;
  }

  /** Called when an `on…` handler prop is set or cleared. */
  protected handlerChanged(_name: string, _handler: unknown): void {}

  protected write(name: string, value: unknown): void {
    const prev = this.props[name];
    if (prev === value && (typeof value !== "object" || value === null)) return;
    this.props[name] = value;
    this.propChanged(name, value, prev);
  }

  /** Records a value the widget itself reported (user input) without echoing it back. */
  protected adopt(name: string, value: unknown): void {
    this.props[name] = value;
    // A native widget already shows what the person did; the headless mirror is told instead.
    if (this.handle && !currentHost().interactive) this.sendProp(name, value);
  }

  /**
   * After a handler ran for user input, a bound (controlled) prop that the program did not
   * follow is put back, so the widget shows the program's value, as a controlled input should.
   */
  protected reassert(name: string): void {
    const bound = this.bindings && this.bindings.get(name);
    if (bound && bound.value !== this.props[name]) {
      this.props[name] = bound.value;
      this.propChanged(name, bound.value, undefined);
    }
  }

  protected propChanged(name: string, value: unknown, _prev: unknown): void {
    if (!this.handle) return;
    if (LAYOUT_PROPS.has(name)) {
      currentHost().set(this.handle, "layout", this.layout());
      if (name === "hidden") currentHost().set(this.handle, "hidden", !!value);
      if ((name === "align" || name === "direction") && this.holdsChildren) {
        for (const child of this.children) child.relayout();
      }
      return;
    }
    if (COMMON_NATIVE.indexOf(name) >= 0 || this.nativeProps().indexOf(name) >= 0) {
      this.sendProp(name, value);
    }
  }

  /** Sends one prop to the host; widgets rename or convert here. */
  protected sendProp(name: string, value: unknown): void {
    currentHost().set(this.handle, name, value === undefined ? null : value);
  }

  protected relayout(): void {
    if (this.handle) currentHost().set(this.handle, "layout", this.layout());
  }

  /** Whether the parent lays children out in a row. */
  protected inRow(): boolean {
    return !!this.parent && this.parent.isRow();
  }

  isRow(): boolean {
    return false;
  }

  /** Flexbox defaults for this widget, before `style` and individual props. */
  protected layoutDefaults(_style: Record<string, unknown>): void {}

  /** The complete layout object the host receives. */
  layout(): Record<string, unknown> {
    const style: Record<string, unknown> = {};
    this.layoutDefaults(style);
    const p = this.props;
    if (p.style) Object.assign(style, p.style);
    const direct: [string, string][] = [
      ["width", "width"], ["height", "height"], ["minWidth", "minWidth"], ["minHeight", "minHeight"],
      ["maxWidth", "maxWidth"], ["maxHeight", "maxHeight"], ["aspectRatio", "aspectRatio"],
      ["grow", "flexGrow"], ["shrink", "flexShrink"], ["basis", "flexBasis"], ["margin", "margin"],
      ["padding", "padding"], ["gap", "gap"], ["rowGap", "rowGap"], ["columnGap", "columnGap"],
      ["gridColumn", "gridColumn"], ["gridRow", "gridRow"],
    ];
    for (const [prop, key] of direct) if (p[prop] !== undefined) style[key] = p[prop];
    if (typeof p.grow === "boolean") style.flexGrow = p.grow ? 1 : 0;
    if (p.alignSelf !== undefined) style.alignSelf = ALIGN[p.alignSelf] || p.alignSelf;
    if (p.align !== undefined) style.alignItems = ALIGN[p.align] || p.align;
    if (p.justify !== undefined) style.justifyContent = JUSTIFY[p.justify] || p.justify;
    if (p.wrap !== undefined) style.flexWrap = p.wrap ? "wrap" : "nowrap";
    if (p.hidden) style.display = "none";
    return style;
  }

  // ── Tree ──

  /** Appends children: nodes, text, arrays, JSX elements, or functions (reactive regions). */
  append(children: unknown): this {
    if (!this.holdsChildren) {
      if (children === undefined || children === null) return this;
      throw new Error(`kiln:gui: <${this.constructor.name}> does not take children`);
    }
    insert(this, children, null);
    return this;
  }

  /** Inserts `child` before `before` (or last). Moves a child that is attached elsewhere. */
  insertBefore(child: Node, before: Node | null): void {
    if (child === this || child.contains(this)) throw new Error("kiln:gui: a node cannot contain itself");
    if (child.parent) child.parent.detach(child);
    const at = before ? this.children.indexOf(before) : -1;
    if (at >= 0) this.children.splice(at, 0, child);
    else this.children.push(child);
    child.parent = this;
    if (this.handle && child.materializable) {
      child.mount();
      const host = currentHost();
      host.insert(this.handle, child.handle, this.mountedAfter(child));
      host.set(child.handle, "layout", child.layout());
    }
  }

  appendChild(child: Node): void {
    this.insertBefore(child, null);
  }

  /** Detaches `child`, keeping it (and its widget) for reuse. Use `dispose()` to destroy it. */
  removeChild(child: Node): void {
    if (child.parent === this) this.detach(child);
  }

  private detach(child: Node): void {
    const at = this.children.indexOf(child);
    if (at >= 0) this.children.splice(at, 1);
    child.parent = null;
    if (this.handle && child.handle) currentHost().remove(this.handle, child.handle);
  }

  contains(node: Node): boolean {
    for (let n: Node | null = node; n; n = n.parent) if (n === this) return true;
    return false;
  }

  /** Markers (region anchors) have no widget. */
  get materializable(): boolean {
    return true;
  }

  /** The handle of the first mounted sibling after `child`, which the host inserts before. */
  private mountedAfter(child: Node): number {
    for (let i = this.children.indexOf(child) + 1; i < this.children.length; i++) {
      if (this.children[i].handle) return this.children[i].handle;
    }
    return 0;
  }

  /** Creates the widget (and its subtree's) in the host with the current props. */
  mount(): void {
    if (this.handle || this.disposed || !this.materializable) return;
    const host = currentHost();
    this.handle = host.create(this.kind);
    mounted.set(this.handle, this);
    for (const name of COMMON_NATIVE) if (this.props[name] !== undefined && name !== "hidden") this.sendProp(name, this.props[name]);
    if (this.props.hidden) host.set(this.handle, "hidden", true);
    for (const name of this.nativeProps()) if (this.props[name] !== undefined) this.sendProp(name, this.props[name]);
    host.set(this.handle, "layout", this.layout());
    for (const child of this.children) {
      if (!child.materializable) continue;
      child.mount();
      host.insert(this.handle, child.handle, 0);
    }
  }

  /** Destroys this node's widget subtree and stops its bindings. Idempotent. */
  dispose(): void {
    if (this.disposed) return;
    this.disposed = true;
    if (this.parent) this.parent.detach(this);
    this.release();
  }

  private release(): void {
    this.disposed = true;
    if (this.bindings) {
      for (const b of this.bindings.values()) b.stop();
      this.bindings = null;
    }
    const children = this.children.splice(0, this.children.length);
    for (const child of children) {
      child.parent = null;
      child.release();
    }
    if (this.handle) {
      mounted.delete(this.handle);
      const handle = this.handle;
      this.handle = 0;
      currentHost().destroy(handle);
    }
    this.listeners = null;
  }

  // ── Events ──

  /** Adds a listener for event `name`; returns a function that removes it. */
  on(name: string, handler: Handler): Cleanup {
    const map = this.listeners || (this.listeners = new Map());
    const list = map.get(name) || [];
    list.push(handler);
    map.set(name, list);
    return () => {
      const at = list.indexOf(handler);
      if (at >= 0) list.splice(at, 1);
    };
  }

  /** Calls the `on<Name>` prop handler, then listeners, inside one batch. */
  emit(name: string, ...args: unknown[]): void {
    batch(() => {
      const prop = this.props["on" + name.charAt(0).toUpperCase() + name.slice(1)];
      if (typeof prop === "function") prop(...args);
      const list = this.listeners && this.listeners.get(name);
      if (list) for (const h of list.slice()) h(...args);
    });
  }

  /**
   * A user action, from the host or from `fire()` in tests: updates the props the widget
   * changed, emits the event, then restores controlled props.
   */
  receive(_event: string, _a: number, _b: number, _text: string): void {}

  /** The accessibility role kiln gives this widget; the same word on every host. */
  get role(): string {
    if (this.props.pressable) return "button";
    return ROLES[this.kind] || "group";
  }

  /** What assistive technology sees for this widget (it must be mounted: in a shown window). */
  describe(): AccessibilityDescription {
    if (!this.handle) throw new Error("kiln:gui: describe() needs the node to be in a shown window");
    return { role: this.role, ...currentHost().describe(this.handle) };
  }

  toString(): string {
    return inspect(this);
  }
}

/** An invisible anchor that marks where a reactive region's nodes go. */
class Marker extends Node {
  get materializable(): boolean {
    return false;
  }
  get kind(): string {
    return "";
  }
}

// ── Containers ──

export interface LayoutProps {
  width?: MaybeAccessor<number | string>;
  height?: MaybeAccessor<number | string>;
  minWidth?: MaybeAccessor<number | string>;
  minHeight?: MaybeAccessor<number | string>;
  maxWidth?: MaybeAccessor<number | string>;
  maxHeight?: MaybeAccessor<number | string>;
  aspectRatio?: MaybeAccessor<number>;
  grow?: MaybeAccessor<number | boolean>;
  shrink?: MaybeAccessor<number>;
  basis?: MaybeAccessor<number | string>;
  alignSelf?: MaybeAccessor<Align>;
  margin?: MaybeAccessor<Edges>;
  gridColumn?: MaybeAccessor<number | string>;
  gridRow?: MaybeAccessor<number | string>;
  hidden?: MaybeAccessor<boolean>;
  disabled?: MaybeAccessor<boolean>;
  tooltip?: MaybeAccessor<string>;
  opacity?: MaybeAccessor<number>;
  /** What VoiceOver announces, when the visible text is not enough (an icon-only button). */
  accessibilityLabel?: MaybeAccessor<string>;
  /** A longer description VoiceOver reads after a pause. */
  accessibilityHint?: MaybeAccessor<string>;
  /** Raw flexbox/grid properties (camelCase CSS names), applied before the props above. */
  style?: MaybeAccessor<Record<string, unknown>>;
  ref?: (node: any) => void;
}

export type Align = "start" | "center" | "end" | "stretch" | "baseline";
export type Edges = number | string | [number, number] | [number, number, number, number] | { top?: number; right?: number; bottom?: number; left?: number };

export interface ContainerProps extends LayoutProps {
  padding?: MaybeAccessor<Edges>;
  gap?: MaybeAccessor<number>;
  /** Cross-axis alignment of children. */
  align?: MaybeAccessor<Align>;
  /** Main-axis distribution of children. */
  justify?: MaybeAccessor<"start" | "center" | "end" | "between" | "around" | "evenly">;
  wrap?: MaybeAccessor<boolean>;
  background?: MaybeAccessor<string>;
  cornerRadius?: MaybeAccessor<number>;
  borderWidth?: MaybeAccessor<number>;
  borderColor?: MaybeAccessor<string>;
  /** Makes the whole container clickable. */
  onPress?: () => void;
  children?: unknown;
}

const PAINT = ["background", "cornerRadius", "borderWidth", "borderColor"];
const PAINT_AND_PRESS = [...PAINT, "pressable"];

/** A flexbox container. `direction` is `"column"` unless set. Prefer VStack and HStack. */
export class Box extends Node {
  constructor(props?: ContainerProps & { direction?: MaybeAccessor<"row" | "column"> }, children?: unknown) {
    super(props as Record<string, unknown>, children);
  }
  get holdsChildren(): boolean {
    return true;
  }
  protected nativeProps(): readonly string[] {
    return PAINT_AND_PRESS;
  }
  /** A container with `onPress` is clickable as a whole (a list row, a card). */
  protected handlerChanged(name: string, handler: unknown): void {
    if (name === "onPress") this.write("pressable", typeof handler === "function");
  }
  receive(event: string, _a?: number, _b?: number, _text?: string): void {
    if (event === "press" && !this.props.disabled) this.emit("press");
  }
  /** Presses the container as a person would. */
  press(): void {
    fire(this, "press");
  }
  isRow(): boolean {
    const d = this.props.direction || (this.props.style && this.props.style.flexDirection);
    return d === "row" || d === "row-reverse";
  }
  protected layoutDefaults(s: Record<string, unknown>): void {
    s.flexDirection = this.props.direction || "column";
    s.alignItems = "stretch";
    if (this.inRow()) s.minWidth = 0;
  }
}

/** Children top to bottom, stretched to the stack's width. */
export class VStack extends Box {
  protected layoutDefaults(s: Record<string, unknown>): void {
    s.flexDirection = "column";
    s.alignItems = "stretch";
    s.gap = 8;
    if (this.inRow()) s.minWidth = 0;
  }
  isRow(): boolean {
    return false;
  }
}

/** Children left to right, centred vertically. */
export class HStack extends Box {
  protected layoutDefaults(s: Record<string, unknown>): void {
    s.flexDirection = "row";
    s.alignItems = "center";
    s.gap = 8;
    if (this.inRow()) s.minWidth = 0;
  }
  isRow(): boolean {
    return true;
  }
}

/** A grouped surface: padded, rounded, filled. */
export class Card extends Box {
  protected layoutDefaults(s: Record<string, unknown>): void {
    s.flexDirection = this.props.direction || "column";
    s.alignItems = "stretch";
    s.gap = 8;
    s.padding = 14;
    if (this.inRow()) s.minWidth = 0;
  }
  mount(): void {
    const p = this.props;
    if (p.background === undefined) p.background = "fill";
    if (p.cornerRadius === undefined) p.cornerRadius = 10;
    if (p.borderWidth === undefined) p.borderWidth = 1;
    if (p.borderColor === undefined) p.borderColor = "separator";
    super.mount();
  }
}

export interface GridProps extends ContainerProps {
  /** `"1fr 2fr"`, `"200px auto"`, `"repeat(3, 1fr)"`, or a count of equal columns. */
  columns?: MaybeAccessor<string | number>;
  rows?: MaybeAccessor<string | number>;
  rowGap?: MaybeAccessor<number>;
  columnGap?: MaybeAccessor<number>;
}

/** CSS grid. Children take `gridColumn` / `gridRow` (`2`, `"span 2"`, `"1 / 3"`). */
export class Grid extends Box {
  constructor(props?: GridProps, children?: unknown) {
    super(props, children);
  }
  layout(): Record<string, unknown> {
    const s = super.layout();
    if (this.props.columns !== undefined) s.gridTemplateColumns = this.props.columns;
    if (this.props.rows !== undefined) s.gridTemplateRows = this.props.rows;
    return s;
  }
  protected layoutDefaults(s: Record<string, unknown>): void {
    s.display = "grid";
    s.gap = 8;
    if (this.inRow()) s.minWidth = 0;
  }
  isRow(): boolean {
    return false;
  }
}

/** A vertically scrolling column. Fills the space its parent gives it. */
export class ScrollView extends Box {
  get kind(): string {
    return "scroll";
  }
  protected nativeProps(): readonly string[] {
    return [];
  }
  protected layoutDefaults(s: Record<string, unknown>): void {
    s.flexGrow = 1;
    s.flexShrink = 1;
    s.minHeight = 0;
    s.minWidth = 0;
    s.gap = 8;
  }
  isRow(): boolean {
    return false;
  }
}

/** Takes the free space along its parent's axis. */
export class Spacer extends Node {
  constructor(props?: LayoutProps) {
    super(props as Record<string, unknown>);
  }
  get kind(): string {
    return "spacer";
  }
  protected layoutDefaults(s: Record<string, unknown>): void {
    s.flexGrow = 1;
    s.flexShrink = 1;
  }
}

/** A hairline across its parent's axis. */
export class Divider extends Node {
  constructor(props?: LayoutProps) {
    super(props as Record<string, unknown>);
  }
  get kind(): string {
    return "divider";
  }
  protected layoutDefaults(s: Record<string, unknown>): void {
    if (this.inRow()) {
      s.width = 1;
      s.alignSelf = "stretch";
    } else {
      s.height = 1;
      s.alignSelf = "stretch";
    }
    s.flexShrink = 0;
  }
}

// ── Window ──

export interface WindowProps extends ContainerProps {
  title?: MaybeAccessor<string>;
  /** Content size in points. */
  width?: MaybeAccessor<number>;
  height?: MaybeAccessor<number>;
  minWidth?: MaybeAccessor<number>;
  minHeight?: MaybeAccessor<number>;
  resizable?: MaybeAccessor<boolean>;
  /** The background: `"window"` (default), `"sidebar"`, `"translucent"`, `"hud"`, `"content"`. */
  material?: MaybeAccessor<string>;
  onClose?: () => void;
  onResize?: (width: number, height: number) => void;
}

const openWindows = new Set<Window>();

function findAutofocus(node: Node): Node | null {
  if (node.props.autofocus && node.handle) return node;
  for (const child of node.children) {
    const found = findAutofocus(child);
    if (found) return found;
  }
  return null;
}

/** A top-level window. Its content is a padded column; `show()` puts it on screen. */
export class Window extends Box {
  constructor(props?: WindowProps, children?: unknown) {
    super(props, children);
  }
  get kind(): string {
    return "window";
  }
  protected nativeProps(): readonly string[] {
    return ["title", "width", "height", "minWidth", "minHeight", "resizable", "material", "background", "pressable"];
  }
  protected layoutDefaults(s: Record<string, unknown>): void {
    s.flexDirection = "column";
    s.alignItems = "stretch";
    s.padding = 20;
    s.gap = 12;
  }
  isRow(): boolean {
    return false;
  }
  layout(): Record<string, unknown> {
    // A window's size props size the window, not its content box.
    const s = super.layout();
    delete s.width;
    delete s.height;
    delete s.minWidth;
    delete s.minHeight;
    return s;
  }
  protected propChanged(name: string, value: unknown, prev: unknown): void {
    if (name === "width" || name === "height" || name === "minWidth" || name === "minHeight") {
      if (this.handle) this.sendProp(name, value);
      return;
    }
    super.propChanged(name, value, prev);
  }

  get title(): string {
    return this.props.title || "";
  }
  set title(value: string) {
    this.set("title", value);
  }
  get visible(): boolean {
    return openWindows.has(this);
  }

  show(): void {
    if (this.disposed) throw new Error("kiln:gui: this window was disposed");
    const host = currentHost();
    if (!host.interactive) warnIfHeadless();
    if (this.props.width === undefined) this.props.width = 800;
    if (this.props.height === undefined) this.props.height = 600;
    this.mount();
    host.call(this.handle, "show");
    const autofocus = findAutofocus(this);
    if (autofocus) host.call(autofocus.handle, "focus");
    openWindows.add(this);
    eventLoop.start();
  }

  /** Closes the window (it can be shown again) and calls `onClose`. */
  close(): void {
    if (!openWindows.has(this)) return;
    openWindows.delete(this);
    if (this.handle) currentHost().call(this.handle, "close");
    this.emit("close");
    eventLoop.windowClosed();
  }

  minimize(): void {
    if (this.handle) currentHost().call(this.handle, "minimize");
  }
  maximize(): void {
    if (this.handle) currentHost().call(this.handle, "maximize");
  }
  unmaximize(): void {
    if (this.handle) currentHost().call(this.handle, "unmaximize");
  }
  setFullscreen(on: boolean): void {
    if (this.handle) currentHost().call(this.handle, "fullscreen", on ? "true" : "false");
  }
  center(): void {
    if (this.handle) currentHost().call(this.handle, "center");
  }
  /** Writes the window's contents as a PNG (native hosts only). */
  snapshot(path: string): void {
    if (!this.handle) throw new Error("kiln:gui: show the window before taking a snapshot");
    currentHost().call(this.handle, "snapshot", path);
  }

  dispose(): void {
    openWindows.delete(this);
    super.dispose();
    eventLoop.windowClosed();
  }

  receive(event: string, a: number, b: number): void {
    if (event === "close") {
      if (!openWindows.has(this)) return;
      openWindows.delete(this);
      this.emit("close");
      eventLoop.windowClosed();
    } else if (event === "resize") {
      this.adopt("width", a);
      this.adopt("height", b);
      this.emit("resize", a, b);
    }
  }
}

// ── Leaves ──

export interface TextStyleProps {
  /** `largeTitle`, `title`, `title2`, `title3`, `headline`, `body`, `callout`, `subheadline`, `footnote`, `caption`. */
  variant?: MaybeAccessor<string>;
  size?: MaybeAccessor<number>;
  weight?: MaybeAccessor<"light" | "regular" | "medium" | "semibold" | "bold" | "heavy">;
  monospace?: MaybeAccessor<boolean>;
  /** A semantic color (`secondary`, `accent`, `red`, `success`…) or `#rrggbb`. */
  color?: MaybeAccessor<string>;
  align?: MaybeAccessor<"start" | "center" | "end" | "justify">;
}

const TEXT_STYLE = ["variant", "size", "weight", "monospace", "color"];

/** Builds a text prop from children: strings, numbers and functions, concatenated. */
function textOf(children: unknown, what: string): MaybeAccessor<string> {
  const parts = flatten(children);
  for (const p of parts) {
    if (p instanceof Node || p instanceof JsxElement) throw new Error(`kiln:gui: only text can go inside ${what}`);
  }
  const dynamic = parts.some((p) => typeof p === "function");
  const join = () => parts.map((p) => (typeof p === "function" ? str((p as Accessor<unknown>)()) : str(p))).join("");
  return dynamic ? join : join();
}

function str(v: unknown): string {
  return v === null || v === undefined || v === false || v === true ? "" : String(v);
}

/** A label. Wraps to its width; `lines={1}` truncates instead. */
export class Text extends Node {
  constructor(props?: (LayoutProps & TextStyleProps & { text?: MaybeAccessor<string | number>; lines?: MaybeAccessor<number>; selectable?: MaybeAccessor<boolean>; children?: unknown }) | MaybeAccessor<string>) {
    super(typeof props === "object" && props !== null ? (props as Record<string, unknown>) : props !== undefined ? { text: props } : null);
  }
  get kind(): string {
    return "text";
  }
  protected nativeProps(): readonly string[] {
    return ["text", "lines", "selectable", "align", ...TEXT_STYLE];
  }
  append(children: unknown): this {
    this.set("text", textOf(children, "<text>"));
    return this;
  }
  protected sendProp(name: string, value: unknown): void {
    super.sendProp(name, name === "text" ? str(value) : value);
  }
  protected layoutDefaults(s: Record<string, unknown>): void {
    s.flexShrink = 1;
    if (this.inRow()) s.minWidth = 0;
  }
  get text(): string {
    return str(this.props.text);
  }
  set text(value: MaybeAccessor<string>) {
    this.set("text", value);
  }
}

export interface ButtonProps extends LayoutProps {
  label?: MaybeAccessor<string>;
  /** `primary` (the default button, Return), `secondary`, `destructive`, `plain` (borderless
   * text), `link` (borderless, accent-colored). */
  variant?: MaybeAccessor<"primary" | "secondary" | "destructive" | "plain" | "link">;
  size?: MaybeAccessor<"sm" | "md" | "lg">;
  /** An SF Symbol name or alias (`"plus"`, `"trash"`, `"gear"`). */
  icon?: MaybeAccessor<string>;
  onPress?: () => void;
  children?: unknown;
}

export class Button extends Node {
  constructor(props?: ButtonProps | MaybeAccessor<string>, onPress?: () => void) {
    super(typeof props === "object" && props !== null ? (props as Record<string, unknown>) : props !== undefined ? { label: props, onPress } : null);
  }
  get kind(): string {
    return "button";
  }
  protected nativeProps(): readonly string[] {
    return ["label", "variant", "size", "icon"];
  }
  append(children: unknown): this {
    this.set("label", textOf(children, "<button>"));
    return this;
  }
  protected sendProp(name: string, value: unknown): void {
    if (name === "size") super.sendProp("controlSize", value);
    else super.sendProp(name, name === "label" ? str(value) : value);
  }
  protected layoutDefaults(s: Record<string, unknown>): void {
    s.flexShrink = 0;
    if (!this.inRow()) s.alignSelf = "start";
  }
  get label(): string {
    return str(this.props.label);
  }
  set label(value: MaybeAccessor<string>) {
    this.set("label", value);
  }
  /** Presses the button as a person would (ignored while disabled). */
  press(): void {
    fire(this, "press");
  }
  receive(event: string): void {
    if (event === "press" && !this.props.disabled) this.emit("press");
  }
}

export interface TextInputProps extends LayoutProps, TextStyleProps {
  value?: MaybeAccessor<string>;
  placeholder?: MaybeAccessor<string>;
  /** Hides what is typed (a password field). Fixed at creation. */
  secure?: boolean;
  /** Takes keyboard focus when its window is shown. */
  autofocus?: boolean;
  /** Every edit. */
  onChange?: (value: string) => void;
  /** Return pressed. */
  onSubmit?: (value: string) => void;
}

/** A single-line text field. */
export class TextInput extends Node {
  constructor(props?: TextInputProps) {
    super(props as Record<string, unknown>);
  }
  get kind(): string {
    return this.props.secure ? "secureinput" : "textinput";
  }
  protected nativeProps(): readonly string[] {
    return ["value", "placeholder", "align", ...TEXT_STYLE];
  }
  protected layoutDefaults(s: Record<string, unknown>): void {
    s.minWidth = 80;
    if (this.inRow()) {
      s.flexGrow = 1;
      s.flexShrink = 1;
    }
  }
  get value(): string {
    return str(this.props.value);
  }
  set value(value: MaybeAccessor<string>) {
    this.set("value", value);
  }
  focus(): void {
    if (this.handle) currentHost().call(this.handle, "focus");
  }
  receive(event: string, _a: number, _b: number, text: string): void {
    if (event === "input" || event === "change") {
      this.adopt("value", text);
      this.emit("change", text);
      this.reassert("value");
    } else if (event === "submit") {
      this.adopt("value", text);
      this.emit("submit", text);
    }
  }
}

/** A multi-line, scrolling plain-text editor. */
export class TextArea extends Node {
  constructor(props?: LayoutProps & TextStyleProps & { value?: MaybeAccessor<string>; autofocus?: boolean; onChange?: (value: string) => void }) {
    super(props as Record<string, unknown>);
  }
  get kind(): string {
    return "textarea";
  }
  protected nativeProps(): readonly string[] {
    return ["value", ...TEXT_STYLE];
  }
  protected layoutDefaults(s: Record<string, unknown>): void {
    s.minHeight = 80;
    s.minWidth = 80;
    s.flexGrow = 1;
    s.flexShrink = 1;
  }
  get value(): string {
    return str(this.props.value);
  }
  set value(value: MaybeAccessor<string>) {
    this.set("value", value);
  }
  focus(): void {
    if (this.handle) currentHost().call(this.handle, "focus");
  }
  receive(event: string, _a: number, _b: number, text: string): void {
    if (event === "input" || event === "change") {
      this.adopt("value", text);
      this.emit("change", text);
      this.reassert("value");
    }
  }
}

export interface RangeProps extends LayoutProps {
  value?: MaybeAccessor<number>;
  min?: MaybeAccessor<number>;
  max?: MaybeAccessor<number>;
}

export class Slider extends Node {
  constructor(props?: RangeProps & { step?: MaybeAccessor<number>; onChange?: (value: number) => void }) {
    super(props as Record<string, unknown>);
  }
  get kind(): string {
    return "slider";
  }
  protected nativeProps(): readonly string[] {
    return ["min", "max", "value"];
  }
  protected layoutDefaults(s: Record<string, unknown>): void {
    s.minWidth = 100;
    if (this.inRow()) {
      s.flexGrow = 1;
      s.flexShrink = 1;
    }
  }
  get value(): number {
    return Number(this.props.value) || 0;
  }
  set value(value: MaybeAccessor<number>) {
    this.set("value", value);
  }
  receive(event: string, a: number): void {
    if (event !== "change") return;
    const step = Number(this.props.step) || 0;
    const min = Number(this.props.min) || 0;
    const value = step > 0 ? min + Math.round((a - min) / step) * step : a;
    if (value === this.props.value) return;
    this.adopt("value", value);
    this.emit("change", value);
    if (step > 0 && value !== a) this.propChanged("value", value, a);
    this.reassert("value");
  }
}

export class ProgressBar extends Node {
  constructor(props?: RangeProps & { indeterminate?: MaybeAccessor<boolean> }) {
    super(props as Record<string, unknown>);
  }
  get kind(): string {
    return "progress";
  }
  protected nativeProps(): readonly string[] {
    return ["min", "max", "value", "indeterminate"];
  }
  protected layoutDefaults(s: Record<string, unknown>): void {
    s.minWidth = 60;
    if (this.inRow()) {
      s.flexGrow = 1;
      s.flexShrink = 1;
    }
  }
  get value(): number {
    return Number(this.props.value) || 0;
  }
  set value(value: MaybeAccessor<number>) {
    this.set("value", value);
  }
}

/** A small spinning activity indicator. */
export class Spinner extends Node {
  constructor(props?: LayoutProps) {
    super(props as Record<string, unknown>);
  }
  get kind(): string {
    return "spinner";
  }
  protected layoutDefaults(s: Record<string, unknown>): void {
    s.flexShrink = 0;
    if (!this.inRow()) s.alignSelf = "start";
  }
}

abstract class Checkable extends Node {
  protected nativeProps(): readonly string[] {
    return ["checked"];
  }
  protected layoutDefaults(s: Record<string, unknown>): void {
    s.flexShrink = 0;
    if (!this.inRow()) s.alignSelf = "start";
  }
  protected sendProp(name: string, value: unknown): void {
    super.sendProp(name, name === "checked" ? !!value : value);
  }
  get checked(): boolean {
    return !!this.props.checked;
  }
  set checked(value: MaybeAccessor<boolean>) {
    this.set("checked", value);
  }
  /** Flips the control as a person would. */
  toggle(): void {
    fire(this, "change", this.checked ? 0 : 1);
  }
  receive(event: string, a: number): void {
    if (event !== "change" || this.props.disabled) return;
    const checked = a !== 0;
    this.adopt("checked", checked);
    this.emit("change", checked);
    this.reassert("checked");
  }
}

/** An on/off switch. */
export class Toggle extends Checkable {
  constructor(props?: LayoutProps & { checked?: MaybeAccessor<boolean>; onChange?: (checked: boolean) => void }) {
    super(props as Record<string, unknown>);
  }
  get kind(): string {
    return "toggle";
  }
}

/** A checkbox with its label. */
export class Checkbox extends Checkable {
  constructor(props?: LayoutProps & { checked?: MaybeAccessor<boolean>; label?: MaybeAccessor<string>; onChange?: (checked: boolean) => void; children?: unknown }) {
    super(props as Record<string, unknown>);
  }
  get kind(): string {
    return "checkbox";
  }
  protected nativeProps(): readonly string[] {
    return ["checked", "label"];
  }
  append(children: unknown): this {
    this.set("label", textOf(children, "<checkbox>"));
    return this;
  }
}

abstract class Choice extends Node {
  protected nativeProps(): readonly string[] {
    return ["options", "selected"];
  }
  protected sendProp(name: string, value: unknown): void {
    if (name === "selected") super.sendProp("selectedIndex", Number(value) || 0);
    else super.sendProp(name, value);
  }
  protected layoutDefaults(s: Record<string, unknown>): void {
    s.flexShrink = 0;
    if (!this.inRow()) s.alignSelf = "start";
  }
  get selected(): number {
    return Number(this.props.selected) || 0;
  }
  set selected(value: MaybeAccessor<number>) {
    this.set("selected", value);
  }
  /** Chooses option `index` as a person would. */
  choose(index: number): void {
    fire(this, "change", index);
  }
  receive(event: string, a: number): void {
    if (event !== "change" || this.props.disabled) return;
    const options = (this.props.options || []) as string[];
    this.adopt("selected", a);
    this.emit("change", a, options[a]);
    this.reassert("selected");
  }
}

export interface ChoiceProps extends LayoutProps {
  options: MaybeAccessor<string[]>;
  selected?: MaybeAccessor<number>;
  onChange?: (index: number, option: string) => void;
}

/** A pop-up menu of options. */
export class Select extends Choice {
  constructor(props?: ChoiceProps) {
    super(props as unknown as Record<string, unknown>);
  }
  get kind(): string {
    return "select";
  }
}

/** A row of mutually exclusive segments. */
export class SegmentedControl extends Choice {
  constructor(props?: ChoiceProps) {
    super(props as unknown as Record<string, unknown>);
  }
  get kind(): string {
    return "segmented";
  }
}

/** An SF Symbol (`symbol`) or an image file (`src`). */
export class Image extends Node {
  constructor(props?: LayoutProps & { symbol?: MaybeAccessor<string>; src?: MaybeAccessor<string>; size?: MaybeAccessor<number>; color?: MaybeAccessor<string> }) {
    super(props as Record<string, unknown>);
  }
  get kind(): string {
    return "image";
  }
  protected nativeProps(): readonly string[] {
    return ["symbol", "src", "size", "color"];
  }
  protected sendProp(name: string, value: unknown): void {
    super.sendProp(name === "symbol" ? "src" : name, value);
  }
  protected layoutDefaults(s: Record<string, unknown>): void {
    s.flexShrink = 0;
    if (!this.inRow()) s.alignSelf = "start";
  }
}

/** Delivers a user action to `node` exactly as the native host would. For tests and scripts. */
export function fire(node: Node, event: "press" | "change" | "input" | "submit" | "close" | "resize", value?: number | string | boolean, extra?: number): void {
  const text = typeof value === "string" ? value : "";
  const a = typeof value === "number" ? value : typeof value === "boolean" ? (value ? 1 : 0) : 0;
  batch(() => node.receive(event, a, extra || 0, text));
}

/** The node tree as indented text: tags and the props that matter. */
export function inspect(node: Node, depth = 0): string {
  if (!node.materializable) return "";
  const skip = new Set(["style"]);
  const props = Object.keys(node.props)
    .filter((k) => !skip.has(k) && typeof node.props[k] !== "function")
    .sort()
    .map((k) => `${k}=${JSON.stringify(node.props[k])}`)
    .join(" ");
  const tag = node.constructor.name;
  let out = `${"  ".repeat(depth)}<${tag}${props ? " " + props : ""}>`;
  for (const c of node.children) {
    const line = inspect(c, depth + 1);
    if (line) out += "\n" + line;
  }
  return out;
}

// ─────────────────────────────────────────────────────────────────────────────────────────────
// 5. JSX
//
// `jsx()` returns a description, not a widget: components must run parent-first so that
// context, ownership and conditional rendering work. Descriptions turn into widgets when
// inserted (or when `h()` is used, which builds immediately).

export type Component<P = any> = (props: P) => unknown;

type WidgetClass = new (props?: any, children?: any) => Node;

function isWidgetClass(f: unknown): f is WidgetClass {
  return typeof f === "function" && !!(f as any).prototype && (f as any).prototype instanceof Node;
}

const INTRINSICS: Record<string, WidgetClass> = {
  window: Window,
  vstack: VStack,
  hstack: HStack,
  box: Box,
  grid: Grid,
  card: Card,
  scroll: ScrollView,
  text: Text,
  button: Button,
  input: TextInput,
  textarea: TextArea,
  slider: Slider,
  toggle: Toggle,
  checkbox: Checkbox,
  select: Select,
  segmented: SegmentedControl,
  progress: ProgressBar,
  spinner: Spinner,
  image: Image,
  divider: Divider,
  spacer: Spacer,
};

export class JsxElement {
  readonly type: string | Component | WidgetClass;
  readonly props: Record<string, unknown>;

  constructor(type: string | Component | WidgetClass, props: Record<string, unknown>) {
    this.type = type;
    this.props = props;
  }

  /** Builds this element: a widget for a tag or class, whatever a component returns. */
  render(): unknown {
    const { type, props } = this;
    if (typeof type === "string") {
      const Widget = INTRINSICS[type];
      if (!Widget) throw new Error(`kiln:gui: unknown element <${type}> (known: ${Object.keys(INTRINSICS).join(", ")})`);
      return new Widget(props);
    }
    if (isWidgetClass(type)) return new type(props);
    return untrack(() => (type as Component)(props));
  }
}

export function jsx(type: string | Component | WidgetClass, props: Record<string, unknown> | null, key?: unknown): JsxElement {
  void key;
  return new JsxElement(type, props || {});
}

export const jsxs = jsx;
export const jsxDEV = jsx;

export function Fragment(props: { children?: unknown }): unknown {
  return props.children;
}

/** Hyperscript: builds immediately and returns the widget (or whatever a component returns). */
export function h(type: string | Component | WidgetClass, props?: Record<string, unknown> | null, ...children: unknown[]): any {
  const all = Object.assign({}, props || {});
  if (children.length > 0) all.children = children.length === 1 ? children[0] : children;
  const built = new JsxElement(type, all).render();
  return built;
}

function flatten(value: unknown, out: unknown[] = []): unknown[] {
  if (Array.isArray(value)) for (const v of value) flatten(v, out);
  else if (value !== undefined && value !== null && value !== false && value !== true && value !== "") out.push(value);
  return out;
}

/**
 * Renders elements (untracked, owned by the current owner) and flattens arrays, leaving
 * functions in place. Control flow calls this where it decides what to show, so the widgets
 * belong to that decision and survive anything else re-running.
 */
function renderChildren(value: unknown, out: (Node | Accessor<unknown>)[] = []): (Node | Accessor<unknown>)[] {
  if (value === undefined || value === null || typeof value === "boolean") return out;
  if (value instanceof Node) out.push(value);
  else if (value instanceof JsxElement) renderChildren(untrack(() => value.render()), out);
  else if (Array.isArray(value)) for (const v of value) renderChildren(v, out);
  else if (typeof value === "function") out.push(value as Accessor<unknown>);
  else out.push(new Text({ text: String(value) }));
  return out;
}

/** Renders and then calls every function (tracked), down to a flat list of nodes. */
function toNodes(value: unknown, out: Node[] = []): Node[] {
  for (const item of renderChildren(value)) {
    if (typeof item === "function") toNodes(item(), out);
    else out.push(item);
  }
  return out;
}

/**
 * Inserts `value` into `parent` before `before`. Static values are inserted once; a function
 * becomes a region that re-renders its part of the children whenever what it reads changes,
 * leaving the rest of the parent alone.
 */
function insert(parent: Node, value: unknown, before: Node | null): void {
  if (value === undefined || value === null || typeof value === "boolean") return;
  if (Array.isArray(value)) {
    for (const v of value) insert(parent, v, before);
    return;
  }
  if (value instanceof JsxElement) {
    insert(parent, value.render(), before);
    return;
  }
  if (value instanceof Node) {
    parent.insertBefore(value, before);
    return;
  }
  if (typeof value !== "function") {
    parent.insertBefore(new Text({ text: String(value) }), before);
    return;
  }
  const marker = new Marker();
  parent.insertBefore(marker, before);
  let current: Node[] = [];
  let reusedText: Text | null = null;
  onCleanup(() => {
    for (const n of current) if (n.parent === parent) parent.removeChild(n);
    if (reusedText) reusedText.dispose();
    marker.dispose();
  });
  createEffect(() => {
    const raw = (value as Accessor<unknown>)();
    // A region that renders plain text keeps one Text widget and updates it.
    if (typeof raw === "string" || typeof raw === "number") {
      if (!reusedText) reusedText = runWithOwner(null, () => new Text());
      reusedText.text = String(raw);
      return reconcile(parent, marker, current, (current = [reusedText]));
    }
    const next = toNodes(raw);
    if (reusedText && next.indexOf(reusedText) < 0) {
      const old = reusedText;
      reusedText = null;
      if (old.parent === parent) parent.removeChild(old);
      old.dispose();
    }
    reconcile(parent, marker, current, next);
    current = next;
  });
}

function reconcile(parent: Node, marker: Node, prev: Node[], next: Node[]): void {
  const keep = new Set(next);
  for (const n of prev) if (!keep.has(n) && n.parent === parent) parent.removeChild(n);
  let anchor: Node = marker;
  for (let i = next.length - 1; i >= 0; i--) {
    const n = next[i];
    if (n.parent !== parent || parent.children[parent.children.indexOf(anchor) - 1] !== n) parent.insertBefore(n, anchor);
    anchor = n;
  }
}

/** Renders `code` into `parent` inside a new root. Returns a function that disposes it all. */
export function render(code: () => unknown, parent: Node): Cleanup {
  return createRoot((dispose) => {
    insert(parent, untrack(code), null);
    return dispose;
  });
}

/**
 * Builds the app, shows its windows and resolves when the last one closes (or the person
 * quits). A root that is not a window is placed in one.
 */
export function runApp(app: unknown | (() => unknown)): Promise<void> {
  return new Promise((resolve) => {
    createRoot((dispose) => {
      const nodes = untrack(() => toNodes(typeof app === "function" && !isWidgetClass(app) ? (app as Accessor<unknown>)() : app));
      const windows = nodes.map((n) => (n instanceof Window ? n : new Window({ title: "kiln" }, n)));
      eventLoop.whenAllClosed(() => {
        dispose();
        resolve();
      });
      for (const w of windows) w.show();
      const snapshot = env("KILN_GUI_SNAPSHOT");
      if (snapshot && windows.length > 0) {
        setTimeout(() => {
          eventLoop.drainOnce();
          windows[0].snapshot(snapshot);
          app_.quit();
        }, 500);
      }
    });
  });
}

// ─────────────────────────────────────────────────────────────────────────────────────────────
// 6. Event loop
//
// The OS delivers events to the main thread's run loop; the program's own work (timers, I/O)
// runs on kiln's loop. While a window is open, each tick processes OS events for a bounded
// slice, dispatches what arrived as one batch, then yields so kiln's loop runs its turns. The
// slice is short (8 ms) while something is happening and long (50 ms) when idle: input is
// handled at once in both, and timers and I/O are at most one slice late. A runtime hook that
// lets the host wait on both loops at once would remove the slice (ADR 0074, follow-up).

const ACTIVE_SLICE_MS = 8;
const IDLE_SLICE_MS = 50;
const ACTIVE_WINDOW_MS = 500;

const eventLoop = {
  running: false,
  activeUntil: 0,
  lastOperations: 0,
  keepAlive: 0,
  closedWaiters: [] as Cleanup[],

  start(): void {
    if (this.running || !currentHost().interactive) return;
    this.running = true;
    setTimeout(tick, 0);
  },

  /** Dispatches every queued host event as one batch. Returns how many there were. */
  drainOnce(): number {
    const host = currentHost();
    let count = 0;
    batch(() => {
      for (let e = host.nextEvent(); e; e = host.nextEvent()) {
        count++;
        dispatch(e);
      }
    });
    return count;
  },

  whenAllClosed(fn: Cleanup): void {
    this.closedWaiters.push(fn);
  },

  windowClosed(): void {
    if (openWindows.size > 0) return;
    const waiters = this.closedWaiters.splice(0, this.closedWaiters.length);
    for (const w of waiters) w();
  },
};

/** Dispatches every event the host has queued, as one batch. The event loop calls this; tests
 * and programs with their own loop can too. Returns how many events there were. */
export function processEvents(): number {
  return eventLoop.drainOnce();
}

function tick(): void {
  const host = currentHost();
  let events = eventLoop.drainOnce();
  const busy = Date.now() < eventLoop.activeUntil;
  host.wait(busy ? ACTIVE_SLICE_MS : IDLE_SLICE_MS);
  events += eventLoop.drainOnce();
  if (events > 0 || host.operations !== eventLoop.lastOperations) {
    eventLoop.activeUntil = Date.now() + ACTIVE_WINDOW_MS;
    eventLoop.lastOperations = host.operations;
  }
  if (openWindows.size > 0 || eventLoop.keepAlive > 0) {
    // A non-zero delay: kiln's loop parks for it, which lets its I/O tasks run.
    setTimeout(tick, 1);
  } else {
    eventLoop.running = false;
  }
}

const menuHandlers = new Map<string, () => void>();
const hotkeyHandlers = new Map<number, () => void>();
const trayHandlers = new Map<number, () => void>();

function dispatch(e: HostEvent): void {
  if (e.handle === 0) {
    if (e.kind === EVENT_MENU) {
      const handler = menuHandlers.get(e.text);
      if (handler) handler();
    } else if (e.kind === EVENT_HOTKEY) {
      const handler = hotkeyHandlers.get(e.a);
      if (handler) handler();
    } else if (e.kind === EVENT_TRAY) {
      const handler = trayHandlers.get(e.a);
      if (handler) handler();
    } else if (e.kind === EVENT_QUIT) {
      app_.quit();
    }
    return;
  }
  const node = mounted.get(e.handle);
  const name = EVENT_NAMES[e.kind];
  if (node && name) node.receive(name, e.a, e.b, e.text);
}

// ─────────────────────────────────────────────────────────────────────────────────────────────
// 7. OS services

export interface OpenOptions {
  title?: string;
  defaultPath?: string;
  filters?: FileFilter[];
}

export const dialog = {
  /** One file, or null when cancelled. */
  async openFile(options: OpenOptions = {}): Promise<string | null> {
    return currentHost().services.dialog("open", { ...options }) as string | null;
  },
  async openFiles(options: OpenOptions = {}): Promise<string[]> {
    return (currentHost().services.dialog("open", { ...options, multiple: true }) as string[] | null) || [];
  },
  async saveFile(options: OpenOptions & { fileName?: string } = {}): Promise<string | null> {
    return currentHost().services.dialog("save", { ...options }) as string | null;
  },
  async pickFolder(options: OpenOptions = {}): Promise<string | null> {
    return currentHost().services.dialog("folder", { ...options }) as string | null;
  },
  async message(message: string, options: { title?: string; detail?: string; kind?: "info" | "warning" | "error" } = {}): Promise<void> {
    currentHost().services.dialog("message", { ...options, message });
  },
  /** OK/Cancel; true for OK. */
  async confirm(message: string, options: { title?: string; detail?: string; kind?: "info" | "warning" | "error" } = {}): Promise<boolean> {
    return currentHost().services.dialog("confirm", { ...options, message }) === true;
  },
};

export const clipboard = {
  async readText(): Promise<string> {
    return currentHost().services.clipboardRead();
  },
  async writeText(text: string): Promise<void> {
    currentHost().services.clipboardWrite(String(text));
  },
  async clear(): Promise<void> {
    currentHost().services.clipboardClear();
  },
};

/** The system appearance. `isDark()` asks the OS each time. */
export const appearance = {
  isDark(): boolean {
    return currentHost().services.isDark();
  },
};

export function notify(title: string, body = "", appName = app_.name): void {
  currentHost().services.notify(title, body, appName);
}

export type AppPathName = "home" | "appData" | "userData" | "config" | "cache" | "temp" | "desktop" | "documents" | "downloads";

export interface MenuItem {
  id?: string;
  label: string;
  /** `"CmdOrCtrl+S"`, `"Shift+Cmd+Z"`. */
  accelerator?: string;
  enabled?: boolean;
  checked?: boolean;
  onClick?: () => void;
  items?: (MenuItem | "separator")[];
}

export interface Menu {
  label: string;
  items: (MenuItem | "separator")[];
}

let nextMenuId = 1;

/** Turns menus into the host's JSON shape, registering click handlers by id. */
function menuSpec(items: (MenuItem | "separator")[], handlers: Map<string, () => void>): unknown[] {
  return items.map((item) => {
    if (item === "separator") return { separator: true };
    const id = item.id || `menu-${nextMenuId++}`;
    if (item.onClick) handlers.set(id, item.onClick);
    const spec: Record<string, unknown> = { id, label: item.label };
    if (item.accelerator) spec.accelerator = item.accelerator;
    if (item.enabled === false) spec.enabled = false;
    if (item.checked !== undefined) spec.checked = item.checked;
    if (item.items) spec.items = menuSpec(item.items, handlers);
    return spec;
  });
}

const app_ = {
  name: "kiln",

  path(name: AppPathName): string {
    return currentHost().services.appPath(name);
  },

  /** True when no other process holds `appId`; the lock lasts until this process exits. */
  requestSingleInstance(appId: string): boolean {
    return currentHost().services.singleInstance(appId);
  },

  /** Sets the menus between Edit and Window in the menu bar. The app, Edit and Window menus are always there. */
  setMenu(menus: Menu[]): void {
    for (const key of Array.from(menuHandlers.keys())) if (!key.startsWith("tray:")) menuHandlers.delete(key);
    currentHost().services.setMenu(menus.map((m) => ({ label: m.label, items: menuSpec(m.items, menuHandlers) })));
  },

  /** Closes every window, which ends `runApp`. */
  quit(): void {
    for (const w of Array.from(openWindows)) w.close();
  },
};

export { app_ as app };

/** A menu-bar (status) item with a title and an optional menu. */
export class Tray {
  readonly id: number;
  private disposed = false;

  constructor(options: { title?: string; tooltip?: string; menu?: (MenuItem | "separator")[]; onClick?: () => void }) {
    this.id = currentHost().services.trayCreate(this.spec(options));
    if (options.onClick) trayHandlers.set(this.id, options.onClick);
    eventLoop.keepAlive++;
    eventLoop.start();
    if (currentOwner) onCleanup(() => this.dispose());
  }

  private spec(options: { title?: string; tooltip?: string; menu?: (MenuItem | "separator")[] }): Record<string, unknown> {
    const spec: Record<string, unknown> = {};
    if (options.title !== undefined) spec.title = options.title;
    if (options.tooltip !== undefined) spec.tooltip = options.tooltip;
    if (options.menu) spec.menu = menuSpec(options.menu, menuHandlers);
    return spec;
  }

  update(options: { title?: string; tooltip?: string; menu?: (MenuItem | "separator")[] }): void {
    currentHost().services.trayUpdate(this.id, this.spec(options));
  }

  dispose(): void {
    if (this.disposed) return;
    this.disposed = true;
    trayHandlers.delete(this.id);
    currentHost().services.trayDestroy(this.id);
    eventLoop.keepAlive--;
  }
}

/** System-wide keyboard shortcuts, delivered even when the app is in the background. */
export const shortcuts = {
  /** Registers `accelerator` (`"Cmd+Shift+K"`); returns a function that unregisters it. */
  register(accelerator: string, handler: () => void): Cleanup {
    const id = currentHost().services.hotkeyRegister(accelerator);
    hotkeyHandlers.set(id, handler);
    eventLoop.keepAlive++;
    eventLoop.start();
    let done = false;
    return () => {
      if (done) return;
      done = true;
      hotkeyHandlers.delete(id);
      currentHost().services.hotkeyUnregister(id);
      eventLoop.keepAlive--;
    };
  },
};
