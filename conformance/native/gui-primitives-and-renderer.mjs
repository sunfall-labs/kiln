import {
  Window,
  VStack,
  HStack,
  Button,
  Text,
  TextInput,
  Slider,
  Toggle,
  TrayIcon,
  Menu,
  MenuItem,
  MenuSeparator,
  Submenu,
  createRenderer,
  dialog,
  clipboard,
  systemTheme,
  notification,
  app,
  globalShortcut,
  turnMailbox,
  nodeRegistry,
  createSignal,
  createEffect,
  createMemo,
  batch,
  untrack,
  onCleanup,
} from 'kiln:gui';

// 1. Native Widget Primitives
const win = new Window({ title: 'Counter App', width: 400, height: 300 });
console.log('window:', win.title, win.width, win.height, win.visible);

let count = 0;
const label = new Text(`Count: ${count}`);
const button = new Button({
  label: 'Increment',
  onPress: () => {
    count++;
    label.text = `Count: ${count}`;
  },
});

const layout = new VStack({ spacing: 10, children: [label, button] });
win.content = layout;

console.log('initial:', label.text, button.label, layout.children.length);

button.click();
console.log('click 1:', label.text);

button.click();
console.log('click 2:', label.text);

win.show();
console.log('window visible:', win.visible);
win.maximize();
win.unmaximize();
win.minimize();
win.setFullscreen(false);
win.setResizable(true);
console.log('win fullscreen:', win.fullscreen, 'resizable:', win.resizable);
win.close();
console.log('window closed:', win.visible);

// 2. Additional Controls
const input = new TextInput({ placeholder: 'Name', value: 'Kiln' });
console.log('input:', input.placeholder, input.value);
input.value = 'Solid Native';
console.log('input modified:', input.value);

const slider = new Slider({ min: 10, max: 200, value: 50 });
console.log('slider:', slider.min, slider.max, slider.value);

const toggle = new Toggle({ checked: true });
console.log('toggle:', toggle.checked);

// 3. Node Registry & Hierarchy
console.log('registry has win:', nodeRegistry.has(win.id));
console.log('registry has button:', nodeRegistry.has(button.id));

// 4. SolidJS 2.0 Universal Renderer Bridge
const renderer = createRenderer();
const solidWin = renderer.createElement('window');
renderer.setProperty(solidWin, 'title', 'Solid Window');

const solidVStack = renderer.createElement('vstack');
renderer.setProperty(solidVStack, 'spacing', 16);

const solidText = renderer.createTextNode('Solid Text');
renderer.insertNode(solidVStack, solidText);

const solidButton = renderer.createElement('button');
renderer.setProperty(solidButton, 'label', 'Solid Click');
let solidClicked = false;
renderer.setProperty(solidButton, 'onPress', () => {
  solidClicked = true;
});
renderer.insertNode(solidVStack, solidButton);
renderer.insertNode(solidWin, solidVStack);

console.log('solid hierarchy:', solidWin.tag, solidWin.firstChild?.tag, solidVStack.children.length);
solidButton.dispatchEvent('click');
console.log('solid button clicked:', solidClicked);
renderer.replaceText(solidText, 'Updated Solid Text');
console.log('solid text updated:', solidText.text);

// 5. Fine-grained Reactivity (SolidJS 2.0 Signals, Effects, Memos)
const [signalCount, setSignalCount] = createSignal(10);
const doubleCount = createMemo(() => signalCount() * 2);

let effectLog = [];
let cleanupCount = 0;

createEffect(() => {
  const current = doubleCount();
  effectLog.push(`double is ${current}`);
  onCleanup(() => {
    cleanupCount++;
  });
});

console.log('memo initial:', doubleCount());
console.log('effect log 1:', effectLog.join(', '));

setSignalCount(20);
console.log('memo after update:', doubleCount());
console.log('effect log 2:', effectLog.join(', '));
console.log('cleanup count:', cleanupCount);

// Test batching
batch(() => {
  setSignalCount(25);
  setSignalCount(30);
});
console.log('memo after batch:', doubleCount());
console.log('effect log 3:', effectLog.join(', '));

// Test untrack
const untrackedVal = untrack(() => signalCount());
console.log('untracked value:', untrackedVal);

// 6. Reactive Widget Property Binding (Signal -> Effect -> Native Widget Property)
const reactiveLabel = new Text('');
createEffect(() => {
  reactiveLabel.text = `Reactive: ${signalCount()}`;
});
console.log('reactive widget initial:', reactiveLabel.text);

setSignalCount(100);
console.log('reactive widget updated:', reactiveLabel.text);

// 7. Turn Mailbox & Reentrancy Guard (ADR 0074 §4)
let turnDispatched = false;
solidButton.addEventListener('custom-turn', (arg) => {
  turnDispatched = true;
  console.log('turn mailbox dispatched event with arg:', arg);
});

turnMailbox.post(solidButton.id, 'custom-turn', 'payload-123');
console.log('turn dispatched status:', turnDispatched);

// 8. OS Utility Bridges
console.log('system theme:', systemTheme.getTheme());
systemTheme.setTheme('dark');
console.log('updated theme:', systemTheme.getTheme());
systemTheme.setTheme('light');

await clipboard.writeText('kiln-gui-clipboard');
const readBack = await clipboard.readText();
console.log('clipboard readback:', readBack);
await clipboard.clear();
const clearedBack = await clipboard.readText();
console.log('clipboard after clear:', clearedBack);

// 9. Native Menu & Tray Icon (ADR 0074)
const menu = new Menu();
const item1 = new MenuItem({ label: 'Open', onClick: () => console.log('menu open') });
const item2 = new MenuItem({ label: 'Quit', onClick: () => console.log('menu quit') });
menu.append(item1);
menu.appendSeparator();
const submenu = new Submenu('More Options');
const subItem = new MenuItem({ label: 'Preferences' });
submenu.append(subItem);
menu.appendSubmenu(submenu);
menu.append(item2);
console.log('menu items count:', menu.getItems().length);
console.log('item1 label:', item1.label, 'enabled:', item1.enabled);
console.log('submenu label:', submenu.label, 'items:', submenu.getItems().length);

const tray = new TrayIcon({ tooltip: 'Kiln App Tray', menu });
console.log('tray tooltip:', tray.tooltip);
tray.tooltip = 'Updated Kiln Tray';
console.log('tray updated tooltip:', tray.tooltip);
console.log('tray menu items:', tray.menu?.getItems().length);
tray.dispose();

// 10. Additional GUI OS APIs & App Environment
console.log('dialog pickFolder defined:', typeof dialog.pickFolder === 'function');
console.log('dialog openFiles defined:', typeof dialog.openFiles === 'function');
console.log('notification show defined:', typeof notification.show === 'function');
console.log('app name:', app.name, 'version:', app.version);
console.log('app getPath temp defined:', typeof app.getPath('temp') === 'string');
console.log('app single instance lock:', typeof app.requestSingleInstanceLock('test-app') === 'boolean');

// 11. Accelerators & Global Shortcuts
const saveItem = new MenuItem({ label: 'Save', accelerator: 'Ctrl+S' });
console.log('item accelerator:', saveItem.accelerator);

console.log('globalShortcut register defined:', typeof globalShortcut.register === 'function');
console.log('globalShortcut poll defined:', typeof globalShortcut.poll === 'function');

console.log('gui conformance passed');
