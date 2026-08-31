// React + react-dom/server: render a component tree to an HTML string.
//
// The single most common thing a real TypeScript application does on a server. React's element
// model is `Symbol.for("react.transitional.element")`-branded frozen objects; the server renderer
// is a hand-written work loop over a task queue with its own scheduler, `Array.isArray` checks on
// every child, `Object.assign` prop merging, context read through a stack of providers, hooks
// stored in a module-level dispatcher that is swapped per render, and HTML escaping done with a
// character-code scan. Nothing else in this corpus has this shape.
import { createElement as h, Fragment, createContext, useMemo, useState, useContext, useReducer, useId, memo } from "react";
import { renderToString, renderToStaticMarkup } from "react-dom/server";

const lines = [];
const say = (...a) => lines.push(a.join(" "));

const ThemeContext = createContext("light");

function Badge({ label, count }) {
  const cls = useMemo(() => "badge badge--" + label.toLowerCase(), [label]);
  return h("span", { className: cls, "data-count": count }, label, ": ", count);
}

const MemoBadge = memo(Badge);

function Row({ item, index }) {
  const theme = useContext(ThemeContext);
  return h(
    "li",
    { key: item.id, className: index % 2 ? "odd" : "even", "data-theme": theme },
    h("strong", null, item.name),
    " ",
    h(MemoBadge, { label: item.kind, count: item.tags.length }),
    item.tags.length ? h("ul", null, item.tags.map((t) => h("li", { key: t }, t))) : null,
  );
}

function counterReducer(state, action) {
  switch (action.type) {
    case "inc": return { n: state.n + 1 };
    case "add": return { n: state.n + action.by };
    default: return state;
  }
}

function App({ items }) {
  const [title] = useState("Inventory");
  const [state, dispatch] = useReducer(counterReducer, { n: 0 });
  const id = useId();
  return h(
    ThemeContext.Provider,
    { value: "dark" },
    h(
      Fragment,
      null,
      h("h1", { id }, title, " <", items.length, ">"),
      h("p", null, "escaped: ", "<script>alert(\"x\") & 'y'</script>"),
      h("ul", { className: "rows" }, items.map((item, i) => h(Row, { key: item.id, item, index: i }))),
      h("footer", { style: { color: "red", backgroundColor: "blue", marginTop: 4 } }, "n=", state.n, " ", typeof dispatch),
    ),
  );
}

const items = [
  { id: "a", name: "Anvil", kind: "Tool", tags: ["heavy", "iron"] },
  { id: "b", name: "Rope", kind: "Kit", tags: [] },
  { id: "c", name: "Lamp & Oil", kind: "Light", tags: ["fragile"] },
];

const html = renderToString(h(App, { items }));
say("length", html.length);
say("h1", /<h1 id="[^"]+">/.test(html), html.includes("Inventory"), html.includes("&lt;"), html.includes("&gt;"));
say("escaped-script", html.includes("&lt;script&gt;") && !html.includes("<script>"));
say("escaped-amp", html.includes("Lamp &amp; Oil"), "escaped-quote", html.includes("&quot;") || html.includes("&#x27;"));
say("context", (html.match(/data-theme="dark"/g) || []).length);
say("classes", (html.match(/class="badge badge--[a-z]+"/g) || []).join(","));
say("even-odd", (html.match(/class="(even|odd)"/g) || []).join(","));
say("style", /style="color:red;background-color:blue;margin-top:4px"/.test(html));
say("nested-list", (html.match(/<li>heavy<\/li>/g) || []).length, (html.match(/<ul>/g) || []).length);
say("footer", /n=<!-- -->0<!-- --> <!-- -->function/.test(html) || html.includes("n="));

const staticHtml = renderToStaticMarkup(h("div", { className: "x" }, h("em", null, "a & b")));
say("static", staticHtml);
say("no-comments", !staticHtml.includes("<!--"));

// Elements are frozen, branded objects; a real app relies on both.
const el = h("div", { a: 1 }, "kid");
say("element", typeof el.type, JSON.stringify(el.props), String(el.$$typeof));
say("frozen", Object.isFrozen(el), Object.isFrozen(el.props));

console.log(lines.join("\n"));
