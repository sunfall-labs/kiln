// Example: Modern Native macOS GUI Application in Kiln (ADR 0074)
// Features:
//  - SolidJS 2.0 fine-grained reactivity (zero VDOM overhead)
//  - Apple-style card hierarchy with subtle frosted translucency
//  - Generous margin gutters (>= 36pt) and top-bar breathing room
//  - Translucent vibrancy (NSVisualEffectView), Metal CoreAnimation layers
//  - Live widgets: Sliders, Buttons, TextInputs, Toggles, Cards, Dividers
//  - Zero-CPU idle wait in event runloop
//
// Run native on macOS:
//   kiln run --native --node examples/gui-mac-demo.ts

import {
  Window,
  VStack,
  HStack,
  Card,
  Divider,
  Button,
  Text,
  TextInput,
  Slider,
  Toggle,
  createSignal,
  createEffect,
  runLoop,
  systemTheme,
  getActiveDriver,
} from 'kiln:gui';

console.log('⚡ Initializing Kiln Developer Studio on macOS...');
console.log(`Active Platform Driver: ${getActiveDriver().name}`);

const win = new Window({
  title: '⚡ Kiln Developer Studio • macOS AppKit',
  width: 620,
  height: 560,
});

// Fine-grained reactive state
const [workers, setWorkers] = createSignal(8);
const [handle, setHandle] = createSignal('developer');
const [metalAccel, setMetalAccel] = createSignal(true);
const [telemetry, setTelemetry] = createSignal(true);

// ── Header Section ───────────────────────────────────────────
const titleLabel = new Text({
  text: '⚡ Kiln Developer Studio',
  variant: 'title',
});

const subtitleLabel = new Text({
  text: 'SolidJS 2.0 Reactive Signals • Native macOS Cocoa • Metal CA Layers',
  variant: 'subheadline',
  color: 'secondary',
});

// ── Card 1: Worker & Memory Allocation ───────────────────────
const computeHeader = new Text({
  text: '⚙️  Compute Engine & Concurrency',
  variant: 'headline',
});

const computeMetrics = new Text({
  text: `${workers()} Workers • ${workers() * 64} MB Heap`,
  variant: 'headline',
  color: 'blue',
});

const computeTitleRow = new HStack({
  spacing: 12,
  children: [computeHeader, computeMetrics],
});

const slider = new Slider({
  min: 1,
  max: 32,
  value: workers(),
  width: 260,
  onChange: (val: number) => {
    setWorkers(Math.round(val));
  },
});

const decBtn = new Button({
  label: '  − 1  ',
  onPress: () => setWorkers((w: number) => Math.max(1, w - 1)),
});

const defaultBtn = new Button({
  label: '  8 (Default)  ',
  onPress: () => setWorkers(8),
});

const incBtn = new Button({
  label: '  + 1  ',
  onPress: () => setWorkers((w: number) => Math.min(32, w + 1)),
});

const sliderControlRow = new HStack({
  spacing: 12,
  children: [slider, decBtn, defaultBtn, incBtn],
});

createEffect(() => {
  computeMetrics.text = `${workers()} Workers • ${workers() * 64} MB Heap`;
  slider.value = workers();
});

const computeCard = new Card({
  width: 548,
  children: [computeTitleRow, sliderControlRow],
});

// ── Card 2: Developer Profile & Identity ─────────────────────
const profileHeader = new Text({
  text: '👤  Developer Identity & Dispatch',
  variant: 'headline',
});

const nameInput = new TextInput({
  value: handle(),
  placeholder: 'Enter developer handle...',
  width: 240,
  onChange: (val: string) => {
    setHandle(val.trim() || 'developer');
  },
});

const inputRow = new HStack({
  spacing: 14,
  children: [profileHeader, nameInput],
});

const sessionReadout = new Text({
  text: `Session: @${handle()} • us-west-2 • Latency: 0.14ms`,
  variant: 'subheadline',
  color: 'secondary',
});

createEffect(() => {
  sessionReadout.text = `Session: @${handle()} • Region: us-west-2 • Latency: 0.14ms`;
});

const profileCard = new Card({
  width: 548,
  children: [inputRow, sessionReadout],
});

// ── Card 3: Runtime Subsystems & Features ────────────────────
const metalSwitch = new Toggle({
  checked: metalAccel(),
  onToggle: (checked: boolean) => setMetalAccel(checked),
});

const metalLabel = new Text({
  text: 'Metal CoreAnimation Hardware Blending',
  variant: 'body',
});

const metalPill = new Text({
  text: metalAccel() ? '🟢 ACCELERATED' : '⚪ CPU RENDER',
  variant: 'badge',
  color: metalAccel() ? 'green' : 'secondary',
});

createEffect(() => {
  metalPill.text = metalAccel() ? '🟢 ACCELERATED' : '⚪ CPU RENDER';
  metalPill.setProperty('color', metalAccel() ? 'green' : 'secondary');
});

const metalRow = new HStack({
  spacing: 12,
  children: [metalSwitch, metalLabel, metalPill],
});

const telemetrySwitch = new Toggle({
  checked: telemetry(),
  onToggle: (checked: boolean) => setTelemetry(checked),
});

const telemetryLabel = new Text({
  text: 'Realtime Profiler & Telemetry Stream',
  variant: 'body',
});

const telemetryPill = new Text({
  text: telemetry() ? '🟢 STREAMING' : '⚪ PAUSED',
  variant: 'badge',
  color: telemetry() ? 'green' : 'secondary',
});

createEffect(() => {
  telemetryPill.text = telemetry() ? '🟢 STREAMING' : '⚪ PAUSED';
  telemetryPill.setProperty('color', telemetry() ? 'green' : 'secondary');
});

const telemetryRow = new HStack({
  spacing: 12,
  children: [telemetrySwitch, telemetryLabel, telemetryPill],
});

const subsystemsCard = new Card({
  width: 548,
  children: [metalRow, telemetryRow],
});

// ── Footer Section ───────────────────────────────────────────
const themeInfo = new Text({
  text: `macOS ${systemTheme.getTheme().toUpperCase()} • Zero-CPU Idle Wait`,
  variant: 'caption',
  color: 'tertiary',
});

const resetAllBtn = new Button({
  label: '  Reset All  ',
  onPress: () => {
    setWorkers(8);
    setHandle('developer');
    nameInput.value = 'developer';
    setMetalAccel(true);
    setTelemetry(true);
  },
});

const closeBtn = new Button({
  label: '  Close Studio  ',
  variant: 'primary',
  onPress: () => {
    console.log('Close requested. Exiting cleanly...');
    win.close();
  },
});

const footerActionRow = new HStack({
  spacing: 12,
  children: [themeInfo, resetAllBtn, closeBtn],
});

// ── Assemble Root Content View ───────────────────────────────
win.content = new VStack({
  spacing: 14,
  children: [
    titleLabel,
    subtitleLabel,
    computeCard,
    profileCard,
    subsystemsCard,
    footerActionRow,
  ],
});

console.log('Presenting vibrant Apple-style macOS studio window...');
win.show();
console.log('Window active! Event loop listening...');

runLoop(win).then(() => {
  console.log('Window closed cleanly. Exiting.');
  process.exit(0);
});
