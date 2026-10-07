import { Actor } from "kiln:actors";

// An actor whose realm can end three ways while a caller still waits on it.
export class Mortal extends Actor {
  slow(ms) { return new Promise((resolve) => setTimeout(() => resolve("slow done"), ms)); }
  crashLater() { setTimeout(() => { throw new Error("uncaught in actor"); }, 5); return "scheduled"; }
  exit() { process.exit(3); }
}
