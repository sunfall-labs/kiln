// expect: stream 2,4,6
import { Effect, Stream } from "effect";
Effect.runPromise(Effect.map(Stream.runCollect(Stream.map(Stream.fromIterable([1, 2, 3]), (n) => n * 2)), (c) => Array.from(c).join(","))).then((v) => console.log("stream", v));
