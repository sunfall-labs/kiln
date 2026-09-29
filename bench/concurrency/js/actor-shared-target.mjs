import { Actor } from "kiln:actors";
import { isShared } from "kiln:shared";
import { walkGraph } from "./common.mjs";

export class SharedGraphReader extends Actor {
  load(value) {
    this.dataset = value;
    return { sum: walkGraph(value), shared: isShared(value) };
  }

  query() {
    return walkGraph(this.dataset);
  }
}
