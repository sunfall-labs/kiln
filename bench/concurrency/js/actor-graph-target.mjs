import { Actor } from "kiln:actors";

export class GraphReader extends Actor {
  echo(value) { return value; }
  sum(items) {
    let sum = 0;
    for (const item of items) {
      sum += item.id + item.name.length + item.tags[1].length +
             Math.trunc(item.price * 4) + item.cat.code;
    }
    return sum;
  }
}
