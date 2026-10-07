const validate = new Function(
  "data, { instancePath = '', dynamicAnchors = {} } = {}",
  "if (!dynamicAnchors.node) dynamicAnchors.node = 'root'; return [data, instancePath, dynamicAnchors.node];"
);

const fromDefaults = validate({ value: 1 });
const fromOptions = validate({ value: 2 }, { instancePath: "/next", dynamicAnchors: { node: "shared" } });

console.log(JSON.stringify({ fromDefaults, fromOptions }));
