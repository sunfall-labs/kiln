// Virtual native GUI built-in module for Kiln (ADR 0074).
// Exposes cross-platform native UI widget primitives, an in-memory virtual tree
// for headless testability, and a fine-grained universal renderer seam for SolidJS 2.0.

const G = globalThis as any;
let nextNodeId = 1;
export const nodeRegistry = new Map<number, Node>();

// Platform OS Windowing Drivers (ADR 0074)
export interface PlatformDriver {
  readonly name: string;
  init(): boolean;
  getHandle(nodeId: number): bigint | null;
  createWindow(win: Window): bigint | null;
  showWindow(win: Window): void;
  closeWindow(win: Window): void;
  maximizeWindow?(win: Window): void;
  unmaximizeWindow?(win: Window): void;
  minimizeWindow?(win: Window): void;
  setFullscreen?(win: Window, fullscreen: boolean): void;
  setResizable?(win: Window, resizable: boolean): void;
  setWindowContent(winHandle: bigint, childHandle: bigint): void;
  createWidget(node: Node): bigint | null;
  updateProperty(handle: bigint, tag: string, name: string, value: any): void;
  insertChild(parentHandle: bigint, childHandle: bigint, anchorHandle: bigint | null): void;
  removeChild(parentHandle: bigint, childHandle: bigint): void;
  reorderChild(parentHandle: bigint, childHandle: bigint, anchorHandle: bigint | null): void;
  disposeWidget(node: Node): void;
  pumpEvents(): boolean;
}

function toCStr(ffi: any, str: string): bigint {
  const enc = new TextEncoder();
  const bytes = enc.encode(str + "\0");
  return ffi.getRawPointer(bytes.buffer);
}

function fromCStr(ffi: any, ptr: bigint, maxLen = 4096): string {
  if (!ptr || !ffi || typeof ffi.toArrayBuffer !== "function") return "";
  try {
    const buf = ffi.toArrayBuffer(ptr, maxLen);
    const u8 = new Uint8Array(buf);
    let len = 0;
    while (len < u8.length && u8[len] !== 0) len++;
    return new TextDecoder().decode(u8.subarray(0, len));
  } catch (_) {
    return "";
  }
}

export class Gtk4Driver implements PlatformDriver {
  readonly name = "gtk4";
  private initialized = false;
  private gtk: any = null;
  private ffi: any = null;
  private handles = new Map<number, bigint>();
  private callbacks = new Map<number, bigint[]>();

  init(): boolean {
    if (this.initialized) return this.gtk !== null;
    this.initialized = true;
    try {
      this.ffi = (typeof process !== "undefined" && (process as any).__kilnNativeModules?.ffi) || null;
      if (!this.ffi) return false;
      const hasDisplay = typeof process !== "undefined" && process.env && (process.env.WAYLAND_DISPLAY || process.env.DISPLAY);
      if (!hasDisplay) return false;
      if (process.platform !== "linux") return false;

      this.gtk = this.ffi.dlopen("libgtk-4.so.1", {
        gtk_init_check: { arguments: [], return: "i32" },
        gtk_window_new: { arguments: [], return: "pointer" },
        gtk_window_set_title: { arguments: ["pointer", "pointer"], return: "void" },
        gtk_window_set_default_size: { arguments: ["pointer", "i32", "i32"], return: "void" },
        gtk_window_set_child: { arguments: ["pointer", "pointer"], return: "void" },
        gtk_window_present: { arguments: ["pointer"], return: "void" },
        gtk_window_close: { arguments: ["pointer"], return: "void" },
        gtk_window_maximize: { arguments: ["pointer"], return: "void" },
        gtk_window_unmaximize: { arguments: ["pointer"], return: "void" },
        gtk_window_minimize: { arguments: ["pointer"], return: "void" },
        gtk_window_fullscreen: { arguments: ["pointer"], return: "void" },
        gtk_window_unfullscreen: { arguments: ["pointer"], return: "void" },
        gtk_window_set_resizable: { arguments: ["pointer", "i32"], return: "void" },
        gtk_box_new: { arguments: ["i32", "i32"], return: "pointer" },
        gtk_box_append: { arguments: ["pointer", "pointer"], return: "void" },
        gtk_box_remove: { arguments: ["pointer", "pointer"], return: "void" },
        gtk_button_new_with_label: { arguments: ["pointer"], return: "pointer" },
        gtk_button_set_label: { arguments: ["pointer", "pointer"], return: "void" },
        gtk_label_new: { arguments: ["pointer"], return: "pointer" },
        gtk_label_set_text: { arguments: ["pointer", "pointer"], return: "void" },
        gtk_entry_new: { arguments: [], return: "pointer" },
        gtk_editable_set_text: { arguments: ["pointer", "pointer"], return: "void" },
        gtk_editable_get_text: { arguments: ["pointer"], return: "pointer" },
        gtk_switch_new: { arguments: [], return: "pointer" },
        gtk_switch_set_active: { arguments: ["pointer", "i32"], return: "void" },
        gtk_switch_get_active: { arguments: ["pointer"], return: "i32" },
        gtk_scale_new_with_range: { arguments: ["i32", "f64", "f64", "f64"], return: "pointer" },
        gtk_range_set_value: { arguments: ["pointer", "f64"], return: "void" },
        gtk_range_get_value: { arguments: ["pointer"], return: "f64" },
        gtk_widget_set_margin_top: { arguments: ["pointer", "i32"], return: "void" },
        gtk_widget_set_margin_bottom: { arguments: ["pointer", "i32"], return: "void" },
        gtk_widget_set_margin_start: { arguments: ["pointer", "i32"], return: "void" },
        gtk_widget_set_margin_end: { arguments: ["pointer", "i32"], return: "void" },
        gtk_widget_set_halign: { arguments: ["pointer", "i32"], return: "void" },
        gtk_widget_set_valign: { arguments: ["pointer", "i32"], return: "void" },
        g_signal_connect_data: { arguments: ["pointer", "pointer", "pointer", "pointer", "pointer", "i32"], return: "u64" },
        g_main_context_iteration: { arguments: ["pointer", "i32"], return: "i32" },
        g_main_context_pending: { arguments: ["pointer"], return: "i32" },
      });

      if (this.gtk && this.gtk.functions && this.gtk.functions.gtk_init_check && this.gtk.functions.gtk_init_check() === 1) {
        return true;
      }
    } catch (_) {}
    this.gtk = null;
    return false;
  }

  getHandle(nodeId: number): bigint | null {
    return this.handles.get(nodeId) || null;
  }

  createWindow(win: Window): bigint | null {
    if (!this.gtk) return null;
    let handle = this.handles.get(win.id);
    if (handle) return handle;

    try {
      handle = this.gtk.functions.gtk_window_new();
      this.handles.set(win.id, handle);
      this.gtk.functions.gtk_window_set_title(handle, toCStr(this.ffi, win.title || "Kiln Window"));
      this.gtk.functions.gtk_window_set_default_size(handle, win.width || 800, win.height || 600);

      const closeCallbackPtr = this.gtk.lib.registerCallback(
        { arguments: ["pointer", "pointer"], return: "i32" },
        () => {
          win.visible = false;
          turnMailbox.post(win.id, "close");
          return 0;
        }
      );
      this.callbacks.set(win.id, [closeCallbackPtr]);
      this.gtk.functions.g_signal_connect_data(handle, toCStr(this.ffi, "close-request"), closeCallbackPtr, 0n, 0n, 0);
      return handle;
    } catch (_) {
      return null;
    }
  }

  setWindowContent(winHandle: bigint, childHandle: bigint): void {
    if (!this.gtk) return;
    try {
      this.gtk.functions.gtk_widget_set_margin_top(childHandle, 24);
      this.gtk.functions.gtk_widget_set_margin_bottom(childHandle, 24);
      this.gtk.functions.gtk_widget_set_margin_start(childHandle, 24);
      this.gtk.functions.gtk_widget_set_margin_end(childHandle, 24);
      this.gtk.functions.gtk_window_set_child(winHandle, childHandle);
    } catch (_) {}
  }

  showWindow(win: Window): void {
    if (!this.gtk) return;
    let winHandle = this.handles.get(win.id);
    if (!winHandle) {
      winHandle = this.createWindow(win);
    }
    if (winHandle) {
      if (win.content) {
        const childHandle = this.createWidget(win.content);
        if (childHandle) {
          this.setWindowContent(winHandle, childHandle);
        }
      }
      this.gtk.functions.gtk_window_present(winHandle);
      this.pumpEvents();
    }
  }

  closeWindow(win: Window): void {
    const handle = this.handles.get(win.id);
    if (handle && this.gtk) {
      try {
        this.gtk.functions.gtk_window_close(handle);
      } catch (_) {}
      this.handles.delete(win.id);
      this.callbacks.delete(win.id);
    }
  }

  maximizeWindow(win: Window): void {
    const handle = this.handles.get(win.id);
    if (handle && this.gtk?.functions?.gtk_window_maximize) {
      try {
        this.gtk.functions.gtk_window_maximize(handle);
      } catch (_) {}
    }
  }

  unmaximizeWindow(win: Window): void {
    const handle = this.handles.get(win.id);
    if (handle && this.gtk?.functions?.gtk_window_unmaximize) {
      try {
        this.gtk.functions.gtk_window_unmaximize(handle);
      } catch (_) {}
    }
  }

  minimizeWindow(win: Window): void {
    const handle = this.handles.get(win.id);
    if (handle && this.gtk?.functions?.gtk_window_minimize) {
      try {
        this.gtk.functions.gtk_window_minimize(handle);
      } catch (_) {}
    }
  }

  setFullscreen(win: Window, fullscreen: boolean): void {
    const handle = this.handles.get(win.id);
    if (handle && this.gtk?.functions) {
      try {
        if (fullscreen) {
          if (this.gtk.functions.gtk_window_fullscreen) {
            this.gtk.functions.gtk_window_fullscreen(handle);
          }
        } else {
          if (this.gtk.functions.gtk_window_unfullscreen) {
            this.gtk.functions.gtk_window_unfullscreen(handle);
          }
        }
      } catch (_) {}
    }
  }

  setResizable(win: Window, resizable: boolean): void {
    const handle = this.handles.get(win.id);
    if (handle && this.gtk?.functions?.gtk_window_set_resizable) {
      try {
        this.gtk.functions.gtk_window_set_resizable(handle, resizable ? 1 : 0);
      } catch (_) {}
    }
  }

  createWidget(node: Node): bigint | null {
    if (!this.gtk) return null;
    let handle = this.handles.get(node.id);
    if (handle) return handle;

    try {
      const ffi = this.ffi;
      const gtk = this.gtk;
      const nodeCallbacks: bigint[] = [];

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
        nodeCallbacks.push(callbackPtr);
        gtk.functions.g_signal_connect_data(handle, toCStr(ffi, "clicked"), callbackPtr, 0n, 0n, 0);
      } else if (node.tag === "text") {
        const text = node.getProperty("text") || "";
        handle = gtk.functions.gtk_label_new(toCStr(ffi, String(text)));
      } else if (node.tag === "textinput") {
        handle = gtk.functions.gtk_entry_new();
        const val = node.getProperty("value");
        if (val) gtk.functions.gtk_editable_set_text(handle, toCStr(ffi, String(val)));
        const changedCallbackPtr = gtk.lib.registerCallback(
          { arguments: ["pointer", "pointer"], return: "void" },
          () => {
            const text = fromCStr(ffi, gtk.functions.gtk_editable_get_text(handle));
            turnMailbox.post(node.id, "input", text);
            turnMailbox.post(node.id, "change", text);
          }
        );
        nodeCallbacks.push(changedCallbackPtr);
        gtk.functions.g_signal_connect_data(handle, toCStr(ffi, "changed"), changedCallbackPtr, 0n, 0n, 0);
      } else if (node.tag === "slider") {
        const min = node.getProperty("min") || 0;
        const max = node.getProperty("max") || 100;
        handle = gtk.functions.gtk_scale_new_with_range(0, min, max, 1.0);
        const val = node.getProperty("value");
        if (val !== undefined) gtk.functions.gtk_range_set_value(handle, Number(val));
        const sliderCallbackPtr = gtk.lib.registerCallback(
          { arguments: ["pointer", "pointer"], return: "void" },
          () => {
            const currentVal = gtk.functions.gtk_range_get_value(handle);
            turnMailbox.post(node.id, "change", currentVal);
          }
        );
        nodeCallbacks.push(sliderCallbackPtr);
        gtk.functions.g_signal_connect_data(handle, toCStr(ffi, "value-changed"), sliderCallbackPtr, 0n, 0n, 0);
      } else if (node.tag === "toggle") {
        handle = gtk.functions.gtk_switch_new();
        const chk = node.getProperty("checked");
        if (chk) gtk.functions.gtk_switch_set_active(handle, 1);
        const toggleCallbackPtr = gtk.lib.registerCallback(
          { arguments: ["pointer", "i32", "pointer"], return: "i32" },
          () => {
            const active = gtk.functions.gtk_switch_get_active(handle) !== 0;
            turnMailbox.post(node.id, "toggle", active);
            return 0;
          }
        );
        nodeCallbacks.push(toggleCallbackPtr);
        gtk.functions.g_signal_connect_data(handle, toCStr(ffi, "state-set"), toggleCallbackPtr, 0n, 0n, 0);
      } else {
        handle = gtk.functions.gtk_box_new(1, 0);
      }

      if (handle) {
        this.handles.set(node.id, handle);
        if (nodeCallbacks.length > 0) {
          this.callbacks.set(node.id, nodeCallbacks);
        }
        if (node.tag === "vstack" || node.tag === "hstack") {
          for (let i = 0; i < node.children.length; i++) {
            const childHandle = this.createWidget(node.children[i]);
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

  updateProperty(handle: bigint, tag: string, name: string, value: any): void {
    if (!this.gtk) return;
    try {
      if (tag === "button" && name === "label") {
        this.gtk.functions.gtk_button_set_label(handle, toCStr(this.ffi, String(value)));
      } else if (tag === "text" && (name === "text" || name === "value")) {
        this.gtk.functions.gtk_label_set_text(handle, toCStr(this.ffi, String(value)));
      } else if (tag === "textinput" && name === "value") {
        this.gtk.functions.gtk_editable_set_text(handle, toCStr(this.ffi, String(value)));
      } else if (tag === "slider" && name === "value") {
        this.gtk.functions.gtk_range_set_value(handle, Number(value));
      } else if (tag === "toggle" && name === "checked") {
        this.gtk.functions.gtk_switch_set_active(handle, value ? 1 : 0);
      } else if (tag === "window" && name === "title") {
        this.gtk.functions.gtk_window_set_title(handle, toCStr(this.ffi, String(value)));
      }
    } catch (_) {}
  }

  insertChild(parentHandle: bigint, childHandle: bigint, anchorHandle: bigint | null): void {
    if (!this.gtk) return;
    try {
      this.gtk.functions.gtk_box_append(parentHandle, childHandle);
    } catch (_) {}
  }

  removeChild(parentHandle: bigint, childHandle: bigint): void {
    if (!this.gtk) return;
    try {
      this.gtk.functions.gtk_box_remove(parentHandle, childHandle);
    } catch (_) {}
  }

  reorderChild(parentHandle: bigint, childHandle: bigint, anchorHandle: bigint | null): void {
    if (!this.gtk) return;
    try {
      this.gtk.functions.gtk_box_remove(parentHandle, childHandle);
      this.gtk.functions.gtk_box_append(parentHandle, childHandle);
    } catch (_) {}
  }

  disposeWidget(node: Node): void {
    const handle = this.handles.get(node.id);
    if (handle) {
      if (node.tag === "window" && this.gtk) {
        try {
          this.gtk.functions.gtk_window_close(handle);
        } catch (_) {}
      }
      this.handles.delete(node.id);
    }
    this.callbacks.delete(node.id);
  }

  pumpEvents(): boolean {
    if (!this.gtk) return false;
    let hadEvents = false;
    try {
      while (this.gtk.functions.g_main_context_iteration(0n, 0) !== 0) {
        hadEvents = true;
      }
    } catch (_) {}
    return hadEvents;
  }
}

export class AppKitDriver implements PlatformDriver {
  readonly name = "appkit";
  private initialized = false;
  private handles = new Map<number, bigint>();

  init(): boolean {
    if (this.initialized) return true;
    this.initialized = true;
    return typeof process !== "undefined" && process.platform === "darwin";
  }

  getHandle(nodeId: number): bigint | null { return this.handles.get(nodeId) || null; }
  createWindow(win: Window): bigint | null { return null; }
  setWindowContent(winHandle: bigint, childHandle: bigint): void {}
  showWindow(win: Window): void { win.visible = true; }
  closeWindow(win: Window): void { win.visible = false; }
  createWidget(node: Node): bigint | null { return null; }
  updateProperty(handle: bigint, tag: string, name: string, value: any): void {}
  insertChild(parentHandle: bigint, childHandle: bigint, anchorHandle: bigint | null): void {}
  removeChild(parentHandle: bigint, childHandle: bigint): void {}
  reorderChild(parentHandle: bigint, childHandle: bigint, anchorHandle: bigint | null): void {}
  disposeWidget(node: Node): void { this.handles.delete(node.id); }
  pumpEvents(): boolean { return false; }
}

export class Win32Driver implements PlatformDriver {
  readonly name = "win32";
  private initialized = false;
  private handles = new Map<number, bigint>();

  init(): boolean {
    if (this.initialized) return true;
    this.initialized = true;
    return typeof process !== "undefined" && process.platform === "win32";
  }

  getHandle(nodeId: number): bigint | null { return this.handles.get(nodeId) || null; }
  createWindow(win: Window): bigint | null { return null; }
  setWindowContent(winHandle: bigint, childHandle: bigint): void {}
  showWindow(win: Window): void { win.visible = true; }
  closeWindow(win: Window): void { win.visible = false; }
  createWidget(node: Node): bigint | null { return null; }
  updateProperty(handle: bigint, tag: string, name: string, value: any): void {}
  insertChild(parentHandle: bigint, childHandle: bigint, anchorHandle: bigint | null): void {}
  removeChild(parentHandle: bigint, childHandle: bigint): void {}
  reorderChild(parentHandle: bigint, childHandle: bigint, anchorHandle: bigint | null): void {}
  disposeWidget(node: Node): void { this.handles.delete(node.id); }
  pumpEvents(): boolean { return false; }
}

export class HeadlessDriver implements PlatformDriver {
  readonly name = "headless";
  private handles = new Map<number, bigint>();

  init(): boolean { return true; }
  getHandle(nodeId: number): bigint | null { return this.handles.get(nodeId) || null; }
  createWindow(win: Window): bigint | null { return null; }
  setWindowContent(winHandle: bigint, childHandle: bigint): void {}
  showWindow(win: Window): void { win.visible = true; }
  closeWindow(win: Window): void { win.visible = false; }
  createWidget(node: Node): bigint | null { return null; }
  updateProperty(handle: bigint, tag: string, name: string, value: any): void {}
  insertChild(parentHandle: bigint, childHandle: bigint, anchorHandle: bigint | null): void {}
  removeChild(parentHandle: bigint, childHandle: bigint): void {}
  reorderChild(parentHandle: bigint, childHandle: bigint, anchorHandle: bigint | null): void {}
  disposeWidget(node: Node): void { this.handles.delete(node.id); }
  pumpEvents(): boolean { return false; }
}

let activeDriver: PlatformDriver | null = null;

export function getActiveDriver(): PlatformDriver {
  if (activeDriver) return activeDriver;

  if (typeof process !== "undefined") {
    if (process.platform === "linux") {
      const gtk = new Gtk4Driver();
      if (gtk.init()) {
        activeDriver = gtk;
        return activeDriver;
      }
    } else if (process.platform === "darwin") {
      const appkit = new AppKitDriver();
      if (appkit.init()) {
        activeDriver = appkit;
        return activeDriver;
      }
    } else if (process.platform === "win32") {
      const win32 = new Win32Driver();
      if (win32.init()) {
        activeDriver = win32;
        return activeDriver;
      }
    }
  }

  activeDriver = new HeadlessDriver();
  return activeDriver;
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
    const driver = getActiveDriver();
    if (driver) {
      driver.disposeWidget(this);
    }
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
      const driver = getActiveDriver();
      const parentHandle = driver.getHandle(this.id);
      const childHandle = driver.getHandle(child.id);
      if (parentHandle && childHandle) {
        driver.removeChild(parentHandle, childHandle);
      }
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
    const driver = getActiveDriver();
    const parentHandle = driver.getHandle(this.id);
    if (parentHandle) {
      const childHandle = driver.createWidget(child);
      if (childHandle) {
        const anchorHandle = anchor ? driver.getHandle(anchor.id) || null : null;
        driver.insertChild(parentHandle, childHandle, anchorHandle);
      }
    }
    if (typeof G.__kilnGuiInsertNode === "function") {
      G.__kilnGuiInsertNode(this.id, child.id, anchor ? anchor.id : 0);
    }
  }

  moveChild(child: Node, beforeAnchor: Node | null): void {
    const oldIndex = this.children.indexOf(child);
    if (oldIndex < 0) return;
    this.children.splice(oldIndex, 1);
    if (beforeAnchor) {
      const newIndex = this.children.indexOf(beforeAnchor);
      if (newIndex >= 0) {
        this.children.splice(newIndex, 0, child);
      } else {
        this.children.push(child);
      }
    } else {
      this.children.push(child);
    }
    const driver = getActiveDriver();
    const parentHandle = driver.getHandle(this.id);
    const childHandle = driver.getHandle(child.id);
    if (parentHandle && childHandle) {
      const anchorHandle = beforeAnchor ? driver.getHandle(beforeAnchor.id) || null : null;
      driver.reorderChild(parentHandle, childHandle, anchorHandle);
    }
  }

  setProperty(name: string, value: any): void {
    this.properties[name] = value;
    if (name.startsWith("on") && typeof value === "function") {
      let eventName = name.slice(2).toLowerCase();
      if (eventName === "press") eventName = "click";
      this.addEventListener(eventName, value);
      if (eventName === "change") {
        this.addEventListener("input", value);
      }
    }
    const driver = getActiveDriver();
    const handle = driver.getHandle(this.id);
    if (handle) {
      driver.updateProperty(handle, this.tag, name, value);
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
    if ((event === "input" || event === "change") && typeof args[0] === "string") {
      this.properties.value = args[0];
    } else if (event === "change" && typeof args[0] === "number") {
      this.properties.value = args[0];
    } else if (event === "toggle" && typeof args[0] === "boolean") {
      this.properties.checked = args[0];
    } else if (event === "close") {
      this.properties.visible = false;
    }
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
      const driver = getActiveDriver();
      const winHandle = driver.getHandle(this.id);
      if (winHandle) {
        const childHandle = driver.createWidget(node);
        if (childHandle) {
          driver.setWindowContent(winHandle, childHandle);
        }
      }
    }
  }

  show(): void {
    this.visible = true;
    const driver = getActiveDriver();
    driver.showWindow(this);
    if (typeof G.__kilnGuiShowWindow === "function") {
      G.__kilnGuiShowWindow(this.id);
    }
  }

  close(): void {
    this.visible = false;
    const driver = getActiveDriver();
    driver.closeWindow(this);
    if (typeof G.__kilnGuiCloseWindow === "function") {
      G.__kilnGuiCloseWindow(this.id);
    }
  }

  maximize(): void {
    const driver = getActiveDriver();
    if (typeof driver.maximizeWindow === "function") {
      driver.maximizeWindow(this);
    }
    this.dispatchEvent("maximize");
  }

  unmaximize(): void {
    const driver = getActiveDriver();
    if (typeof driver.unmaximizeWindow === "function") {
      driver.unmaximizeWindow(this);
    }
    this.dispatchEvent("unmaximize");
  }

  minimize(): void {
    const driver = getActiveDriver();
    if (typeof driver.minimizeWindow === "function") {
      driver.minimizeWindow(this);
    }
    this.dispatchEvent("minimize");
  }

  setFullscreen(fullscreen: boolean): void {
    const driver = getActiveDriver();
    if (typeof driver.setFullscreen === "function") {
      driver.setFullscreen(this, fullscreen);
    }
    this.setProperty("fullscreen", fullscreen);
  }

  get fullscreen(): boolean {
    return !!this.properties.fullscreen;
  }
  set fullscreen(val: boolean) {
    this.setFullscreen(val);
  }

  setResizable(resizable: boolean): void {
    const driver = getActiveDriver();
    if (typeof driver.setResizable === "function") {
      driver.setResizable(this, resizable);
    }
    this.setProperty("resizable", resizable);
  }

  get resizable(): boolean {
    return this.properties.resizable !== false;
  }
  set resizable(val: boolean) {
    this.setResizable(val);
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

export interface PickFolderOptions {
  title?: string;
  defaultPath?: string;
}

export interface NotificationOptions {
  title: string;
  body?: string;
  appName?: string;
}

export interface MessageOptions {
  title?: string;
  message: string;
  type?: "info" | "warning" | "error";
}

export class HostGuiBridge {
  private initialized = false;
  private lib: any = null;
  private ffi: any = null;

  init(): boolean {
    if (this.initialized) return this.lib !== null;
    this.initialized = true;
    try {
      this.ffi = (typeof process !== "undefined" && (process as any).__kilnNativeModules?.ffi) || null;
      if (!this.ffi) return false;

      const fs = (typeof process !== "undefined" && (process as any).__kilnNativeModules?.fs) || null;
      const suffix = this.ffi.suffix || ".so";

      const candidates: string[] = [];
      if (typeof process !== "undefined" && process.env && process.env.KILN_HOST_GUI_LIB) {
        candidates.push(process.env.KILN_HOST_GUI_LIB);
      }
      const cwd = typeof process !== "undefined" && typeof process.cwd === "function" ? process.cwd() : ".";
      candidates.push(
        cwd + "/target/release/libkiln_host_gui" + suffix,
        cwd + "/target/debug/libkiln_host_gui" + suffix,
        "./target/release/libkiln_host_gui" + suffix,
        "./target/debug/libkiln_host_gui" + suffix
      );

      let targetLib: string | null = null;
      if (fs && typeof fs.existsSync === "function") {
        for (const candidate of candidates) {
          try {
            if (fs.existsSync(candidate)) {
              targetLib = candidate;
              break;
            }
          } catch (_) {}
        }
      }

      if (!targetLib) return false;

      this.lib = this.ffi.dlopen(targetLib, {
        kiln_gui_dialog_open_file: { arguments: ["pointer", "pointer", "pointer", "pointer", "u64"], return: "i32" },
        kiln_gui_dialog_pick_folder: { arguments: ["pointer", "pointer", "pointer", "u64"], return: "i32" },
        kiln_gui_dialog_save_file: { arguments: ["pointer", "pointer", "pointer", "pointer", "pointer", "u64"], return: "i32" },
        kiln_gui_dialog_message: { arguments: ["pointer", "pointer", "pointer"], return: "i32" },
        kiln_gui_clipboard_read: { arguments: ["pointer", "u64"], return: "i32" },
        kiln_gui_clipboard_write: { arguments: ["pointer"], return: "i32" },
        kiln_gui_clipboard_clear: { arguments: [], return: "i32" },
        kiln_gui_get_theme: { arguments: [], return: "i32" },
        kiln_gui_notification_show: { arguments: ["pointer", "pointer", "pointer"], return: "i32" },
        kiln_gui_tray_create: { arguments: ["pointer"], return: "u32" },
        kiln_gui_tray_set_tooltip: { arguments: ["u32", "pointer"], return: "i32" },
        kiln_gui_tray_destroy: { arguments: ["u32"], return: "i32" },
        kiln_gui_tray_clear: { arguments: [], return: "i32" },
        kiln_gui_app_get_path: { arguments: ["pointer", "pointer", "u64"], return: "i32" },
        kiln_gui_app_single_instance_lock: { arguments: ["pointer"], return: "i32" },
        kiln_gui_hotkey_register: { arguments: ["pointer"], return: "u32" },
        kiln_gui_hotkey_unregister: { arguments: ["u32"], return: "i32" },
        kiln_gui_hotkey_poll: { arguments: [], return: "u32" },
      });
      return this.lib !== null;
    } catch (_) {
      return false;
    }
  }

  openFile(options?: OpenFileOptions): string | null {
    if (!this.init() || !this.lib) return null;
    try {
      const titlePtr = options?.title ? toCStr(this.ffi, options.title) : 0n;
      let filterNamePtr = 0n;
      let filterExtsPtr = 0n;
      if (options?.filters && options.filters.length > 0) {
        filterNamePtr = toCStr(this.ffi, options.filters[0].name);
        filterExtsPtr = toCStr(this.ffi, options.filters[0].extensions.join(","));
      }
      const buf = new Uint8Array(4096);
      const bufPtr = this.ffi.getRawPointer(buf.buffer);
      const len = this.lib.kiln_gui_dialog_open_file(titlePtr, filterNamePtr, filterExtsPtr, bufPtr, BigInt(buf.length));
      if (len > 0) {
        return new TextDecoder().decode(buf.subarray(0, len));
      }
      return null;
    } catch (_) {
      return null;
    }
  }

  pickFolder(options?: PickFolderOptions): string | null {
    if (!this.init() || !this.lib) return null;
    try {
      const titlePtr = options?.title ? toCStr(this.ffi, options.title) : 0n;
      const defaultPathPtr = options?.defaultPath ? toCStr(this.ffi, options.defaultPath) : 0n;
      const buf = new Uint8Array(4096);
      const bufPtr = this.ffi.getRawPointer(buf.buffer);
      const len = this.lib.kiln_gui_dialog_pick_folder(titlePtr, defaultPathPtr, bufPtr, BigInt(buf.length));
      if (len > 0) {
        return new TextDecoder().decode(buf.subarray(0, len));
      }
      return null;
    } catch (_) {
      return null;
    }
  }

  saveFile(options?: SaveFileOptions): string | null {
    if (!this.init() || !this.lib) return null;
    try {
      const titlePtr = options?.title ? toCStr(this.ffi, options.title) : 0n;
      const defaultPathPtr = options?.defaultPath ? toCStr(this.ffi, options.defaultPath) : 0n;
      let filterNamePtr = 0n;
      let filterExtsPtr = 0n;
      if (options?.filters && options.filters.length > 0) {
        filterNamePtr = toCStr(this.ffi, options.filters[0].name);
        filterExtsPtr = toCStr(this.ffi, options.filters[0].extensions.join(","));
      }
      const buf = new Uint8Array(4096);
      const bufPtr = this.ffi.getRawPointer(buf.buffer);
      const len = this.lib.kiln_gui_dialog_save_file(titlePtr, defaultPathPtr, filterNamePtr, filterExtsPtr, bufPtr, BigInt(buf.length));
      if (len > 0) {
        return new TextDecoder().decode(buf.subarray(0, len));
      }
      return null;
    } catch (_) {
      return null;
    }
  }

  showMessage(options: MessageOptions): void {
    if (!this.init() || !this.lib) return;
    try {
      const titlePtr = options.title ? toCStr(this.ffi, options.title) : 0n;
      const msgPtr = toCStr(this.ffi, options.message);
      const kindPtr = options.kind ? toCStr(this.ffi, options.kind) : 0n;
      this.lib.kiln_gui_dialog_message(titlePtr, msgPtr, kindPtr);
    } catch (_) {}
  }

  clipboardRead(): string | null {
    if (!this.init() || !this.lib) return null;
    try {
      const buf = new Uint8Array(8192);
      const bufPtr = this.ffi.getRawPointer(buf.buffer);
      const len = this.lib.kiln_gui_clipboard_read(bufPtr, BigInt(buf.length));
      if (len >= 0) {
        return new TextDecoder().decode(buf.subarray(0, len));
      }
      return null;
    } catch (_) {
      return null;
    }
  }

  clipboardWrite(text: string): boolean {
    if (!this.init() || !this.lib) return false;
    try {
      const textPtr = toCStr(this.ffi, text);
      const res = this.lib.kiln_gui_clipboard_write(textPtr);
      return res === 0;
    } catch (_) {
      return false;
    }
  }

  clipboardClear(): boolean {
    if (!this.init() || !this.lib) return false;
    try {
      return this.lib.kiln_gui_clipboard_clear() === 0;
    } catch (_) {
      return false;
    }
  }

  getTheme(): "dark" | "light" | null {
    if (!this.init() || !this.lib) return null;
    try {
      const code = this.lib.kiln_gui_get_theme();
      return code === 0 ? "dark" : "light";
    } catch (_) {
      return null;
    }
  }

  showNotification(options: NotificationOptions): boolean {
    if (!this.init() || !this.lib) return false;
    try {
      const titlePtr = toCStr(this.ffi, options.title);
      const bodyPtr = options.body ? toCStr(this.ffi, options.body) : 0n;
      const appNamePtr = options.appName ? toCStr(this.ffi, options.appName) : 0n;
      return this.lib.kiln_gui_notification_show(titlePtr, bodyPtr, appNamePtr) === 0;
    } catch (_) {
      return false;
    }
  }

  createTray(tooltip: string): number | null {
    if (!this.init() || !this.lib) return null;
    try {
      const tipPtr = toCStr(this.ffi, tooltip);
      const id = this.lib.kiln_gui_tray_create(tipPtr);
      return id > 0 ? id : null;
    } catch (_) {
      return null;
    }
  }

  setTrayTooltip(id: number, tooltip: string): boolean {
    if (!this.init() || !this.lib) return false;
    try {
      const tipPtr = toCStr(this.ffi, tooltip);
      return this.lib.kiln_gui_tray_set_tooltip(id, tipPtr) === 0;
    } catch (_) {
      return false;
    }
  }

  destroyTray(id: number): boolean {
    if (!this.init() || !this.lib) return false;
    try {
      return this.lib.kiln_gui_tray_destroy(id) === 0;
    } catch (_) {
      return false;
    }
  }

  clearTrays(): void {
    if (!this.init() || !this.lib) return;
    try {
      this.lib.kiln_gui_tray_clear();
    } catch (_) {}
  }

  appGetPath(name: string): string | null {
    if (!this.init() || !this.lib) return null;
    try {
      const namePtr = toCStr(this.ffi, name);
      const buf = new Uint8Array(4096);
      const bufPtr = this.ffi.getRawPointer(buf.buffer);
      const len = this.lib.kiln_gui_app_get_path(namePtr, bufPtr, BigInt(buf.length));
      if (len > 0) {
        return new TextDecoder().decode(buf.subarray(0, len));
      }
      return null;
    } catch (_) {
      return null;
    }
  }

  appSingleInstanceLock(appId: string): boolean {
    if (!this.init() || !this.lib) return true;
    try {
      const appIdPtr = toCStr(this.ffi, appId);
      return this.lib.kiln_gui_app_single_instance_lock(appIdPtr) === 0;
    } catch (_) {
      return true;
    }
  }

  hotkeyRegister(accelerator: string): number {
    if (!this.init() || !this.lib) return 0;
    try {
      const accPtr = toCStr(this.ffi, accelerator);
      return this.lib.kiln_gui_hotkey_register(accPtr);
    } catch (_) {
      return 0;
    }
  }

  hotkeyUnregister(id: number): boolean {
    if (!this.init() || !this.lib) return false;
    try {
      return this.lib.kiln_gui_hotkey_unregister(id) === 0;
    } catch (_) {
      return false;
    }
  }

  hotkeyPoll(): number {
    if (!this.init() || !this.lib) return 0;
    try {
      return this.lib.kiln_gui_hotkey_poll();
    } catch (_) {
      return 0;
    }
  }
}

export const hostGuiBridge = new HostGuiBridge();

export const dialog = {
  async openFile(options?: OpenFileOptions): Promise<string | null> {
    if (typeof G.__kilnGuiOpenFile === "function") {
      return G.__kilnGuiOpenFile(options);
    }
    const nativeMod = (typeof process !== "undefined" && (process as any).__kilnNativeModules?.gui) || null;
    if (typeof nativeMod?.openFile === "function") {
      return nativeMod.openFile(options);
    }
    const hostPath = hostGuiBridge.openFile(options);
    if (hostPath !== null) return hostPath;
    return null;
  },
  async openFiles(options?: OpenFileOptions): Promise<string[]> {
    if (typeof G.__kilnGuiOpenFiles === "function") {
      return G.__kilnGuiOpenFiles(options);
    }
    const single = await this.openFile(options);
    return single ? [single] : [];
  },
  async pickFolder(options?: PickFolderOptions): Promise<string | null> {
    if (typeof G.__kilnGuiPickFolder === "function") {
      return G.__kilnGuiPickFolder(options);
    }
    const nativeMod = (typeof process !== "undefined" && (process as any).__kilnNativeModules?.gui) || null;
    if (typeof nativeMod?.pickFolder === "function") {
      return nativeMod.pickFolder(options);
    }
    const hostPath = hostGuiBridge.pickFolder(options);
    if (hostPath !== null) return hostPath;
    return null;
  },
  async saveFile(options?: SaveFileOptions): Promise<string | null> {
    if (typeof G.__kilnGuiSaveFile === "function") {
      return G.__kilnGuiSaveFile(options);
    }
    const nativeMod = (typeof process !== "undefined" && (process as any).__kilnNativeModules?.gui) || null;
    if (typeof nativeMod?.saveFile === "function") {
      return nativeMod.saveFile(options);
    }
    const hostPath = hostGuiBridge.saveFile(options);
    if (hostPath !== null) return hostPath;
    return null;
  },
  async message(options: MessageOptions): Promise<void> {
    if (typeof G.__kilnGuiShowMessage === "function") {
      return G.__kilnGuiShowMessage(options);
    }
    const nativeMod = (typeof process !== "undefined" && (process as any).__kilnNativeModules?.gui) || null;
    if (typeof nativeMod?.showMessage === "function") {
      return nativeMod.showMessage(options);
    }
    hostGuiBridge.showMessage(options);
  },
};

let virtualClipboard = "";
export const clipboard = {
  async readText(): Promise<string> {
    if (typeof G.__kilnGuiClipboardRead === "function") {
      return G.__kilnGuiClipboardRead();
    }
    const nativeMod = (typeof process !== "undefined" && (process as any).__kilnNativeModules?.gui) || null;
    if (typeof nativeMod?.readText === "function") {
      return nativeMod.readText();
    }
    const hostText = hostGuiBridge.clipboardRead();
    if (hostText !== null) return hostText;
    return virtualClipboard;
  },
  async writeText(text: string): Promise<void> {
    if (typeof G.__kilnGuiClipboardWrite === "function") {
      return G.__kilnGuiClipboardWrite(text);
    }
    const nativeMod = (typeof process !== "undefined" && (process as any).__kilnNativeModules?.gui) || null;
    if (typeof nativeMod?.writeText === "function") {
      return nativeMod.writeText(text);
    }
    if (!hostGuiBridge.clipboardWrite(text)) {
      virtualClipboard = String(text);
    }
  },
  async clear(): Promise<void> {
    if (typeof G.__kilnGuiClipboardClear === "function") {
      return G.__kilnGuiClipboardClear();
    }
    const nativeMod = (typeof process !== "undefined" && (process as any).__kilnNativeModules?.gui) || null;
    if (typeof nativeMod?.clear === "function") {
      return nativeMod.clear();
    }
    hostGuiBridge.clipboardClear();
    virtualClipboard = "";
  },
};

let currentTheme: "dark" | "light" = "light";
const themeListeners: ((theme: "dark" | "light") => void)[] = [];
export const systemTheme = {
  getTheme(): "dark" | "light" {
    if (typeof G.__kilnGuiGetTheme === "function") {
      return G.__kilnGuiGetTheme();
    }
    const nativeMod = (typeof process !== "undefined" && (process as any).__kilnNativeModules?.gui) || null;
    if (typeof nativeMod?.getTheme === "function") {
      return nativeMod.getTheme();
    }
    const hostTheme = hostGuiBridge.getTheme();
    if (hostTheme !== null) return hostTheme;
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

export const notification = {
  async show(options: NotificationOptions): Promise<void> {
    if (typeof G.__kilnGuiShowNotification === "function") {
      return G.__kilnGuiShowNotification(options);
    }
    const nativeMod = (typeof process !== "undefined" && (process as any).__kilnNativeModules?.gui) || null;
    if (typeof nativeMod?.showNotification === "function") {
      return nativeMod.showNotification(options);
    }
    hostGuiBridge.showNotification(options);
  },
};

export type AppPathName =
  | "home"
  | "appData"
  | "userData"
  | "cache"
  | "temp"
  | "desktop"
  | "documents"
  | "downloads"
  | "config";

export const app = {
  name: "kiln-app",
  version: "0.1.0",
  getPath(name: AppPathName): string {
    if (typeof G.__kilnGuiAppGetPath === "function") {
      return G.__kilnGuiAppGetPath(name);
    }
    const hostPath = hostGuiBridge.appGetPath(name);
    if (hostPath !== null) return hostPath;
    if (name === "temp") {
      return (typeof process !== "undefined" && (process.env?.TMPDIR || process.env?.TEMP || "/tmp")) || "/tmp";
    }
    if (name === "home") {
      return (typeof process !== "undefined" && process.env?.HOME) || "/";
    }
    return (typeof process !== "undefined" && process.env?.HOME ? `${process.env.HOME}/.${name}` : `/${name}`);
  },
  quit(exitCode = 0): void {
    if (typeof process !== "undefined" && typeof process.exit === "function") {
      process.exit(exitCode);
    }
  },
  requestSingleInstanceLock(appId = "kiln-app"): boolean {
    if (typeof G.__kilnGuiAppSingleInstanceLock === "function") {
      return G.__kilnGuiAppSingleInstanceLock(appId);
    }
    return hostGuiBridge.appSingleInstanceLock(appId);
  },
};

const registeredHotkeys = new Map<string, number>();
const hotkeyHandlers = new Map<number, () => void>();

export const globalShortcut = {
  register(accelerator: string, callback: () => void): boolean {
    if (typeof G.__kilnGuiHotkeyRegister === "function") {
      const id = G.__kilnGuiHotkeyRegister(accelerator);
      if (id > 0) {
        registeredHotkeys.set(accelerator, id);
        hotkeyHandlers.set(id, callback);
        return true;
      }
      return false;
    }
    const id = hostGuiBridge.hotkeyRegister(accelerator);
    if (id > 0) {
      registeredHotkeys.set(accelerator, id);
      hotkeyHandlers.set(id, callback);
      return true;
    }
    return false;
  },
  unregister(accelerator: string): void {
    const id = registeredHotkeys.get(accelerator);
    if (id !== undefined) {
      hostGuiBridge.hotkeyUnregister(id);
      registeredHotkeys.delete(accelerator);
      hotkeyHandlers.delete(id);
    }
  },
  unregisterAll(): void {
    for (const id of registeredHotkeys.values()) {
      hostGuiBridge.hotkeyUnregister(id);
    }
    registeredHotkeys.clear();
    hotkeyHandlers.clear();
  },
  poll(): boolean {
    const firedId = hostGuiBridge.hotkeyPoll();
    if (firedId > 0) {
      const handler = hotkeyHandlers.get(firedId);
      if (handler) {
        handler();
        return true;
      }
    }
    return false;
  },
};

export interface MenuItemOptions {
  id?: string;
  label: string;
  enabled?: boolean;
  checked?: boolean;
  accelerator?: string;
  onClick?: () => void;
}

export class MenuItem {
  readonly id: string;
  label: string;
  enabled: boolean;
  checked?: boolean;
  accelerator?: string;
  onClick?: () => void;

  constructor(options: MenuItemOptions) {
    this.id = options.id || String(nextNodeId++);
    this.label = options.label;
    this.enabled = options.enabled !== false;
    this.checked = options.checked;
    this.accelerator = options.accelerator;
    this.onClick = options.onClick;
  }
}

export class Menu {
  readonly id: number = nextNodeId++;
  private items: (MenuItem | Submenu | MenuSeparator)[] = [];

  append(item: MenuItem | Submenu | MenuSeparator): void {
    this.items.push(item);
  }

  appendSeparator(): void {
    this.items.push(new MenuSeparator());
  }

  appendSubmenu(submenu: Submenu): void {
    this.items.push(submenu);
  }

  getItems(): (MenuItem | Submenu | MenuSeparator)[] {
    return [...this.items];
  }
}

export class MenuSeparator extends MenuItem {
  constructor() {
    super({ label: "-" });
  }
}

export class Submenu extends Menu {
  label: string;

  constructor(label: string) {
    super();
    this.label = label;
  }
}

export interface TrayIconOptions {
  tooltip?: string;
  menu?: Menu;
  onPress?: () => void;
}

export class TrayIcon {
  readonly id: number;
  private _tooltip: string;
  private _menu: Menu | null;

  constructor(options: TrayIconOptions = {}) {
    this._tooltip = options.tooltip || "";
    this._menu = options.menu || null;
    const hostId = hostGuiBridge.createTray(this._tooltip);
    this.id = hostId !== null ? hostId : nextNodeId++;
  }

  get tooltip(): string {
    return this._tooltip;
  }

  set tooltip(value: string) {
    this._tooltip = value;
    hostGuiBridge.setTrayTooltip(this.id, value);
  }

  get menu(): Menu | null {
    return this._menu;
  }

  set menu(value: Menu | null) {
    this._menu = value;
  }

  dispose(): void {
    hostGuiBridge.destroyTray(this.id);
  }
}

// Turn Mailbox & Reentrancy-Safe Runloop Integration (ADR 0074 §4)
interface MailboxTurn {
  targetId: number;
  event: string;
  args: any[];
}

const mailboxQueue: MailboxTurn[] = [];
let isExecutingTurn = false;

export const turnMailbox = {
  pumpEvents(): boolean {
    const driver = getActiveDriver();
    const driverHad = driver ? driver.pumpEvents() : false;
    const shortcutHad = globalShortcut.poll();
    return driverHad || shortcutHad;
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

export function runLoop(win?: Window, intervalMs = 16): Promise<void> {
  return new Promise((resolve) => {
    let currentInterval = intervalMs;
    let timerId: any = null;

    const tick = () => {
      const hadEvents = turnMailbox.pumpEvents();
      if (win && !win.visible) {
        if (timerId !== null) clearInterval(timerId);
        resolve();
        return;
      }

      // Adaptive idle regulation (ADR 0074 §4):
      // If events occurred or turn queue was processed, keep fast 60fps pump (16ms).
      // If idle, back off to 48ms (~20fps) to minimize CPU usage, ramping back up instantly upon interaction.
      const targetInterval = hadEvents ? intervalMs : Math.min(intervalMs * 3, 50);
      if (targetInterval !== currentInterval) {
        currentInterval = targetInterval;
        clearInterval(timerId);
        timerId = setInterval(tick, currentInterval);
      }
    };

    timerId = setInterval(tick, currentInterval);
  });
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
  TrayIcon,
  Menu,
  MenuItem,
  MenuSeparator,
  Submenu,
  createRenderer,
  createElement,
  h,
  Fragment,
  app,
  globalShortcut,
  dialog,
  clipboard,
  systemTheme,
  notification,
  turnMailbox,
  nodeRegistry,
  createSignal,
  createEffect,
  createMemo,
  batch,
  untrack,
  onCleanup,
  runLoop,
  getActiveDriver,
};

