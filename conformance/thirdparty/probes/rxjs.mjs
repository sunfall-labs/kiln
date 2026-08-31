// RxJS: observables, schedulers and Promise interop.
//
// The promise-heavy input. RxJS drives its own microtask scheduler, converts to and from
// promises in both directions (`firstValueFrom`, `from(promise)`, `lastValueFrom`), consumes
// async iterables through `Symbol.asyncIterator`, and its `Subscriber` hierarchy is deep ES
// classes with `Symbol.observable` interop. If real code touches the corners Test262's
// built-ins/Promise cluster covers, this is where it shows.
import {
  of, from, interval, firstValueFrom, lastValueFrom, forkJoin, merge, concat,
  combineLatest, defer, throwError, EMPTY, Subject, BehaviorSubject, ReplaySubject,
} from "rxjs";
import {
  map, filter, take, mergeMap, concatMap, switchMap, scan, toArray, catchError,
  retry, delay, tap, startWith, distinctUntilChanged, reduce,
} from "rxjs/operators";

const lines = [];
const say = (...a) => lines.push(a.join(" "));

async function* asyncSource() {
  for (let i = 1; i <= 3; i++) {
    await Promise.resolve();
    yield i;
  }
}

async function main() {
  // Synchronous pipeline.
  say("sync", await firstValueFrom(of(1, 2, 3).pipe(map((x) => x * 2), toArray())));

  // from(Promise): the adapter that calls `.then` on a thenable.
  say("from-promise", await firstValueFrom(from(Promise.resolve("p"))));

  // from(asyncIterable): Symbol.asyncIterator driven by the scheduler.
  say("from-async-iter", (await firstValueFrom(from(asyncSource()).pipe(toArray()))).join(","));

  // mergeMap over promises: concurrency plus promise coercion inside an operator.
  const merged = await lastValueFrom(
    from([1, 2, 3]).pipe(mergeMap((x) => Promise.resolve(x * 10)), toArray()),
  );
  say("mergeMap-promise", merged.sort((a, b) => a - b).join(","));

  // concatMap preserves order where mergeMap does not.
  say("concatMap", (await lastValueFrom(from([3, 2, 1]).pipe(concatMap((x) => of(x).pipe(delay(x))), toArray()))).join(","));

  // Errors: a rejected promise must surface through catchError, not as an unhandled rejection.
  say("catchError", await firstValueFrom(
    from(Promise.reject(new Error("boom"))).pipe(catchError((e) => of("caught:" + e.message))),
  ));

  // retry re-subscribes, which re-runs the deferred factory.
  let attempts = 0;
  say("retry", await firstValueFrom(
    defer(() => { attempts++; return attempts < 3 ? throwError(() => new Error("again")) : of("ok@" + attempts); }).pipe(retry(5)),
  ));

  // Timer-driven: interval uses the async scheduler, i.e. setInterval.
  say("interval", (await lastValueFrom(interval(1).pipe(take(4), toArray()))).join(","));

  // Combinators over mixed sources.
  say("forkJoin", JSON.stringify(await lastValueFrom(forkJoin({ a: of(1), b: Promise.resolve(2), c: from([3, 4]) }))));
  say("combineLatest", JSON.stringify(await lastValueFrom(combineLatest([of(1, 2), of("x")]).pipe(toArray()))));
  say("merge", (await lastValueFrom(merge(of(1), of(2)).pipe(toArray()))).join(","));
  say("concat", (await lastValueFrom(concat(of(1), from(Promise.resolve(2))).pipe(toArray()))).join(","));

  // Subjects: multicast with replay semantics.
  const subj = new Subject();
  const seen = [];
  const sub = subj.pipe(scan((a, b) => a + b, 0)).subscribe((v) => seen.push(v));
  subj.next(1); subj.next(2); subj.next(3); subj.complete();
  sub.unsubscribe();
  say("subject", seen.join(","));

  const bs = new BehaviorSubject(0);
  say("behavior", (await lastValueFrom(bs.pipe(startWith(-1), take(2), toArray()))).join(","));

  const rs = new ReplaySubject(2);
  rs.next("a"); rs.next("b"); rs.next("c"); rs.complete();
  say("replay", (await lastValueFrom(rs.pipe(toArray()))).join(","));

  // EMPTY completes with no value: lastValueFrom must reject with EmptyError.
  try {
    await lastValueFrom(EMPTY);
    say("empty", "no-throw");
  } catch (e) {
    say("empty", e.name);
  }

  // The observable is itself async-iterable in modern rxjs; if not, say so deterministically.
  const src = of(1, 2, 3);
  if (typeof src[Symbol.asyncIterator] === "function") {
    const collected = [];
    for await (const v of src) collected.push(v);
    say("for-await-observable", collected.join(","));
  } else {
    say("for-await-observable", "unsupported");
  }

  // Symbol.observable interop, the protocol that makes rxjs and other libraries interchangeable.
  const key = Symbol.for("rxjs.internal.observable") ?? null;
  say("interop", typeof src.subscribe, typeof src.pipe, String(key));

  // distinctUntilChanged + reduce: a plain fold over a hot-ish source.
  say("fold", await lastValueFrom(from([1, 1, 2, 2, 3]).pipe(distinctUntilChanged(), reduce((a, b) => a + b, 0))));

  // tap + filter, to make sure side effects run in subscription order.
  const order = [];
  await lastValueFrom(from([1, 2, 3, 4]).pipe(tap((x) => order.push("t" + x)), filter((x) => x % 2 === 0), tap((x) => order.push("f" + x)), toArray()));
  say("order", order.join(""));

  console.log(lines.join("\n"));
}

main().catch((e) => { console.log("FATAL", e && e.message); });
