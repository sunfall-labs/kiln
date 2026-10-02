// kiln:gui widgets on the headless host: what the host is asked to build, how user input flows
// back, controlled props, reactive regions, control flow, disposal and the OS services.
import {
  app,
  Button,
  clipboard,
  createRoot,
  createSignal,
  dialog,
  fire,
  For,
  h,
  HStack,
  inspect,
  jsx,
  notify,
  processEvents,
  render,
  runApp,
  Show,
  Slider,
  Switch,
  Match,
  Text,
  TextInput,
  Toggle,
  useHeadless,
  VStack,
  Window,
} from "kiln:gui";

const host = useHeadless();

// Imperative construction; nothing exists in the host until a window is shown.
const [count, setCount] = createSignal(0);
const label = new Text(() => `Count: ${count()}`);
const plus = new Button("+1", () => setCount((c) => c + 1));
const win = new Window({ title: "Counter", width: 320, height: 200 }, [label, new HStack({ gap: 4 }, [plus])]);
console.log("before show:", host.widgets.size, win.handle);
win.show();
console.log(host.inspect(win.handle));

// A press goes through the same path as a native event, and the label follows synchronously.
plus.press();
plus.press();
console.log("after presses:", label.text, host.widgets.get(label.handle)!.props.text);

// Disabled buttons ignore presses.
plus.set("disabled", true);
plus.press();
console.log("disabled:", label.text, host.widgets.get(plus.handle)!.props.disabled);
plus.set("disabled", false);

// Layout is decided here and sent as one object.
console.log("layout:", JSON.stringify(host.widgets.get(plus.parent!.handle)!.props.layout));
console.log("button in row:", JSON.stringify(plus.layout()), "in column:", JSON.stringify(new Button("x").layout()));

// Controlled input: the bound value wins over what was typed unless the handler follows it.
const [name, setName] = createSignal("kiln");
const upper = new TextInput({ value: name, onChange: (v: string) => setName(v.toUpperCase()) });
const ignored = new TextInput({ value: () => "fixed", onChange: () => {} });
const free = new TextInput({ value: "start" });
win.append([upper, ignored, free]);
fire(upper, "input", "abc");
fire(ignored, "input", "typed");
fire(free, "input", "typed");
const shown = (n: { handle: number }) => host.widgets.get(n.handle)!.props.value;
console.log("controlled:", upper.value, shown(upper), "|", ignored.value, shown(ignored), "|", free.value, shown(free));

// A slider with a step reports and shows snapped values; a toggle reports booleans.
const steps: number[] = [];
const slider = new Slider({ min: 0, max: 10, step: 2.5, onChange: (v: number) => steps.push(v) });
const flips: boolean[] = [];
const toggle = new Toggle({ onChange: (on: boolean) => flips.push(on) });
win.append([slider, toggle]);
fire(slider, "change", 3.4);
fire(slider, "change", 3.6);
fire(slider, "change", 8.9);
toggle.toggle();
toggle.toggle();
console.log("slider:", steps.join(","), shown(slider), "toggle:", flips.join(","));

// Event-prop reassignment replaces the handler instead of stacking it.
let first = 0;
let second = 0;
plus.set("onPress", () => first++);
plus.set("onPress", () => second++);
plus.press();
console.log("handler replaced:", first, second);

// Removing keeps a node (it can come back); disposing destroys its whole subtree.
const group = new VStack({}, [new Text("a"), new Text("b"), new Text("c"), new Text("d")]);
win.append(group);
const before = host.widgets.size;
win.removeChild(group);
console.log("removed:", host.widgets.size === before, group.handle > 0, group.parent === null);
win.append(group);
group.dispose();
console.log("disposed subtree:", before - host.widgets.size, group.children.length, group.disposed);

// Reactive regions re-render only their own part of the children, in place.
const [items, setItems] = createSignal(["x", "y"]);
const [showFooter, setShowFooter] = createSignal(false);
const list = new VStack();
let rendered = 0;
render(
  () => [
    h(Text, { text: "header" }),
    h(For, {
      each: items,
      children: (item: string) => {
        rendered++;
        return jsx(Text, { text: item });
      },
    }),
    h(Show, { when: showFooter, fallback: "no footer", children: jsx("text", { children: "footer" }) }),
  ],
  list,
);
win.append(list);
const texts = () => list.children.filter((c) => c.materializable).map((c) => (c as Text).text).join(" ");
console.log("region:", texts());
const kept = list.children[2];
setItems(["y", "x", "z"]);
console.log("reordered:", texts(), "renders:", rendered, "kept:", list.children.indexOf(kept) >= 0);
setShowFooter(true);
console.log("show:", texts());
console.log("host order:", host.widgets.get(list.handle)!.children.map((hd) => host.widgets.get(hd)!.props.text).join(" "));

// Switch picks the first matching case.
const [mode, setMode] = createSignal("b");
const box = new VStack();
render(
  () =>
    h(Switch, {
      fallback: "none",
      children: [
        jsx(Match, { when: () => mode() === "a", children: "is a" }),
        jsx(Match, { when: () => mode() === "b", children: "is b" }),
      ],
    }),
  box,
);
const shownText = () => box.children.filter((c) => c.materializable).map((c) => (c as Text).text).join(",");
const switched = [shownText()];
setMode("a");
switched.push(shownText());
setMode("c");
switched.push(shownText());
console.log("switch:", switched.join(" -> "));

// Nodes created under an owner die with it.
const holder = new VStack();
win.append(holder);
let dispose = () => {};
createRoot((d) => {
  dispose = d;
  holder.append([new Text("owned 1"), new Text("owned 2")]);
});
const owned = holder.children.length;
dispose();
console.log("owner disposal:", owned, holder.children.length);

// Leaves refuse children instead of dropping them.
try {
  new Slider().append(new Text("nope"));
} catch (error) {
  console.log("leaf children:", String(error));
}

// Text children concatenate, and keep following accessors.
const greeting = h("text", null, "Hello, ", () => name(), "!") as Text;
setName("world");
console.log("text children:", greeting.text);

// Services on the headless host are explicit stand-ins.
host.dialogAnswers.open = "/tmp/picked.txt";
dialog.openFile().then((path) => console.log("dialog:", path));
dialog.saveFile().then((path) => console.log("cancelled dialog:", path));
clipboard.writeText("copied").then(() => clipboard.readText()).then((t) => console.log("clipboard:", t));
notify("Done", "All good");
console.log("notifications:", JSON.stringify(host.notifications));
let menuClicks = 0;
app.setMenu([{ label: "File", items: [{ id: "new", label: "New", accelerator: "CmdOrCtrl+N", onClick: () => menuClicks++ }, "separator"] }]);
host.post({ handle: 0, kind: 7, a: 0, b: 0, text: "new" });
console.log("menu:", JSON.stringify(host.menu), "events:", processEvents(), "clicks:", menuClicks);

// A native close event closes the window; runApp resolves once the last one is gone.
const done = runApp(() => jsx("window", { title: "Second", children: jsx("text", { children: "second window" }) }));
done.then(() => console.log("runApp resolved; open:", win.visible));
console.log("inspect:", inspect(win).split("\n").slice(0, 3).join(" / "));
// The window system closing a window arrives as an event.
host.post({ handle: win.handle, kind: 5, a: 0, b: 0, text: "" });
processEvents();
console.log("closed by host:", win.visible);
setTimeout(() => app.quit(), 0);
