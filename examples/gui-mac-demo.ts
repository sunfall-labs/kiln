// Example: Modern Native macOS GUI Application in Kiln (ADR 0074)
// Features:
//  - SolidJS 2.0 fine-grained reactivity (zero VDOM overhead)
//  - Apple-style card hierarchy with subtle frosted translucency
//  - Generous margin gutters (>= 36pt) and top-bar breathing room
//  - Translucent vibrancy (NSVisualEffectView), Metal CoreAnimation layers
//  - Live widgets: SegmentedControl, Spacer, ScrollView, ProgressBar, Select,
//                  Checkbox, Slider, Button with icons, TextInput, Toggle
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
  Spacer,
  ScrollView,
  SegmentedControl,
  ProgressBar,
  Select,
  Checkbox,
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
  width: 660,
  height: 680,
});

// Fine-grained reactive state
const [workers, setWorkers] = createSignal(8);
const [handle, setHandle] = createSignal('developer');
const [metalAccel, setMetalAccel] = createSignal(true);
const [telemetry, setTelemetry] = createSignal(true);
const [strictChecks, setStrictChecks] = createSignal(true);
const [activeTab, setActiveTab] = createSignal('Dashboard');
const [targetArch, setTargetArch] = createSignal('aarch64-apple-darwin');
const [compileProgress, setCompileProgress] = createSignal(65);

// ── Header Section with Title, Spacer, and Segmented Picker ──
const titleLabel = new Text({
  text: '⚡ Kiln Developer Studio',
  variant: 'title',
});

const tabSwitcher = new SegmentedControl({
  items: ['Dashboard', 'Compiler', 'Settings'],
  selectedIndex: 0,
  onChange: (index: number, label: string) => {
    console.log(`Switched to tab: ${label} (index ${index})`);
    setActiveTab(label);
  },
});

const headerTopRow = new HStack({
  spacing: 12,
  children: [titleLabel, new Spacer(), tabSwitcher],
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
  children: [computeHeader, new Spacer(), computeMetrics],
});

const slider = new Slider({
  min: 1,
  max: 32,
  value: workers(),
  width: 240,
  onChange: (val: number) => {
    setWorkers(Math.round(val));
  },
});

const workerProgressBar = new ProgressBar({
  value: (workers() / 32) * 100,
  max: 100,
  width: 140,
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
  spacing: 10,
  children: [slider, workerProgressBar, decBtn, defaultBtn, incBtn],
});

createEffect(() => {
  computeMetrics.text = `${workers()} Workers • ${workers() * 64} MB Heap`;
  slider.value = workers();
  workerProgressBar.setValue((workers() / 32) * 100);
});

const computeCard = new Card({
  width: 588,
  children: [computeTitleRow, sliderControlRow],
});

// ── Card 2: Native Target Architecture & Pipeline ───────────
const archHeader = new Text({
  text: '🎯  Target Triple & Build Pipeline',
  variant: 'headline',
});

const archSelect = new Select({
  options: ['aarch64-apple-darwin', 'x86_64-apple-darwin', 'wasm32-wasi'],
  selectedIndex: 0,
  width: 200,
  onChange: (idx: number, val: string) => {
    console.log(`Target triple changed to: ${val}`);
    setTargetArch(val);
  },
});

const archTitleRow = new HStack({
  spacing: 12,
  children: [archHeader, new Spacer(), archSelect],
});

const progressLabel = new Text({
  text: `Compilation Status: ${targetArch()} (Optimized)`,
  variant: 'subheadline',
  color: 'secondary',
});

const compileBar = new ProgressBar({
  value: compileProgress(),
  max: 100,
  width: 220,
});

createEffect(() => {
  progressLabel.text = `Compilation Status: ${targetArch()} (Optimized)`;
  compileBar.setValue(compileProgress());
});

const buildRow = new HStack({
  spacing: 12,
  children: [progressLabel, new Spacer(), compileBar],
});

const targetCard = new Card({
  width: 588,
  children: [archTitleRow, buildRow],
});

// ── Card 3: Developer Profile & Identity ─────────────────────
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
  children: [profileHeader, new Spacer(), nameInput],
});

const sessionReadout = new Text({
  text: `Session: @${handle()} • us-west-2 • Latency: 0.14ms`,
  variant: 'subheadline',
  color: 'secondary',
});

createEffect(() => {
  sessionReadout.text = `Session: @${handle()} • Region: us-west-2 • Latency: 0.14ms • Tab: ${activeTab()}`;
});

const profileCard = new Card({
  width: 588,
  children: [inputRow, sessionReadout],
});

// ── Card 4: Runtime Subsystems & Features ────────────────────
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
  children: [metalSwitch, metalLabel, new Spacer(), metalPill],
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
  children: [telemetrySwitch, telemetryLabel, new Spacer(), telemetryPill],
});

const strictCheckbox = new Checkbox({
  label: 'Enforce strict ADR-0074 type conformance and memory invariants',
  checked: strictChecks(),
  onToggle: (checked: boolean) => {
    setStrictChecks(checked);
    console.log(`Strict checks toggled: ${checked}`);
  },
});

const subsystemsCard = new Card({
  width: 588,
  children: [metalRow, telemetryRow, new Divider({ width: 556 }), strictCheckbox],
});

// ── Footer Section with Theme info & Action Buttons ───────────
const themeInfo = new Text({
  text: `macOS ${systemTheme.getTheme().toUpperCase()} • Zero-CPU Idle Wait • AppKit`,
  variant: 'caption',
  color: 'tertiary',
});

const simulateBtn = new Button({
  icon: 'play',
  label: '  Run Compile  ',
  onPress: () => {
    console.log('Simulating compilation run...');
    let p = 0;
    const interval = setInterval(() => {
      p += 15;
      if (p >= 100) {
        p = 100;
        clearInterval(interval);
      }
      setCompileProgress(p);
    }, 150);
  },
});

const resetAllBtn = new Button({
  icon: 'refresh',
  label: '  Reset All  ',
  onPress: () => {
    setWorkers(8);
    setHandle('developer');
    nameInput.value = 'developer';
    setMetalAccel(true);
    setTelemetry(true);
    setStrictChecks(true);
    setCompileProgress(65);
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
  spacing: 10,
  children: [themeInfo, new Spacer(), simulateBtn, resetAllBtn, closeBtn],
});

// ── Assemble Root Content View ───────────────────────────────
win.content = new VStack({
  spacing: 12,
  children: [
    headerTopRow,
    subtitleLabel,
    computeCard,
    targetCard,
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
