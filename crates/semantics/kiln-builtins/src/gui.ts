// Virtual native GUI built-in module for Kiln (ADR 0074).
// Exposes cross-platform native UI widget primitives, an in-memory virtual tree
// for headless testability, and a fine-grained universal renderer seam for SolidJS 2.0.

const G = globalThis as any;
let nextNodeId = 1;
export const nodeRegistry = new Map<number, Node>();

// Platform OS Windowing Driver (GTK4 on Linux / AppKit on macOS)
let gtkInstance: any = null;
let gtkDriverInitialized = false;
const nativeHandles = new Map<number, bigint>();

function toCStr(ffi: any, str: string): bigint {
  const enc = new TextEncoder();
  const bytes = enc.encode(str + "\0");
  return ffi.getRawPointer(bytes.buffer);
}

function getGtkDriver() {
  if (gtkDriverInitialized) return gtkInstance;
  gtkDriverInitialized = true;
  try {
    const ffi = (typeof process !== "undefined" && (process as any).__kilnNativeModules?.ffi) || null;
    if (!ffi) return null;
    const hasDisplay = typeof process !== "undefined" && process.env && (process.env.WAYLAND_DISPLAY || process.env.DISPLAY);
    if (!hasDisplay) return null;

    const libName = process.platform === "linux" ? "libgtk-4.so.1" : null;
    if (!libName) return null;

    const gtk = ffi.dlopen(libName, {
      gtk_init_check: { arguments: [], return: "i32" },
      gtk_window_new: { arguments: [], return: "pointer" },
      gtk_window_set_title: { arguments: ["pointer", "pointer"], return: "void" },
      gtk_window_set_default_size: { arguments: ["pointer", "i32", "i32"], return: "void" },
      gtk_window_set_child: { arguments: ["pointer", "pointer"], return: "void" },
      gtk_window_present: { arguments: ["pointer"], return: "void" },
      gtk_window_close: { arguments: ["pointer"], return: "void" },
      gtk_box_new: { arguments: ["i32", "i32"], return: "pointer" },
      gtk_box_append: { arguments: ["pointer", "pointer"], return: "void" },
      gtk_box_remove: { arguments: ["pointer", "pointer"], return: "void" },
      gtk_button_new_with_label: { arguments: ["pointer"], return: "pointer" },
      gtk_button_set_label: { arguments: ["pointer", "pointer"], return: "void" },
      gtk_label_new: { arguments: ["pointer"], return: "pointer" },
      gtk_label_set_text: { arguments: ["pointer", "pointer"], return: "void" },
      gtk_entry_new: { arguments: [], return: "pointer" },
      gtk_editable_set_text: { arguments: ["pointer", "pointer"], return: "void" },
      gtk_switch_new: { arguments: [], return: "pointer" },
      gtk_switch_set_active: { arguments: ["pointer", "i32"], return: "void" },
      gtk_scale_new_with_range: { arguments: ["i32", "f64", "f64", "f64"], return: "pointer" },
      gtk_range_set_value: { arguments: ["pointer", "f64"], return: "void" },
      g_signal_connect_data: { arguments: ["pointer", "pointer", "pointer", "pointer", "pointer", "i32"], return: "u64" },
      g_main_context_iteration: { arguments: ["pointer", "i32"], return: "i32" },
    });

    if (gtk && gtk.functions && gtk.functions.gtk_init_check && gtk.functions.gtk_init_check() === 1) {
      gtkInstance = gtk;
      return gtkInstance;
    }
  } catch (_) {}
  return null;
}

function updateGtkProperty(handle: bigint, tag: string, name: string, value: any): void {
  if (!gtkInstance) return;
  const ffi = (process as any).__kilnNativeModules?.ffi;
  if (!ffi) return;
  try {
    if (tag === "button" && name === "label") {
      gtkInstance.functions.gtk_button_set_label(handle, toCStr(ffi, String(value)));
    } else if (tag === "text" && (name === "text" || name === "value")) {
      gtkInstance.functions.gtk_label_set_text(handle, toCStr(ffi, String(value)));
    } else if (tag === "textinput" && name === "value") {
      gtkInstance.functions.gtk_editable_set_text(handle, toCStr(ffi, String(value)));
    } else if (tag === "slider" && name === "value") {
      gtkInstance.functions.gtk_range_set_value(handle, Number(value));
    } else if (tag === "toggle" && name === "checked") {
      gtkInstance.functions.gtk_switch_set_active(handle, value ? 1 : 0);
    } else if (tag === "window" && name === "title") {
      gtkInstance.functions.gtk_window_set_title(handle, toCStr(ffi, String(value)));
    }
  } catch (_) {}
}

function realizeGtkNode(gtk: any, ffi: any, node: Node): bigint | null {
  let handle = nativeHandles.get(node.id);
  if (handle) return handle;

  try {
    if (node.tag === "vstack") {
      const spacing = node.getProperty("spacing") || 0;
      handle = gtk.functions.gtk_box_new(1 /* GTK_ORIENTATION_VERTICAL */, spacing);
    } else if (node.tag === "hstack") {
      const spacing = node.getProperty("spacing") || 0;
      handle = gtk.functions.gtk_box_new(0 /* GTK_ORIENTATION_HORIZONTAL */, spacing);
    } else if (node.tag === "button") {
      const label = node.getProperty("label") || "";
      handle = gtk.functions.gtk_button_new_with_label(toCStr(ffi, String(label)));
      const callbackPtr = gtk.lib.registerCallback(
        { arguments: ["pointer", "pointer"], return: "void" },
        () => {
          turnMailbox.post(node.id, "click");
        }
      );
      gtk.functions.g_signal_connect_data(handle, toCStr(ffi, "clicked"), callbackPtr, 0n, 0n, 0);
    } else if (node.tag === "text") {
      const text = node.getProperty("text") || "";
      handle = gtk.functions.gtk_label_new(toCStr(ffi, String(text)));
    } else if (node.tag === "textinput") {
      handle = gtk.functions.gtk_entry_new();
      const val = node.getProperty("value");
      if (val) gtk.functions.gtk_editable_set_text(handle, toCStr(ffi, String(val)));
    } else if (node.tag === "slider") {
      const min = node.getProperty("min") || 0;
      const max = node.getProperty("max") || 100;
      handle = gtk.functions.gtk_scale_new_with_range(0, min, max, 1.0);
      const val = node.getProperty("value");
      if (val !== undefined) gtk.functions.gtk_range_set_value(handle, Number(val));
    } else if (node.tag === "toggle") {
      handle = gtk.functions.gtk_switch_new();
      const chk = node.getProperty("checked");
      if (chk) gtk.functions.gtk_switch_set_active(handle, 1);
    } else {
      handle = gtk.functions.gtk_box_new(1, 0);
    }

    if (handle) {
      nativeHandles.set(node.id, handle);
      if (node.tag === "vstack" || node.tag === "hstack") {
        for (let i = 0; i < node.children.length; i++) {
          const childHandle = realizeGtkNode(gtk, ffi, node.children[i]);
          if (childHandle) {
            gtk.functions.gtk_box_append(handle, childHandle);
          }
        }
      }
    }
  } catch (_) {
    return null;
  }

  return handle || null;
}

function showGtkWindow(win: Window): void {
  const gtk = getGtkDriver();
  if (!gtk) return;
  try {
    const ffi = (process as any).__kilnNativeModules?.ffi;
    let winHandle = nativeHandles.get(win.id);
    if (!winHandle) {
      winHandle = gtk.functions.gtk_window_new();
      nativeHandles.set(win.id, winHandle);
      gtk.functions.gtk_window_set_title(winHandle, toCStr(ffi, win.title || "Kiln Window"));
      gtk.functions.gtk_window_set_default_size(winHandle, win.width || 800, win.height || 600);
      if (win.content) {
        const childHandle = realizeGtkNode(gtk, ffi, win.content);
        if (childHandle) {
          gtk.functions.gtk_window_set_child(winHandle, childHandle);
        }
      }
    }
    gtk.functions.gtk_window_present(winHandle);
    gtk.functions.g_main_context_iteration(0n, 0);
  } catch (_) {}
}

function closeGtkWindow(win: Window): void {
  const handle = nativeHandles.get(win.id);
  if (handle && gtkInstance) {
    try {
      gtkInstance.functions.gtk_window_close(handle);
    } catch (_) {}
  }
}

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
    nodeRegistry.set(this.id, this);
  }

  dispose(): void {
    nodeRegistry.delete(this.id);
    for (let i = 0; i < this.children.length; i++) {
      this.children[i].dispose();
    }
    if (this.parent) {
      this.parent.removeChild(this);
    }
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
    const handle = nativeHandles.get(this.id);
    if (handle) {
      updateGtkProperty(handle, this.tag, name, value);
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
    showGtkWindow(this);
    if (typeof G.__kilnGuiShowWindow === "function") {
      G.__kilnGuiShowWindow(this.id);
    }
  }

  close(): void {
    this.visible = false;
    closeGtkWindow(this);
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

const defaultRenderer = createRenderer();

export function createElement(tag: any, props?: any, ...children: any[]): Node {
  let node: Node;
  if (typeof tag === "function") {
    const isClass = tag.prototype && (tag.prototype instanceof Node || tag.prototype.constructor === Node);
    if (isClass) {
      node = new tag(props);
    } else {
      return tag({ ...(props || {}), children });
    }
  } else if (typeof tag === "string") {
    node = defaultRenderer.createElement(tag);
  } else {
    node = new Node("unknown");
  }

  if (props) {
    for (const key of Object.keys(props)) {
      if (key !== "children") {
        node.setProperty(key, props[key]);
      }
    }
  }

  const allChildren = children.length > 0 ? children : (props && props.children ? (Array.isArray(props.children) ? props.children : [props.children]) : []);
  for (let i = 0; i < allChildren.length; i++) {
    const child = allChildren[i];
    if (child !== null && child !== undefined && child !== false) {
      if (typeof child === "string" || typeof child === "number") {
        node.appendChild(new Text(String(child)));
      } else if (child instanceof Node) {
        node.appendChild(child);
      } else if (Array.isArray(child)) {
        for (let j = 0; j < child.length; j++) {
          const c = child[j];
          if (c instanceof Node) {
            node.appendChild(c);
          } else if (typeof c === "string" || typeof c === "number") {
            node.appendChild(new Text(String(c)));
          }
        }
      }
    }
  }

  return node;
}

export const h = createElement;
export const Fragment = (props: any) => props?.children;


// OS Utility Bridges (ADR 0074 §3)
export interface FileFilter {
  name: string;
  extensions: string[];
}

export interface OpenFileOptions {
  title?: string;
  filters?: FileFilter[];
}

export interface SaveFileOptions {
  title?: string;
  defaultPath?: string;
  filters?: FileFilter[];
}

export interface MessageOptions {
  title?: string;
  message: string;
  type?: "info" | "warning" | "error";
}

export const dialog = {
  async openFile(options?: OpenFileOptions): Promise<string | null> {
    if (typeof G.__kilnGuiOpenFile === "function") {
      return G.__kilnGuiOpenFile(options);
    }
    return null;
  },
  async saveFile(options?: SaveFileOptions): Promise<string | null> {
    if (typeof G.__kilnGuiSaveFile === "function") {
      return G.__kilnGuiSaveFile(options);
    }
    return null;
  },
  async message(options: MessageOptions): Promise<void> {
    if (typeof G.__kilnGuiShowMessage === "function") {
      return G.__kilnGuiShowMessage(options);
    }
  },
};

let virtualClipboard = "";
export const clipboard = {
  async readText(): Promise<string> {
    if (typeof G.__kilnGuiClipboardRead === "function") {
      return G.__kilnGuiClipboardRead();
    }
    return virtualClipboard;
  },
  async writeText(text: string): Promise<void> {
    if (typeof G.__kilnGuiClipboardWrite === "function") {
      return G.__kilnGuiClipboardWrite(text);
    }
    virtualClipboard = String(text);
  },
};

let currentTheme: "dark" | "light" = "light";
const themeListeners: ((theme: "dark" | "light") => void)[] = [];
export const systemTheme = {
  getTheme(): "dark" | "light" {
    if (typeof G.__kilnGuiGetTheme === "function") {
      return G.__kilnGuiGetTheme();
    }
    return currentTheme;
  },
  setTheme(theme: "dark" | "light"): void {
    currentTheme = theme;
    for (let i = 0; i < themeListeners.length; i++) {
      themeListeners[i](theme);
    }
  },
  onThemeChange(callback: (theme: "dark" | "light") => void): () => void {
    themeListeners.push(callback);
    return () => {
      const idx = themeListeners.indexOf(callback);
      if (idx >= 0) themeListeners.splice(idx, 1);
    };
  },
};

// Turn Mailbox & Reentrancy-Safe Runloop Integration (ADR 0074 §4)
interface MailboxTurn {
  targetId: number;
  event: string;
  args: any[];
}

const mailboxQueue: MailboxTurn[] = [];
let isExecutingTurn = false;

export const turnMailbox = {
  pumpEvents(): void {
    const gtk = getGtkDriver();
    if (gtk) {
      try {
        while (gtk.functions.g_main_context_iteration(0n, 0) !== 0) {}
      } catch (_) {}
    }
  },
  post(targetId: number, event: string, ...args: any[]): void {
    mailboxQueue.push({ targetId, event, args });
    if (!isExecutingTurn) {
      this.drain();
    }
  },
  drain(): void {
    if (isExecutingTurn) return; // Reentrancy Guard (ADR 0074 §4.3)
    isExecutingTurn = true;
    try {
      this.pumpEvents();
      while (mailboxQueue.length > 0) {
        const item = mailboxQueue.shift()!;
        const target = nodeRegistry.get(item.targetId);
        if (target) {
          target.dispatchEvent(item.event, ...item.args);
        }
      }
    } finally {
      isExecutingTurn = false;
    }
  },
  get pendingCount(): number {
    return mailboxQueue.length;
  },
};

// Fine-grained Reactive System (SolidJS 2.0 primitives)
interface Computation {
  fn: () => void | (() => void);
  cleanups: (() => void)[];
  sources: Set<Signal<any>>;
  stale: boolean;
}

let currentListener: Computation | null = null;
let batchDepth = 0;
const pendingComputations: Set<Computation> = new Set();

class Signal<T> {
  value: T;
  subscribers: Set<Computation> = new Set();

  constructor(value: T) {
    this.value = value;
  }

  read(): T {
    if (currentListener) {
      this.subscribers.add(currentListener);
      currentListener.sources.add(this);
    }
    return this.value;
  }

  write(next: T | ((prev: T) => T)): void {
    const val = typeof next === "function" ? (next as any)(this.value) : next;
    if (val !== this.value) {
      this.value = val;
      const subs = Array.from(this.subscribers);
      for (let i = 0; i < subs.length; i++) {
        const sub = subs[i];
        sub.stale = true;
        if (batchDepth > 0) {
          pendingComputations.add(sub);
        } else {
          runComputation(sub);
        }
      }
    }
  }
}

function runComputation(c: Computation): void {
  for (let i = 0; i < c.cleanups.length; i++) {
    c.cleanups[i]();
  }
  c.cleanups = [];

  const srcs = Array.from(c.sources);
  for (let i = 0; i < srcs.length; i++) {
    srcs[i].subscribers.delete(c);
  }
  c.sources.clear();
  c.stale = false;

  const prevListener = currentListener;
  currentListener = c;
  try {
    const cleanup = c.fn();
    if (typeof cleanup === "function") {
      c.cleanups.push(cleanup);
    }
  } finally {
    currentListener = prevListener;
  }
}

export function createSignal<T>(initialValue: T): [() => T, (value: T | ((prev: T) => T)) => void] {
  const s = new Signal(initialValue);
  return [() => s.read(), (v) => s.write(v)];
}

export function createEffect(fn: () => void | (() => void)): void {
  const c: Computation = {
    fn,
    cleanups: [],
    sources: new Set(),
    stale: false,
  };
  runComputation(c);
}

export function createMemo<T>(fn: () => T): () => T {
  const [get, set] = createSignal<T>(undefined as any);
  createEffect(() => {
    set(fn());
  });
  return get;
}

export function batch<T>(fn: () => T): T {
  batchDepth++;
  try {
    return fn();
  } finally {
    batchDepth--;
    if (batchDepth === 0) {
      const pending = Array.from(pendingComputations);
      pendingComputations.clear();
      for (let i = 0; i < pending.length; i++) {
        const c = pending[i];
        if (c.stale) {
          runComputation(c);
        }
      }
    }
  }
}

export function untrack<T>(fn: () => T): T {
  const prev = currentListener;
  currentListener = null;
  try {
    return fn();
  } finally {
    currentListener = prev;
  }
}

export function onCleanup(fn: () => void): void {
  if (currentListener) {
    currentListener.cleanups.push(fn);
  }
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
  createElement,
  h,
  Fragment,
  dialog,
  clipboard,
  systemTheme,
  turnMailbox,
  nodeRegistry,
  createSignal,
  createEffect,
  createMemo,
  batch,
  untrack,
  onCleanup,
};

