// Example: Native GUI Counter Application in Kiln
// Demonstrates fine-grained reactivity (SolidJS 2.0 signals) driving native widgets
// with zero Virtual DOM overhead and full differential parity.
//
// Run interpreted:
//   kiln run examples/gui-counter.ts
// Compile & run native:
//   kiln run --native examples/gui-counter.ts

import {
  Window,
  VStack,
  HStack,
  Button,
  Text,
  createSignal,
  createEffect,
  systemTheme,
} from 'kiln:gui';

// 1. Fine-grained reactive state (SolidJS 2.0 signals)
const [count, setCount] = createSignal(0);

// 2. Native UI widgets
const win = new Window({ title: 'Kiln Native Counter', width: 360, height: 200 });
const label = new Text('Count: 0');

// 3. Fine-grained signal binding: updates widget property directly without VDOM or tree diffing!
createEffect(() => {
  label.text = `Count: ${count()}`;
});

const decrementBtn = new Button({
  label: '-1',
  onPress: () => setCount((c) => c - 1),
});

const incrementBtn = new Button({
  label: '+1',
  onPress: () => setCount((c) => c + 1),
});

const resetBtn = new Button({
  label: 'Reset',
  onPress: () => setCount(0),
});

const buttonRow = new HStack({
  spacing: 8,
  children: [decrementBtn, resetBtn, incrementBtn],
});

win.content = new VStack({
  spacing: 16,
  children: [label, buttonRow],
});

console.log(`Current theme: ${systemTheme.getTheme()}`);
console.log(`Window initialized: ${win.title} (${win.width}x${win.height})`);
console.log(`Initial count: ${label.text}`);

// Simulate user interactions
incrementBtn.click();
console.log(`After increment click: ${label.text}`);

incrementBtn.click();
console.log(`After second increment: ${label.text}`);

decrementBtn.click();
console.log(`After decrement click: ${label.text}`);

resetBtn.click();
console.log(`After reset click: ${label.text}`);
