// Virtual native GUI built-in module for Kiln (ADR 0074).
// Exposes cross-platform native UI widget primitives, an in-memory virtual tree
// for headless testability, and a fine-grained universal renderer seam for SolidJS 2.0.

const G = globalThis as any;
let nextNodeId = 1;

export class Node {
  readonly id: number;
  readonly tag: string;
  parent: Node | null = null;
  children: Node[] = [];
  properties: Record<string, any> = {};
  private listeners: Record<string, Function[]> = {};

  constructor(tag: string) {
    this.id = nextNodeId++;
    this.tag = tag;
  }

  get firstChild(): Node | null {
    return this.children.length > 0 ? this.children[0] : null;
  }

  get nextSibling(): Node | null {
    if (!this.parent) return null;
    const siblings = this.parent.children;
    const index = siblings.indexOf(this);
    if (index >= 0 && index + 1 < siblings.length) {
      return siblings[index + 1];
    }
    return null;
  }

  appendChild(child: Node): void {
    this.insertBefore(child, null);
  }

  removeChild(child: Node): void {
    const index = this.children.indexOf(child);
    if (index >= 0) {
      this.children.splice(index, 1);
      child.parent = null;
      if (typeof G.__kilnGuiRemoveNode === "function") {
        G.__kilnGuiRemoveNode(this.id, child.id);
      }
    }
  }

  insertBefore(child: Node, anchor: Node | null): void {
    if (child.parent) {
      child.parent.removeChild(child);
    }
    child.parent = this;
    if (anchor) {
      const index = this.children.indexOf(anchor);
      if (index >= 0) {
        this.children.splice(index, 0, child);
      } else {
        this.children.push(child);
      }
    } else {
      this.children.push(child);
    }
    if (typeof G.__kilnGuiInsertNode === "function") {
      G.__kilnGuiInsertNode(this.id, child.id, anchor ? anchor.id : 0);
    }
  }

  setProperty(name: string, value: any): void {
    this.properties[name] = value;
    if (name.startsWith("on") && typeof value === "function") {
      let eventName = name.slice(2).toLowerCase();
      if (eventName === "press") eventName = "click";
      this.addEventListener(eventName, value);
    }
    if (typeof G.__kilnGuiSetProperty === "function") {
      G.__kilnGuiSetProperty(this.id, name, value);
    }
  }

  getProperty(name: string): any {
    return this.properties[name];
  }

  addEventListener(event: string, handler: Function): void {
    if (!this.listeners[event]) {
      this.listeners[event] = [];
    }
    this.listeners[event].push(handler);
    if (typeof G.__kilnGuiAddEventListener === "function") {
      G.__kilnGuiAddEventListener(this.id, event);
    }
  }

  removeEventListener(event: string, handler: Function): void {
    const list = this.listeners[event];
    if (list) {
      const index = list.indexOf(handler);
      if (index >= 0) {
        list.splice(index, 1);
      }
    }
  }

  dispatchEvent(event: string, ...args: any[]): void {
    const list = this.listeners[event];
    if (list) {
      for (let i = 0; i < list.length; i++) {
        list[i](...args);
      }
    }
  }
}

export class Window extends Node {
  constructor(options?: { title?: string; width?: number; height?: number }) {
    super("window");
    if (options) {
      if (options.title !== undefined) this.title = options.title;
      if (options.width !== undefined) this.width = options.width;
      if (options.height !== undefined) this.height = options.height;
    }
  }

  get title(): string {
    return this.properties.title || "";
  }
  set title(val: string) {
    this.setProperty("title", val);
  }

  get width(): number {
    return this.properties.width || 800;
  }
  set width(val: number) {
    this.setProperty("width", val);
  }

  get height(): number {
    return this.properties.height || 600;
  }
  set height(val: number) {
    this.setProperty("height", val);
  }

  get visible(): boolean {
    return !!this.properties.visible;
  }
  set visible(val: boolean) {
    this.setProperty("visible", val);
  }

  get content(): Node | null {
    return this.firstChild;
  }
  set content(node: Node | null) {
    while (this.children.length > 0) {
      this.removeChild(this.children[0]);
    }
    if (node) {
      this.appendChild(node);
    }
  }

  show(): void {
    this.visible = true;
    if (typeof G.__kilnGuiShowWindow === "function") {
      G.__kilnGuiShowWindow(this.id);
    }
  }

  close(): void {
    this.visible = false;
    if (typeof G.__kilnGuiCloseWindow === "function") {
      G.__kilnGuiCloseWindow(this.id);
    }
  }
}

export class VStack extends Node {
  constructor(options?: { spacing?: number; alignment?: string; children?: Node[] }) {
    super("vstack");
    if (options) {
      if (options.spacing !== undefined) this.setProperty("spacing", options.spacing);
      if (options.alignment !== undefined) this.setProperty("alignment", options.alignment);
      if (options.children) {
        for (let i = 0; i < options.children.length; i++) {
          this.appendChild(options.children[i]);
        }
      }
    }
  }

  get spacing(): number {
    return this.properties.spacing || 0;
  }
  set spacing(val: number) {
    this.setProperty("spacing", val);
  }

  get alignment(): string {
    return this.properties.alignment || "center";
  }
  set alignment(val: string) {
    this.setProperty("alignment", val);
  }
}

export class HStack extends Node {
  constructor(options?: { spacing?: number; alignment?: string; children?: Node[] }) {
    super("hstack");
    if (options) {
      if (options.spacing !== undefined) this.setProperty("spacing", options.spacing);
      if (options.alignment !== undefined) this.setProperty("alignment", options.alignment);
      if (options.children) {
        for (let i = 0; i < options.children.length; i++) {
          this.appendChild(options.children[i]);
        }
      }
    }
  }

  get spacing(): number {
    return this.properties.spacing || 0;
  }
  set spacing(val: number) {
    this.setProperty("spacing", val);
  }

  get alignment(): string {
    return this.properties.alignment || "center";
  }
  set alignment(val: string) {
    this.setProperty("alignment", val);
  }
}

export class Text extends Node {
  constructor(textOrOptions?: string | { text?: string }) {
    super("text");
    if (typeof textOrOptions === "string") {
      this.text = textOrOptions;
    } else if (textOrOptions && textOrOptions.text !== undefined) {
      this.text = textOrOptions.text;
    }
  }

  get text(): string {
    return this.properties.text || "";
  }
  set text(val: string) {
    this.setProperty("text", val);
  }
}

export class Button extends Node {
  constructor(labelOrOptions?: string | { label?: string; onPress?: () => void }) {
    super("button");
    if (typeof labelOrOptions === "string") {
      this.label = labelOrOptions;
    } else if (labelOrOptions) {
      if (labelOrOptions.label !== undefined) this.label = labelOrOptions.label;
      if (labelOrOptions.onPress) this.onPress = labelOrOptions.onPress;
    }
  }

  get label(): string {
    return this.properties.label || "";
  }
  set label(val: string) {
    this.setProperty("label", val);
  }

  get onPress(): (() => void) | null {
    return this.properties.onPress || null;
  }
  set onPress(handler: (() => void) | null) {
    this.setProperty("onPress", handler);
  }

  click(): void {
    this.dispatchEvent("click");
  }
}

export class TextInput extends Node {
  constructor(options?: { placeholder?: string; value?: string; onChange?: (val: string) => void }) {
    super("textinput");
    if (options) {
      if (options.placeholder !== undefined) this.setProperty("placeholder", options.placeholder);
      if (options.value !== undefined) this.value = options.value;
      if (options.onChange) this.setProperty("onChange", options.onChange);
    }
  }

  get value(): string {
    return this.properties.value || "";
  }
  set value(val: string) {
    this.setProperty("value", val);
  }

  get placeholder(): string {
    return this.properties.placeholder || "";
  }
  set placeholder(val: string) {
    this.setProperty("placeholder", val);
  }
}

export class Slider extends Node {
  constructor(options?: { min?: number; max?: number; value?: number; onChange?: (val: number) => void }) {
    super("slider");
    if (options) {
      if (options.min !== undefined) this.setProperty("min", options.min);
      if (options.max !== undefined) this.setProperty("max", options.max);
      if (options.value !== undefined) this.value = options.value;
      if (options.onChange) this.setProperty("onChange", options.onChange);
    }
  }

  get value(): number {
    return this.properties.value !== undefined ? this.properties.value : 0;
  }
  set value(val: number) {
    this.setProperty("value", val);
  }

  get min(): number {
    return this.properties.min !== undefined ? this.properties.min : 0;
  }
  set min(val: number) {
    this.setProperty("min", val);
  }

  get max(): number {
    return this.properties.max !== undefined ? this.properties.max : 100;
  }
  set max(val: number) {
    this.setProperty("max", val);
  }
}

export class Toggle extends Node {
  constructor(options?: { checked?: boolean; onToggle?: (checked: boolean) => void }) {
    super("toggle");
    if (options) {
      if (options.checked !== undefined) this.checked = options.checked;
      if (options.onToggle) this.setProperty("onToggle", options.onToggle);
    }
  }

  get checked(): boolean {
    return !!this.properties.checked;
  }
  set checked(val: boolean) {
    this.setProperty("checked", val);
  }
}

// Universal Renderer implementation for SolidJS 2.0 (@solidjs/universal interface)
export function createRenderer<T extends Node = Node>(config?: any): {
  render: (code: () => T, container: T) => () => void;
  createElement: (tag: string) => T;
  createTextNode: (text: string) => T;
  isTextNode: (node: T) => boolean;
  replaceText: (node: T, value: string) => void;
  insertNode: (parent: T, node: T, anchor?: T | null) => void;
  removeNode: (parent: T, node: T) => void;
  setProperty: (node: T, name: string, value: any) => void;
  getParentNode: (node: T) => T | null;
  getFirstChild: (node: T) => T | null;
  getNextSibling: (node: T) => T | null;
} {
  const widgetConstructors: Record<string, new (...args: any[]) => Node> = {
    window: Window,
    vstack: VStack,
    hstack: HStack,
    button: Button,
    text: Text,
    textinput: TextInput,
    slider: Slider,
    toggle: Toggle,
  };

  const renderer = {
    createElement: (tag: string): T => {
      const lower = tag.toLowerCase();
      const Ctor = widgetConstructors[lower] || Node;
      return new Ctor(lower) as T;
    },
    createTextNode: (value: string): T => {
      return new Text(value) as unknown as T;
    },
    isTextNode: (node: T): boolean => {
      return node instanceof Text;
    },
    replaceText: (node: T, value: string): void => {
      if (node instanceof Text) {
        node.text = value;
      } else {
        node.setProperty("text", value);
      }
    },
    insertNode: (parent: T, node: T, anchor?: T | null): void => {
      parent.insertBefore(node, anchor || null);
    },
    removeNode: (parent: T, node: T): void => {
      parent.removeChild(node);
    },
    setProperty: (node: T, name: string, value: any): void => {
      node.setProperty(name, value);
    },
    getParentNode: (node: T): T | null => {
      return node.parent as T | null;
    },
    getFirstChild: (node: T): T | null => {
      return node.firstChild as T | null;
    },
    getNextSibling: (node: T): T | null => {
      return node.nextSibling as T | null;
    },
    render: (code: () => T, container: T): (() => void) => {
      const root = code();
      container.appendChild(root);
      return () => {
        container.removeChild(root);
      };
    },
  };

  return renderer;
}

export default {
  Node,
  Window,
  VStack,
  HStack,
  Text,
  Button,
  TextInput,
  Slider,
  Toggle,
  createRenderer,
};
