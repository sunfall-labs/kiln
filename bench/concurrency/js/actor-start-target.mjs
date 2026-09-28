import { Actor } from "kiln:actors";

export class Ping extends Actor {
  reply(value) { return value + 1; }
}
