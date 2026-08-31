// Ajv: a JSON Schema validator that compiles each schema into JavaScript and evaluates it.
//
// The runtime-code-generation input. Ajv builds a source string with its own code-gen DSL and
// hands it to `new Function`, so a working `Function` constructor with a correct scope chain is
// load-bearing rather than incidental -- this is the shape fastify, express validators and every
// OpenAPI stack take in production. It also exercises deep property descriptors, `RegExp` with
// `u` flags, `Object.getOwnPropertyNames` over schema objects, and structured error objects.
import Ajv from "ajv";

const lines = [];
const say = (...a) => lines.push(a.join(" "));

const ajv = new Ajv({ allErrors: true, strict: false, code: { source: false } });

const schema = {
  type: "object",
  required: ["id", "name"],
  additionalProperties: false,
  properties: {
    id: { type: "integer", minimum: 1 },
    name: { type: "string", minLength: 2, maxLength: 20, pattern: "^[A-Za-z ]+$" },

    tags: { type: "array", items: { type: "string" }, uniqueItems: true, maxItems: 3 },
    role: { enum: ["admin", "user", "guest"] },
    meta: {
      type: "object",
      patternProperties: { "^x-": { type: "string" } },
      additionalProperties: false,
    },
    score: { type: "number", multipleOf: 0.5, exclusiveMaximum: 100 },
    child: { $ref: "#" },
  },
  anyOf: [{ required: ["id"] }, { required: ["name"] }],
};

const validate = ajv.compile(schema);
say("compiled", typeof validate, typeof validate.errors);

const good = { id: 1, name: "Ada Lovelace", tags: ["a", "b"], role: "admin", score: 99.5, meta: { "x-k": "v" }, child: { id: 2, name: "Grace" } };
say("valid", validate(good), validate.errors === null);

const bad = { id: 0, name: "A", tags: ["a", "a", "b", "c"], role: "root", score: 100, meta: { bad: 1 }, extra: true };
say("invalid", validate(bad));
const kws = (validate.errors ?? []).map((e) => e.keyword).sort();
say("error-keywords", kws.join(","));
say("error-count", (validate.errors ?? []).length);
const first = (validate.errors ?? [])[0];
say("first-error", first ? first.instancePath + "|" + first.keyword + "|" + first.message : "none");

// A second, independently compiled schema: each compile is its own `new Function` closure and
// they must not share state.
const listSchema = { type: "array", items: { type: "integer" }, minItems: 2 };
const validateList = ajv.compile(listSchema);
say("list", validateList([1, 2, 3]), validateList([1]), validateList(["a", "b"]));
say("isolation", validate(good), validateList([1, 2]));

// Named schemas and $ref across documents: the resolver plus a second code-gen pass.
ajv.addSchema({ $id: "point", type: "object", properties: { x: { type: "number" }, y: { type: "number" } }, required: ["x", "y"] });
const validateLine = ajv.compile({ type: "object", properties: { a: { $ref: "point" }, b: { $ref: "point" } }, required: ["a", "b"] });
say("ref", validateLine({ a: { x: 1, y: 2 }, b: { x: 3, y: 4 } }), validateLine({ a: { x: 1 }, b: { x: 3, y: 4 } }));

// A custom keyword whose validator is a JavaScript function, called from generated code.
ajv.addKeyword({
  keyword: "even",
  type: "number",
  validate: function evenCheck(sch, data) { return sch === false || data % 2 === 0; },
  errors: false,
});
const validateEven = ajv.compile({ type: "number", even: true });
say("keyword", validateEven(4), validateEven(3));

// errorsText walks the error array and formats it, which is how a server turns this into a 400.
validate(bad);
const text = ajv.errorsText(validate.errors, { separator: " ; " });
say("errorsText-parts", text.split(" ; ").length, text.includes("must be"));

// Function construction itself, since that is what all of the above rests on.
say("Function-ctor", new Function("a", "b", "return a + b")(2, 3), typeof Function.prototype.bind);

console.log(lines.join("\n"));
