import path from "node:path";
console.log(JSON.stringify([
  path.basename("/foo/bar.js", "bar.js"),
  path.basename("/foo/bar.js", ".js"),
  path.dirname("a"),
  path.extname(".hidden"),
  path.relative("/a/b", "/a/b"),
]));
