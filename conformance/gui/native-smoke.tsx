/** @jsxImportSource kiln:gui */
// A native end-to-end check of kiln:gui against the real window system (scripts/gui-smoke.sh).
// It drives the app through the host the way AppKit would, waits for each effect with a
// deadline, prints what it observed, and exits non-zero on any mismatch.
import { createSignal, currentHost, eventLoopMode, render, Window } from "kiln:gui";

const host = currentHost();
const failures: string[] = [];
const expect = (what: string, ok: boolean) => {
  console.log(`${ok ? "ok" : "FAIL"} ${what}`);
  if (!ok) failures.push(what);
};

/** Resolves true as soon as `condition` holds, false after `ms`. */
function until(condition: () => boolean, ms = 3000): Promise<boolean> {
  const deadline = Date.now() + ms;
  return new Promise((resolve) => {
    const check = () => {
      if (condition()) resolve(true);
      else if (Date.now() > deadline) resolve(false);
      else setTimeout(check, 10);
    };
    check();
  });
}

const [count, setCount] = createSignal(0);
const [ticks, setTicks] = createSignal(0);
const [checked, setChecked] = createSignal(false);
let resized = "";
let closed = false;
let button: any;
let checkbox: any;
let label: any;
let row: any;
let rowPresses = 0;
let input: any;

const win = new Window({
  title: "kiln:gui smoke",
  width: 360,
  height: 260,
  onResize: (w: number, h: number) => (resized = `${w}x${h}`),
  onClose: () => (closed = true),
});
render(
  () => [
    <text ref={(n: any) => (label = n)}>{() => `count ${count()} ticks ${ticks()}`}</text>,
    <button ref={(n: any) => (button = n)} onPress={() => setCount((c) => c + 1)}>
      Increment
    </button>,
    <checkbox ref={(n: any) => (checkbox = n)} checked={checked} onChange={setChecked}>
      Checked
    </checkbox>,
    <hstack ref={(n: any) => (row = n)} onPress={() => rowPresses++} padding={8} background="fill" accessibilityLabel="Open row">
      <text>A pressable row</text>
    </hstack>,
    <input ref={(n: any) => (input = n)} placeholder="Focused on show" autofocus />,
  ],
  win,
);

async function main() {
  expect(`native host (${host.name})`, host.interactive);
  win.show();
  // A binary built by kiln links the bridge in, so the realm parks in the run loop; the
  // development dylib (KILN_GUI_LIB) cannot, and polls.
  const expected = process.env.KILN_GUI_LIB ? "polling" : "parked";
  expect(`event loop is ${expected}`, eventLoopMode() === expected);
  const timer = setInterval(() => setTicks((t) => t + 1), 20);

  // Real clicks through AppKit: action -> event queue -> drained by the event loop.
  host.call(button.handle, "click");
  host.call(button.handle, "click");
  host.call(checkbox.handle, "click");
  expect("clicks reached the handler", await until(() => count() === 2));
  expect("checkbox reported its state", await until(() => checked()));
  expect("label followed", label.text.startsWith("count 2"));
  expect("pressable container is wired natively", row.handle > 0 && row.props.pressable === true);
  const rowA11y = row.describe();
  expect(`pressable row is an accessible button (${JSON.stringify(rowA11y)})`, rowA11y.role === "button" && rowA11y.label === "Open row" && rowA11y.element);
  expect("autofocus took keyboard focus", await until(() => input.describe().focused));

  const start = ticks();
  expect("timers run alongside the UI loop", await until(() => ticks() >= start + 10, 2000));

  // While parked, an idle timer ends the wait at its deadline rather than a slice later.
  if (expected === "parked") {
    clearInterval(timer);
    await new Promise((resolve) => setTimeout(resolve, 600)); // let the loop go idle
    const lates: number[] = [];
    for (let i = 0; i < 5; i++) {
      const start = Date.now();
      await new Promise((resolve) => setTimeout(resolve, 200));
      lates.push(Date.now() - start - 200);
    }
    const worst = Math.max(...lates);
    expect(`idle timers fire on time while parked (worst ${worst} ms late)`, worst < 15);
  }
  win.set("width", 400);
  expect("resize event", await until(() => resized === "400x260"));

  clearInterval(timer);
  host.call(win.handle, "close");
  expect("window system close reached onClose", await until(() => closed && !win.visible));

  if (failures.length > 0) process.exit(1);
  console.log("smoke: all checks passed");
}

main();
