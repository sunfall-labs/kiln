// Type-checked by `scripts/gen-kiln-types.sh --check`: the declarations describe the API a
// program actually writes. Never run.
import { Actor, spawnActor, connectActor, isolate, prepareTransfer, transfer, ActorClosedError,
  ActorRemoteError, type ActorRef, type ActorAddress } from "kiln:actors";
import { share, isShared, type Shared } from "kiln:shared";
import { run, type RunResult } from "kiln:process";

class Counter extends Actor {
  constructor(private count: number) { super(); }
  increment(by: number): number { return (this.count += by); }
  async later(): Promise<string> { return "done"; }
}

async function actors(): Promise<void> {
  const owner = await spawnActor<Counter>({ module: new URL("file:///counter.ts"), export: "Counter", args: [0] });
  const next: number = await owner.ref.call("increment", [1]);
  const later: string = await owner.ref.call("later", []);
  const address: ActorAddress<Counter> = owner.ref.address;
  const other: ActorRef<Counter> = connectActor(address);
  await other.call("increment", [2], { signal: AbortSignal.timeout(10) });
  // @ts-expect-error -- arguments are checked against the method
  await owner.ref.call("increment", ["one"]);
  // @ts-expect-error -- only methods can be called
  await owner.ref.call("count", []);
  await owner.ref.call("increment", [1]).catch((error: unknown) => {
    if (error instanceof ActorRemoteError) error.remoteName.toUpperCase();
    if (error instanceof ActorClosedError) error.message.length;
  });
  const bytes = new ArrayBuffer(8);
  await owner.ref.call("increment", [transfer(bytes) as unknown as number]);
  transfer(isolate(() => ({ graph: [1, 2] })));
  transfer(prepareTransfer({ plain: true }));
  await owner.stop({ force: true });
  void next; void later;
}

function shared(): void {
  const frozen: Shared<{ list: number[]; nested: { a: string } }> = share({ list: [1], nested: { a: "x" } });
  const flag: boolean = isShared(frozen);
  // @ts-expect-error -- a shared graph is read-only
  frozen.nested.a = "y";
  const rates = share(new Map([["acme", { rate: 2 }]]));
  const rate: number | undefined = rates.get("acme")?.rate;
  const tags = share(new Set(["a"]));
  const tagged: boolean = tags.has("a");
  // @ts-expect-error -- a shared Map has no `set`
  rates.set("globex", { rate: 3 });
  // @ts-expect-error -- a shared Set has no `add`
  tags.add("b");
  // @ts-expect-error -- a shared Map's values are read-only
  rates.get("acme")!.rate = 4;
  void flag; void rate; void tagged;
}

async function processes(): Promise<void> {
  const result: RunResult = await run("ls", ["-l"], { input: "x", signal: new AbortController().signal });
  const code: number | null = result.exitCode;
  const out: Uint8Array = result.stdout;
  void code; void out;
}

function parallel(): void {
  const doubled: number[] = [1, 2, 3].parallelMap((x) => x * 2);
  const evens: number[] = doubled.parallelFilter((x) => x % 2 === 0);
  const words: string[] = ["a b", "c"].parallelFlatMap((s) => s.split(" "));
  const sum: number = [1, 2, 3].parallelReduce((acc, x) => acc + x, (a, b) => a + b, 0);
  const sorted: number[] = [3, 1, 2].parallelSort((a, b) => a - b);
  const floats: Float64Array = new Float64Array([1, 2]).parallelMap((x) => x / 2);
  const total: number = new Int32Array([1, 2]).parallelReduce((acc, x) => acc + x, (a, b) => a + b, 0);
  void evens; void words; void sum; void sorted; void floats; void total;
}

void actors; void shared; void processes; void parallel;
