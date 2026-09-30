// Example: Live Native GUI Application in Kiln
// Combines SolidJS 2.0 fine-grained reactivity, universal renderer, and native GTK4 windowing
// with direct event pump loop and zero Virtual DOM overhead.
//
// Run native:
//   kiln run --native --node examples/gui-window-demo.ts

import {
  Window,
  VStack,
  HStack,
  Button,
  Text,
  createSignal,
  createEffect,
  runLoop,
  systemTheme,
} from 'kiln:gui';

console.log('⚡ Initializing Kiln Native GUI Window...');

const win = new Window({
  title: 'Kiln GUI • SolidJS 2.0 + GTK4 Driver',
  width: 500,
  height: 320,
});

// Fine-grained reactive signal state
const [count, setCount] = createSignal(0);

// UI Widgets
const titleLabel = new Text('⚡ Kiln Native GUI Subsystem');
const descLabel = new Text('SolidJS 2.0 fine-grained reactivity driving GTK4 native widgets with zero VDOM.');
const countLabel = new Text('Current Count: 0');
const themeLabel = new Text(`System Theme: ${systemTheme.getTheme()}`);

// Direct fine-grained subscription: only updates countLabel.text when count() changes
createEffect(() => {
  countLabel.text = `Current Count: ${count()}`;
  console.log(`[Signal Effect] ${countLabel.text}`);
});

const decrementBtn = new Button({
  label: '  − 1  ',
  onPress: () => {
    setCount((c) => c - 1);
  },
});

const resetBtn = new Button({
  label: '  Reset  ',
  onPress: () => {
    setCount(0);
  },
});

const incrementBtn = new Button({
  label: '  + 1  ',
  onPress: () => {
    setCount((c) => c + 1);
  },
});

const closeBtn = new Button({
  label: '  Close Window  ',
  onPress: () => {
    console.log('Close button pressed, closing window...');
    win.close();
  },
});

const buttonRow = new HStack({
  spacing: 12,
  children: [decrementBtn, resetBtn, incrementBtn],
});

win.content = new VStack({
  spacing: 16,
  children: [titleLabel, descLabel, countLabel, themeLabel, buttonRow, closeBtn],
});

console.log('Displaying native window on desktop...');
win.show();
console.log('Window presented. Running native event pump loop...');

runLoop(win).then(() => {
  console.log('Window closed cleanly. Exiting.');
});
