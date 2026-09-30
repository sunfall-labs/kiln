import { Window, VStack, HStack, Button, Text, TextInput, Slider, Toggle, createRenderer } from 'kiln:gui';

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

// 3. SolidJS 2.0 Universal Renderer Bridge
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

console.log('gui conformance passed');
