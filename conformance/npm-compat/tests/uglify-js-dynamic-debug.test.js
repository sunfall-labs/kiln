// Diagnostic reproduction of uglify-js/tools/node.js's generated library body.
// The markers identify which real source file is executing when dynamic Function fails.
const fs = require("fs");
const toolFile = require.resolve("uglify-js");
const packageRoot = toolFile.replace(/\/tools\/node\.js$/, "");
const files = [
  "utils.js",
  "ast.js",
  "transform.js",
  "parse.js",
  "scope.js",
  "compress.js",
  "output.js",
  "sourcemap.js",
  "mozilla-ast.js",
  "propmangle.js",
  "minify.js",
  "../tools/exports.js",
];
const chunks = files.map((file, index) => {
  const path = `${packageRoot}/lib/${file}`;
  const source = fs.readFileSync(path, "utf8");
  return `${source}\nconsole.log("uglify debug completed ${index}: ${file}");`;
});

function describe_ast() {
  var out = OutputStream({ beautify: true });
  doitem(AST_Node);
  return out.get() + "\n";

  function doitem(ctor) {
    out.print("AST_" + ctor.TYPE);
    var props = ctor.SELF_PROPS.filter(function(prop) {
      return !/^\$/.test(prop);
    });
    if (props.length > 0) {
      out.space();
      out.with_parens(function() {
        props.forEach(function(prop, i) {
          if (i) out.space();
          out.print(prop);
        });
      });
    }
    if (ctor.documentation) {
      out.space();
      out.print_string(ctor.documentation);
    }
    if (ctor.SUBCLASSES.length > 0) {
      out.space();
      out.with_block(function() {
        ctor.SUBCLASSES.sort(function(a, b) {
          return a.TYPE < b.TYPE ? -1 : 1;
        }).forEach(function(ctor) {
          out.indent();
          doitem(ctor);
          out.newline();
        });
      });
    }
  }
}

chunks.push(`exports.describe_ast = ${describe_ast.toString()};\nconsole.log("uglify debug completed describe_ast");`);
const generated = new Function("domprops", "exports", chunks.join("\n\n"));
const domprops = JSON.parse(fs.readFileSync(`${packageRoot}/tools/domprops.json`, "utf8"));
generated(domprops, {});
console.log("uglify debug dynamic body completed");
