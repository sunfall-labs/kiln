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
  Box,
  Grid,
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
  createMemo,
  isPending,
  flush,
  runLoop,
  systemTheme,
  getActiveDriver,
} from 'kiln:gui';

console.log('⚡ Initializing Kiln Developer Studio on macOS...');
console.log(`Active Platform Driver: ${getActiveDriver().name}`);

const win = new Window({
  title: '⚡ Kiln Developer Studio • macOS AppKit',
  width: 660,
  height: 790,
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

const headerTopRow = new Box({
  style: {
    flexDirection: 'row',
    justifyContent: 'space-between',
    alignItems: 'center',
    width: '100%',
    height: 42,
  },
  children: [titleLabel, tabSwitcher],
});

const subtitleLabel = new Text({
  text: 'SolidJS 2.0 Reactive Signals • Native macOS Cocoa • Taffy 0.14 Layout',
  variant: 'subheadline',
  color: 'secondary',
});

// ── Responsive 3-Column Metrics (Taffy 0.14 CSS Grid) ─────────
const heapMetricCard = new Card({
  children: [
    new Text({ text: 'ALLOCATED HEAP', variant: 'caption', color: 'secondary' }),
    new Text({ text: () => `${workers() * 64} MB`, variant: 'headline', color: 'blue' }),
  ],
});

const archMetricCard = new Card({
  children: [
    new Text({ text: 'TARGET ARCHITECTURE', variant: 'caption', color: 'secondary' }),
    new Text({ text: () => targetArch().split('-')[0], variant: 'headline' }),
  ],
});

const engineMetricCard = new Card({
  children: [
    new Text({ text: 'LAYOUT ENGINE', variant: 'caption', color: 'secondary' }),
    new Text({ text: 'Taffy 0.14 Grid', variant: 'headline', color: 'green' }),
  ],
});

const metricsGrid = new Grid({
  columns: '1fr 1fr 1fr',
  gap: 12,
  style: {
    width: '100%',
    height: 82,
  },
  children: [heapMetricCard, archMetricCard, engineMetricCard],
});

// ── Card 1: Worker & Memory Allocation ───────────────────────
const computeHeader = new Text({
  text: '⚙️  Compute Engine & Concurrency',
  variant: 'headline',
});

const computeMetrics = new Text({
  text: () => `${workers()} Workers • ${workers() * 64} MB Heap`,
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
  value: workers,
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
  label: '  8 Default  ',
  onPress: () => setWorkers(8),
});

const incBtn = new Button({
  label: '  + 1  ',
  onPress: () => setWorkers((w: number) => Math.min(32, w + 1)),
});

const sliderRow = new HStack({
  spacing: 10,
  children: [slider, new Spacer(), decBtn, defaultBtn, incBtn],
});

const utilLabel = new Text({
  text: 'Allocation Utilization:',
  variant: 'subheadline',
  color: 'secondary',
});

const workerProgressBar = new ProgressBar({
  value: () => (workers() / 32) * 100,
  max: 100,
  width: 200,
});

const utilValue = new Text({
  text: () => `${Math.round((workers() / 32) * 100)}%`,
  variant: 'subheadline',
  color: 'secondary',
});

const utilRow = new HStack({
  spacing: 12,
  children: [utilLabel, new Spacer(), workerProgressBar, utilValue],
});

const computeCard = new Card({
  width: 588,
  children: [computeTitleRow, sliderRow, utilRow],
});

// ── Card 2: Native Target Architecture & Pipeline ───────────
const archHeader = new Text({
  text: '🎯  Target Triple & Build Pipeline',
  variant: 'headline',
});

const archSelect = new Select({
  options: ['aarch64-apple-darwin', 'x86_64-apple-darwin', 'wasm32-wasi'],
  selectedIndex: 0,
  width: 190,
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
  text: () => `Target: ${targetArch()}`,
  variant: 'subheadline',
  color: 'secondary',
});

const compileBar = new ProgressBar({
  value: compileProgress,
  max: 100,
  width: 190,
});

const compilePctLabel = new Text({
  text: () => `${compileProgress()}%`,
  variant: 'subheadline',
  color: 'secondary',
});

const buildRow = new HStack({
  spacing: 12,
  children: [progressLabel, new Spacer(), compileBar, compilePctLabel],
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
  text: () => `Session: @${handle()} • Region: us-west-2 • Latency: 0.14ms • Tab: ${activeTab()}`,
  variant: 'subheadline',
  color: 'secondary',
});

const profileCard = new Card({
  width: 588,
  children: [inputRow, sessionReadout],
});

// ── Card 4: Runtime Subsystems & Features ────────────────────
const metalSwitch = new Toggle({
  checked: metalAccel,
  onToggle: (checked: boolean) => setMetalAccel(checked),
});

const metalLabel = new Text({
  text: 'Metal CoreAnimation Hardware Blending',
  variant: 'body',
});

const metalPill = new Text({
  text: () => (metalAccel() ? '🟢 ACCELERATED' : '⚪ CPU RENDER'),
  variant: 'badge',
  color: () => (metalAccel() ? 'green' : 'secondary'),
});

const metalRow = new HStack({
  spacing: 12,
  children: [metalSwitch, metalLabel, new Spacer(), metalPill],
});

const telemetrySwitch = new Toggle({
  checked: telemetry,
  onToggle: (checked: boolean) => setTelemetry(checked),
});

const telemetryLabel = new Text({
  text: 'Realtime Profiler & Telemetry Stream',
  variant: 'body',
});

const telemetryPill = new Text({
  text: () => (telemetry() ? '🟢 STREAMING' : '⚪ PAUSED'),
  variant: 'badge',
  color: () => (telemetry() ? 'green' : 'secondary'),
});

const telemetryRow = new HStack({
  spacing: 12,
  children: [telemetrySwitch, telemetryLabel, new Spacer(), telemetryPill],
});

const strictCheckbox = new Checkbox({
  label: 'Enforce strict ADR-0074 type conformance and memory invariants',
  checked: strictChecks,
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
    setTargetArch('aarch64-apple-darwin');
    archSelect.selectedIndex = 0;
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

const footerButtonBox = new Box({
  style: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 10,
  },
  children: [simulateBtn, resetAllBtn, closeBtn],
});

const footerActionRow = new Box({
  style: {
    flexDirection: 'row',
    justifyContent: 'space-between',
    alignItems: 'center',
    width: '100%',
    height: 36,
  },
  children: [themeInfo, footerButtonBox],
});

// ── Assemble Root Content View ───────────────────────────────
win.content = new VStack({
  spacing: 12,
  children: [
    headerTopRow,
    subtitleLabel,
    metricsGrid,
    computeCard,
    targetCard,
    profileCard,
    subsystemsCard,
    new Spacer(),
    footerActionRow,
  ],
});

console.log('Presenting vibrant Apple-style macOS studio window...');
win.show();
if (process.env.KILN_SNAPSHOT) {
  setTimeout(() => {
    win.snapshot(process.env.KILN_SNAPSHOT!);
    console.log(`Studio snapshot saved to: ${process.env.KILN_SNAPSHOT}`);
    process.exit(0);
  }, 400);
}
console.log('Window active! Event loop listening...');

runLoop(win).then(() => {
  console.log('Window closed cleanly. Exiting.');
  process.exit(0);
});
