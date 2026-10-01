// Example: CSS Grid & Flexbox Layout in Kiln Native GUI (ADR 0074 + Taffy)
//
// Demonstrates:
// - CSS Grid with '1fr 1fr 1fr' columns and configurable gap
// - CSS Flexbox with flexDirection: "row", justifyContent: "space-between", alignItems: "center"
// - Real-time reactive updates through Solid 2.0 signals
//
// Run native on macOS:
//   kiln run --native --node examples/gui-grid-demo.ts

import {
  Window,
  VStack,
  Card,
  Box,
  Grid,
  Button,
  Text,
  createSignal,
  createEffect,
  runLoop,
  getActiveDriver,
} from 'kiln:gui';

console.log('⚡ Initializing Kiln Taffy Grid & Flexbox Demo...');
console.log(`Active Platform Driver: ${getActiveDriver().name}`);

const win = new Window({
  title: '⚡ Kiln Taffy Grid & Flexbox Layout',
  width: 680,
  height: 520,
});

const [count, setCount] = createSignal(0);

const root = new VStack({ spacing: 16, padding: 24 });

// Header
const title = new Text({
  text: 'Native CSS Grid & Flexbox (Powered by Taffy)',
  bold: true,
  size: 18,
});
root.appendChild(title);

const subtitle = new Text({
  text: 'Kiln GUI Native Layout Engine (ADR 0074 + Taffy 0.14)',
  size: 13,
});
root.appendChild(subtitle);

// 3-Column Responsive CSS Grid
const grid = new Grid({
  columns: '1fr 1fr 1fr',
  gap: 16,
  style: {
    width: '100%',
    height: 120,
  },
});

const card1 = new Card();
const text1 = new Text({ text: '📦 Taffy 0.14\nWeb-standard Flexbox\n& CSS Grid' });
card1.appendChild(text1);
grid.appendChild(card1);

const card2 = new Card();
const text2 = new Text({ text: '⚡ Thread 0 (ADR 0074)\nDeterministic Layout\nAppKit Integration' });
card2.appendChild(text2);
grid.appendChild(card2);

const card3 = new Card();
const text3 = new Text({ text: '🔄 Reactive Signals\nSolid 2.0 Model\nClicks: 0' });
card3.appendChild(text3);
grid.appendChild(card3);

root.appendChild(grid);

// Flexbox Footer
const footerBox = new Box({
  style: {
    flexDirection: 'row',
    justifyContent: 'space-between',
    alignItems: 'center',
    width: '100%',
    height: 48,
  },
});

const statusText = new Text({ text: 'Engine: Pure Rust Taffy Layout Pipeline' });
footerBox.appendChild(statusText);

const btn = new Button({
  label: 'Increment Signal',
  onClick: () => {
    setCount(c => c + 1);
  },
});
footerBox.appendChild(btn);

root.appendChild(footerBox);

// Reactivity
createEffect(() => {
  const current = count();
  text3.text = `🔄 Reactive Signals\nSolid 2.0 Model\nClicks: ${current}`;
});

win.content = root;
win.show();

console.log('⚡ Window displayed with Taffy Grid & Flexbox!');

if (process.env.KILN_SNAPSHOT) {
  setTimeout(() => {
    win.snapshot(process.env.KILN_SNAPSHOT!);
    console.log(`Snapshot saved to: ${process.env.KILN_SNAPSHOT}`);
    process.exit(0);
  }, 400);
}

runLoop(win).then(() => {
  console.log('Window closed cleanly. Exiting.');
  process.exit(0);
});
