// TypeScript `declare` fields are erased by Bun's direct TypeScript execution. They must not
// create own properties whose value is `undefined`.
class IssueLike {
  declare readonly input?: unknown;
  readonly tag = "Filter";
  declare static metadata: string;
  static live = true;
}

const issue = new IssueLike();
if (Object.hasOwn(issue, "input")) throw new Error("instance declare field was emitted");
if (!Object.hasOwn(issue, "tag")) throw new Error("runtime field was erased");
if (Object.hasOwn(IssueLike, "metadata")) throw new Error("static declare field was emitted");
if (!Object.hasOwn(IssueLike, "live")) throw new Error("runtime static field was erased");
console.log("declare fields erased");
