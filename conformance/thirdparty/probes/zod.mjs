import { z } from "zod";
const User = z.object({
  name: z.string().min(2),
  age: z.number().int().positive(),
  tags: z.array(z.string()).optional(),
});
console.log(JSON.stringify(User.parse({ name: "kiln", age: 3, tags: ["a"] })));
const bad = User.safeParse({ name: "k", age: -1 });
console.log(bad.success, bad.error.issues.length);
console.log(bad.error.issues.map((i) => i.code).sort().join(","));
const Union = z.union([z.literal("a"), z.number()]);
console.log(Union.parse("a"), Union.parse(7));
