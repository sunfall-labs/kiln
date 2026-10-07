// kiln:gui accessibility on the headless host: labels and hints reach the host, pressable
// containers become buttons, unlabelled images stay silent, and autofocus takes focus on show.
import { Button, HStack, Image, Text, TextInput, useHeadless, Window } from "kiln:gui";

useHeadless();

const icon = new Button({ icon: "trash", accessibilityLabel: "Delete", accessibilityHint: "Removes the item" });
const decorative = new Image({ symbol: "star" });
const labelled = new Image({ symbol: "star", accessibilityLabel: "Favourite" });
const row = new HStack({ onPress: () => {} }, [new Text("Open settings")]);
const plain = new HStack({}, [new Text("Not interactive")]);
const first = new TextInput({ placeholder: "first" });
const second = new TextInput({ placeholder: "second", autofocus: true });
const win = new Window({ title: "a11y" }, [icon, decorative, labelled, row, plain, first, second]);
win.show();

const show = (name: string, d: { role: string; label: string; hint: string; element: boolean; focused: boolean }) =>
  console.log(`${name}: role=${d.role} label=${JSON.stringify(d.label)} hint=${JSON.stringify(d.hint)} element=${d.element} focused=${d.focused}`);
show("icon button", icon.describe());
show("decorative image", decorative.describe());
show("labelled image", labelled.describe());
show("pressable row", row.describe());
show("plain row", plain.describe());
show("first input", first.describe());
show("autofocus input", second.describe());

// Labels follow signals like any prop.
icon.set("accessibilityLabel", () => "Delete item");
show("relabelled", icon.describe());

try {
  new Button("detached").describe();
} catch (error) {
  console.log("detached:", String(error));
}
