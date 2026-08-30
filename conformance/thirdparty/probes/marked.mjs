import { marked } from "marked";
console.log(marked.parse("# Title\n\nSome *emphasis* and a [link](http://x).\n\n- a\n- b\n").trim().split("\n").join("|"));
console.log(marked.parse("```js\nconst a = 1;\n```").trim());
