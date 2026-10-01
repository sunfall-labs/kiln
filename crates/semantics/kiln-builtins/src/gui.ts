// Virtual native GUI built-in module for Kiln (ADR 0074).
// Exposes cross-platform native UI widget primitives, an in-memory virtual tree
// for headless testability, and a fine-grained universal renderer seam for SolidJS 2.0.

const G = globalThis as any;
let nextNodeId = 1;
export const nodeRegistry = new Map<number, Node>();

export type ButtonVariant = "primary" | "secondary" | "destructive" | "flat" | "ghost";
export type ControlSize = "sm" | "md" | "lg";

export interface GtkStyleOptions {
  class?: string;
  css?: string;
}

export interface MacStyleOptions {
  bezelStyle?: "rounded" | "regularSquare" | "disclosure" | "inline" | "circular";
  tint?: string;
  keyEquivalent?: string;
}

export interface WinStyleOptions {
  variant?: "accent" | "standard";
  material?: "mica" | "acrylic";
  cornerPreference?: "rounded" | "square";
}

export interface PlatformStyles {
  gtk?: GtkStyleOptions;
  mac?: MacStyleOptions;
  win?: WinStyleOptions;
}

export interface ButtonOptions {
  label?: string;
  variant?: ButtonVariant;
  size?: ControlSize;
  disabled?: boolean;
  platform?: PlatformStyles;
  onPress?: () => void;
}

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
  pumpEvents(waitMs?: number): boolean;
  snapshotWindow?(win: Window, path: string): boolean;
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
  private nodeByHandle = new Map<bigint, number>();
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
        gdk_display_get_default: { arguments: [], return: "pointer" },
        gtk_css_provider_new: { arguments: [], return: "pointer" },
        gtk_css_provider_load_from_string: { arguments: ["pointer", "pointer"], return: "void" },
        gtk_style_context_add_provider_for_display: { arguments: ["pointer", "pointer", "u32"], return: "void" },
        gtk_widget_add_css_class: { arguments: ["pointer", "pointer"], return: "void" },
        gtk_widget_remove_css_class: { arguments: ["pointer", "pointer"], return: "void" },
        gtk_widget_set_sensitive: { arguments: ["pointer", "i32"], return: "void" },
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
      this.nodeByHandle.set(handle, win.id);
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
      this.nodeByHandle.delete(handle);
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

  addGlobalCss(css: string): boolean {
    if (!this.init() || !this.gtk) return false;
    try {
      const display = this.gtk.functions.gdk_display_get_default();
      if (!display) return false;
      const provider = this.gtk.functions.gtk_css_provider_new();
      this.gtk.functions.gtk_css_provider_load_from_string(provider, toCStr(this.ffi, css));
      this.gtk.functions.gtk_style_context_add_provider_for_display(display, provider, 600 /* GTK_STYLE_PROVIDER_PRIORITY_APPLICATION */);
      return true;
    } catch (_) {
      return false;
    }
  }

  applyWidgetStyling(node: Node, handle: bigint): void {
    if (!this.gtk) return;
    try {
      const ffi = this.ffi;
      const gtk = this.gtk;

      // Sensitivity / disabled
      const disabled = node.getProperty("disabled");
      if (disabled !== undefined && gtk.functions.gtk_widget_set_sensitive) {
        gtk.functions.gtk_widget_set_sensitive(handle, disabled ? 0 : 1);
      }

      // Semantic variant
      const variant = node.getProperty("variant");
      if (variant && gtk.functions.gtk_widget_add_css_class) {
        if (variant === "primary") {
          gtk.functions.gtk_widget_add_css_class(handle, toCStr(ffi, "suggested-action"));
        } else if (variant === "destructive") {
          gtk.functions.gtk_widget_add_css_class(handle, toCStr(ffi, "destructive-action"));
        } else if (variant === "flat" || variant === "ghost") {
          gtk.functions.gtk_widget_add_css_class(handle, toCStr(ffi, "flat"));
        }
      }

      // Platform GTK styles
      const platform = node.getProperty("platform") as PlatformStyles | undefined;
      if (platform?.gtk) {
        if (platform.gtk.class && gtk.functions.gtk_widget_add_css_class) {
          const classes = platform.gtk.class.split(/\s+/).filter(Boolean);
          for (let i = 0; i < classes.length; i++) {
            gtk.functions.gtk_widget_add_css_class(handle, toCStr(ffi, classes[i]));
          }
        }
        if (platform.gtk.css) {
          const className = `kiln-node-${node.id}`;
          if (gtk.functions.gtk_widget_add_css_class) {
            gtk.functions.gtk_widget_add_css_class(handle, toCStr(ffi, className));
          }
          this.addGlobalCss(`.${className} { ${platform.gtk.css} }`);
        }
      }
    } catch (_) {}
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
        this.nodeByHandle.set(handle, node.id);
        if (nodeCallbacks.length > 0) {
          this.callbacks.set(node.id, nodeCallbacks);
        }
        this.applyWidgetStyling(node, handle);
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

      if (name === "variant" || name === "disabled" || name === "platform") {
        const nodeId = this.nodeByHandle.get(handle);
        if (nodeId !== undefined) {
          const node = nodeRegistry.get(nodeId);
          if (node) {
            this.applyWidgetStyling(node, handle);
          }
        }
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
      this.nodeByHandle.delete(handle);
      this.handles.delete(node.id);
      this.callbacks.delete(node.id);
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
  private appkit: any = null;
  private ffi: any = null;
  private handles = new Map<number, bigint>();
  private nodeByHandle = new Map<bigint, number>();

  init(): boolean {
    if (this.initialized) return this.appkit !== null;
    this.initialized = true;
    if (typeof process === "undefined" || process.platform !== "darwin") return false;
    try {
      this.ffi = (process as any).__kilnNativeModules?.ffi || null;
      if (!this.ffi) return false;

      const fs = (process as any).__kilnNativeModules?.fs || null;
      const suffix = this.ffi.suffix || ".dylib";
      const cwd = typeof process.cwd === "function" ? process.cwd() : ".";
      const candidates = [
        process.env?.KILN_HOST_GUI_LIB,
        cwd + "/target/release/libkiln_host_gui" + suffix,
        cwd + "/target/debug/libkiln_host_gui" + suffix,
        "./target/release/libkiln_host_gui" + suffix,
        "./target/debug/libkiln_host_gui" + suffix,
      ].filter(Boolean) as string[];

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
      if (!targetLib && candidates.length > 0) targetLib = candidates[1] || candidates[0];
      if (!targetLib) return false;

      this.appkit = this.ffi.dlopen(targetLib, {
        kiln_gui_appkit_init: { arguments: [], return: "i32" },
        kiln_gui_appkit_window_create: { arguments: ["u64", "pointer", "f64", "f64"], return: "i32" },
        kiln_gui_appkit_window_show: { arguments: ["u64"], return: "i32" },
        kiln_gui_appkit_window_close: { arguments: ["u64"], return: "i32" },
        kiln_gui_appkit_window_set_title: { arguments: ["u64", "pointer"], return: "i32" },
        kiln_gui_appkit_window_set_content: { arguments: ["u64", "u64"], return: "i32" },
        kiln_gui_appkit_window_snapshot: { arguments: ["u64", "pointer"], return: "i32" },
        kiln_gui_appkit_widget_create: { arguments: ["u64", "pointer", "pointer"], return: "i32" },
        kiln_gui_appkit_widget_set_property: { arguments: ["u64", "pointer", "pointer", "f64"], return: "i32" },
        kiln_gui_appkit_widget_add_child: { arguments: ["u64", "u64"], return: "i32" },
        kiln_gui_appkit_widget_remove_child: { arguments: ["u64", "u64"], return: "i32" },
        kiln_gui_appkit_widget_destroy: { arguments: ["u64"], return: "i32" },
        kiln_gui_appkit_pump_events: { arguments: ["f64"], return: "i32" },
        kiln_gui_appkit_poll_event: { arguments: ["pointer", "pointer", "u64", "pointer", "u64"], return: "i32" },
      });

      if (
        this.appkit &&
        this.appkit.functions &&
        this.appkit.functions.kiln_gui_appkit_init &&
        this.appkit.functions.kiln_gui_appkit_init() === 0
      ) {
        return true;
      }
    } catch (_) {}
    this.appkit = null;
    return false;
  }

  getHandle(nodeId: number): bigint | null {
    return this.handles.get(nodeId) || null;
  }

  createWindow(win: Window): bigint | null {
    if (!this.appkit) return null;
    let handle = this.handles.get(win.id);
    if (handle) return handle;

    try {
      const res = this.appkit.functions.kiln_gui_appkit_window_create(
        BigInt(win.id),
        toCStr(this.ffi, win.title || "Kiln Window"),
        Number(win.width || 500),
        Number(win.height || 320)
      );
      if (res === 0) {
        handle = BigInt(win.id);
        this.handles.set(win.id, handle);
        this.nodeByHandle.set(handle, win.id);
        return handle;
      }
    } catch (_) {}
    return null;
  }

  setWindowContent(winHandle: bigint, childHandle: bigint): void {
    if (!this.appkit) return;
    try {
      this.appkit.functions.kiln_gui_appkit_window_set_content(winHandle, childHandle);
    } catch (_) {}
  }

  showWindow(win: Window): void {
    win.visible = true;
    if (!this.appkit) return;
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
      this.appkit.functions.kiln_gui_appkit_window_show(BigInt(win.id));
      this.pumpEvents();
    }
  }

  closeWindow(win: Window): void {
    win.visible = false;
    if (!this.appkit) return;
    const handle = this.handles.get(win.id);
    if (handle) {
      try {
        this.appkit.functions.kiln_gui_appkit_window_close(BigInt(win.id));
      } catch (_) {}
      this.handles.delete(win.id);
      this.nodeByHandle.delete(handle);
    }
  }

  snapshotWindow(win: Window, path: string): boolean {
    if (!this.appkit) return false;
    const handle = this.handles.get(win.id);
    if (!handle) return false;
    try {
      if (this.appkit.functions.kiln_gui_appkit_window_snapshot) {
        return this.appkit.functions.kiln_gui_appkit_window_snapshot(handle, toCStr(this.ffi, path)) === 0;
      }
    } catch (_) {}
    return false;
  }

  createWidget(node: Node): bigint | null {
    if (!this.appkit) return null;
    let handle = this.handles.get(node.id);
    if (handle) return handle;

    try {
      let text = (node as any).text || "";
      if (node.tag === "box" || node.tag === "grid") {
        text = JSON.stringify(node.properties);
      } else if (node.tag === "button") {
        text = String(node.getProperty("label") || text);
      } else if (node.tag === "text") {
        text = String(node.getProperty("text") || text);
      } else if (node.tag === "textinput") {
        text = String(node.getProperty("value") || text);
      } else if (node.tag === "segmented") {
        text = String(node.getProperty("items") || text);
      } else if (node.tag === "select" || node.tag === "dropdown") {
        text = String(node.getProperty("options") || text);
      } else if (node.tag === "checkbox") {
        text = String(node.getProperty("label") || text);
      } else if (node.tag === "icon" || node.tag === "image") {
        text = String(node.getProperty("name") || node.getProperty("symbol") || text);
      }

      const res = this.appkit.functions.kiln_gui_appkit_widget_create(
        BigInt(node.id),
        toCStr(this.ffi, node.tag),
        toCStr(this.ffi, text)
      );
      if (res === 0) {
        handle = BigInt(node.id);
        this.handles.set(node.id, handle);
        this.nodeByHandle.set(handle, node.id);

        if (node.tag === "slider") {
          const val = node.getProperty("value");
          if (val !== undefined) this.updateProperty(handle, node.tag, "value", val);
          const min = node.getProperty("min");
          if (min !== undefined) this.updateProperty(handle, node.tag, "min", min);
          const max = node.getProperty("max");
          if (max !== undefined) this.updateProperty(handle, node.tag, "max", max);
        } else if (node.tag === "toggle" || node.tag === "checkbox") {
          const chk = node.getProperty("checked");
          if (chk !== undefined) this.updateProperty(handle, node.tag, "checked", chk);
        } else if (node.tag === "text") {
          const variant = node.getProperty("variant");
          if (variant !== undefined) this.updateProperty(handle, node.tag, "variant", variant);
          const color = node.getProperty("color");
          if (color !== undefined) this.updateProperty(handle, node.tag, "color", color);
          const size = node.getProperty("size");
          if (size !== undefined) this.updateProperty(handle, node.tag, "size", size);
        } else if (node.tag === "button") {
          const variant = node.getProperty("variant");
          if (variant !== undefined) this.updateProperty(handle, node.tag, "variant", variant);
          const icon = node.getProperty("icon");
          if (icon !== undefined) this.updateProperty(handle, node.tag, "icon", icon);
        } else if (node.tag === "textinput") {
          const placeholder = node.getProperty("placeholder");
          if (placeholder !== undefined) this.updateProperty(handle, node.tag, "placeholder", placeholder);
        } else if (node.tag === "segmented") {
          const items = node.getProperty("items");
          if (items !== undefined) this.updateProperty(handle, node.tag, "items", items);
          const idx = node.getProperty("selectedIndex");
          if (idx !== undefined) this.updateProperty(handle, node.tag, "selectedIndex", idx);
        } else if (node.tag === "select" || node.tag === "dropdown") {
          const opts = node.getProperty("options");
          if (opts !== undefined) this.updateProperty(handle, node.tag, "options", opts);
          const idx = node.getProperty("selectedIndex");
          if (idx !== undefined) this.updateProperty(handle, node.tag, "selectedIndex", idx);
        } else if (node.tag === "progressbar" || node.tag === "progress") {
          const val = node.getProperty("value");
          if (val !== undefined) this.updateProperty(handle, node.tag, "value", val);
          const max = node.getProperty("max");
          if (max !== undefined) this.updateProperty(handle, node.tag, "max", max);
          const min = node.getProperty("min");
          if (min !== undefined) this.updateProperty(handle, node.tag, "min", min);
          const ind = node.getProperty("indeterminate");
          if (ind !== undefined) this.updateProperty(handle, node.tag, "indeterminate", ind);
        }

        if (node.tag === "grid" || node.tag === "box") {
          const style = node.getProperty("style");
          if (style !== undefined) this.updateProperty(handle, node.tag, "style", style);
          const columns = node.getProperty("columns");
          if (columns !== undefined) this.updateProperty(handle, node.tag, "columns", columns);
          const rows = node.getProperty("rows");
          if (rows !== undefined) this.updateProperty(handle, node.tag, "rows", rows);
          const gap = node.getProperty("gap");
          if (gap !== undefined) this.updateProperty(handle, node.tag, "gap", gap);
        }

        const isContainer =
          node.tag === "vstack" ||
          node.tag === "hstack" ||
          node.tag === "card" ||
          node.tag === "box" ||
          node.tag === "grid" ||
          node.tag === "scroll" ||
          node.tag === "scrollview";

        if (isContainer) {
          const spacing = node.getProperty("spacing");
          if (spacing !== undefined) this.updateProperty(handle, node.tag, "spacing", spacing);
          const alignment = node.getProperty("alignment");
          if (alignment !== undefined) this.updateProperty(handle, node.tag, "alignment", alignment);
          const padding = node.getProperty("padding");
          if (padding !== undefined) this.updateProperty(handle, node.tag, "padding", padding);
          for (let i = 0; i < node.children.length; i++) {
            const childHandle = this.createWidget(node.children[i]);
            if (childHandle) {
              this.appkit.functions.kiln_gui_appkit_widget_add_child(handle, childHandle);
            }
          }
        }

        const width = node.getProperty("width");
        if (width !== undefined) this.updateProperty(handle, node.tag, "width", width);
        const height = node.getProperty("height");
        if (height !== undefined) this.updateProperty(handle, node.tag, "height", height);
        return handle;
      }
    } catch (_) {}
    return null;
  }

  updateProperty(handle: bigint, tag: string, name: string, value: any): void {
    if (!this.appkit) return;
    try {
      if (tag === "window" && name === "title") {
        this.appkit.functions.kiln_gui_appkit_window_set_title(handle, toCStr(this.ffi, String(value)));
      } else {
        const valStr =
          typeof value === "object" && value !== null
            ? JSON.stringify(value)
            : typeof value === "string"
            ? value
            : String(value ?? "");
        const valNum = typeof value === "number" ? value : (typeof value === "boolean" ? (value ? 1 : 0) : 0);
        this.appkit.functions.kiln_gui_appkit_widget_set_property(
          handle,
          toCStr(this.ffi, name),
          toCStr(this.ffi, valStr),
          valNum
        );
      }
    } catch (_) {}
  }

  insertChild(parentHandle: bigint, childHandle: bigint, anchorHandle: bigint | null): void {
    if (!this.appkit) return;
    try {
      this.appkit.functions.kiln_gui_appkit_widget_add_child(parentHandle, childHandle);
    } catch (_) {}
  }

  removeChild(parentHandle: bigint, childHandle: bigint): void {
    if (!this.appkit) return;
    try {
      this.appkit.functions.kiln_gui_appkit_widget_remove_child(parentHandle, childHandle);
    } catch (_) {}
  }

  reorderChild(parentHandle: bigint, childHandle: bigint, anchorHandle: bigint | null): void {
    if (!this.appkit) return;
    try {
      this.appkit.functions.kiln_gui_appkit_widget_remove_child(parentHandle, childHandle);
      this.appkit.functions.kiln_gui_appkit_widget_add_child(parentHandle, childHandle);
    } catch (_) {}
  }

  disposeWidget(node: Node): void {
    const handle = this.handles.get(node.id);
    if (handle) {
      if (node.tag === "window" && this.appkit) {
        try {
          this.appkit.functions.kiln_gui_appkit_window_close(handle);
        } catch (_) {}
      } else if (this.appkit && this.appkit.functions.kiln_gui_appkit_widget_destroy) {
        try {
          this.appkit.functions.kiln_gui_appkit_widget_destroy(handle);
        } catch (_) {}
      }
      this.nodeByHandle.delete(handle);
      this.handles.delete(node.id);
    }
  }

  pumpEvents(waitMs = 0): boolean {
    if (!this.appkit) return false;
    let hadEvents = false;
    try {
      if (this.appkit.functions.kiln_gui_appkit_pump_events(waitMs) !== 0) {
        hadEvents = true;
      }
      const outIdBuf = new BigUint64Array(1);
      const outIdPtr = this.ffi.getRawPointer(outIdBuf.buffer);
      const outTypeBuf = new Uint8Array(64);
      const outTypePtr = this.ffi.getRawPointer(outTypeBuf.buffer);
      const outValBuf = new Uint8Array(256);
      const outValPtr = this.ffi.getRawPointer(outValBuf.buffer);

      while (
        this.appkit.functions.kiln_gui_appkit_poll_event(
          outIdPtr,
          outTypePtr,
          64n,
          outValPtr,
          256n
        ) === 1
      ) {
        hadEvents = true;
        const targetId = Number(outIdBuf[0]);
        let typeLen = 0;
        while (typeLen < outTypeBuf.length && outTypeBuf[typeLen] !== 0) typeLen++;
        const eventType = new TextDecoder().decode(outTypeBuf.subarray(0, typeLen));

        let valLen = 0;
        while (valLen < outValBuf.length && outValBuf[valLen] !== 0) valLen++;
        const eventVal = new TextDecoder().decode(outValBuf.subarray(0, valLen));

        if (targetId > 0 && eventType) {
          if (eventType === "close") {
            const winNode = nodeRegistry.get(targetId);
            if (winNode && winNode.tag === "window") {
              (winNode as any).visible = false;
            }
            turnMailbox.post(targetId, eventType);
          } else if (eventType === "change") {
            const num = Number(eventVal);
            turnMailbox.post(targetId, eventType, isNaN(num) ? eventVal : num);
          } else if (eventType === "input") {
            turnMailbox.post(targetId, eventType, eventVal);
          } else if (eventType === "toggle") {
            turnMailbox.post(targetId, eventType, eventVal === "true");
          } else {
            turnMailbox.post(targetId, eventType);
          }
        }
      }
    } catch (_) {}
    return hadEvents;
  }
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

// ─────────────────────────────────────────────────────────────
// Fine-grained Reactive System (SolidJS 2.0 Async-First Core)
// ─────────────────────────────────────────────────────────────

interface Computation {
  id: number;
  fn: () => any;
  apply?: (val: any) => void | (() => void);
  cleanups: (() => void)[];
  sources: Set<Signal<any> | Memo<any>>;
  stale: boolean;
  pending: boolean;
  epoch: number;
  disposed: boolean;
}

let nextCompId = 1;
let currentListener: Computation | null = null;
let batchDepth = 0;
let isFlushPending = false;
const pendingComputations: Set<Computation> = new Set();

export function untrack<T>(fn: () => T): T {
  const prev = currentListener;
  currentListener = null;
  try {
    return fn();
  } finally {
    currentListener = prev;
  }
}

function scheduleFlush(): void {
  if (batchDepth > 0) return;
  if (!isFlushPending) {
    isFlushPending = true;
    if (typeof queueMicrotask === "function") {
      queueMicrotask(flush);
    } else {
      Promise.resolve().then(flush);
    }
  }
}

export function flush(): void {
  isFlushPending = false;
  while (pendingComputations.size > 0) {
    const list = Array.from(pendingComputations);
    pendingComputations.clear();
    for (let i = 0; i < list.length; i++) {
      const c = list[i];
      if (c.stale && !c.disposed) {
        runComputation(c);
      }
    }
  }
}

class Signal<T> {
  value: T;
  subscribers: Set<Computation> = new Set();

  constructor(value: T) {
    this.value = value;
  }

  read(): T {
    if (currentListener && !currentListener.disposed) {
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
        subs[i].stale = true;
        pendingComputations.add(subs[i]);
      }
      scheduleFlush();
    }
  }
}

class Memo<T> {
  computation: Computation;
  value: T;
  subscribers: Set<Computation> = new Set();

  constructor(fn: () => T | Promise<T>) {
    this.value = undefined as any;
    this.computation = {
      id: nextCompId++,
      fn,
      cleanups: [],
      sources: new Set(),
      stale: true,
      pending: false,
      epoch: 0,
      disposed: false,
    };
    runComputation(this.computation, (res) => {
      this.value = res;
    });
  }

  read(): T {
    if (currentListener && !currentListener.disposed) {
      this.subscribers.add(currentListener);
      currentListener.sources.add(this);
    }
    if (this.computation.stale) {
      runComputation(this.computation, (res) => {
        this.value = res;
      });
    }
    return this.value;
  }

  notifySubscribers(): void {
    const subs = Array.from(this.subscribers);
    for (let i = 0; i < subs.length; i++) {
      subs[i].stale = true;
      pendingComputations.add(subs[i]);
    }
    scheduleFlush();
  }
}

function runComputation(c: Computation, onResult?: (val: any) => void): void {
  if (c.disposed) return;

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

  const currentEpoch = ++c.epoch;
  const prev = currentListener;
  currentListener = c;
  let raw: any;
  try {
    raw = c.fn();
  } finally {
    currentListener = prev;
  }

  // Handle Promise (Solid 2.0 First-Class Async)
  if (raw && typeof raw.then === "function") {
    c.pending = true;
    raw.then(
      (resolved: any) => {
        if (c.disposed || c.epoch !== currentEpoch) return;
        c.pending = false;
        if (c.apply) {
          const cleanup = c.apply(resolved);
          if (typeof cleanup === "function") c.cleanups.push(cleanup);
        } else if (typeof resolved === "function") {
          c.cleanups.push(resolved);
        }
        if (onResult) onResult(resolved);
      },
      (err: any) => {
        if (c.disposed || c.epoch !== currentEpoch) return;
        c.pending = false;
        console.error("Async computation rejected:", err);
      }
    );
  } else {
    c.pending = false;
    if (c.apply) {
      const cleanup = c.apply(raw);
      if (typeof cleanup === "function") c.cleanups.push(cleanup);
    } else if (typeof raw === "function") {
      c.cleanups.push(raw);
    }
    if (onResult) onResult(raw);
  }
}

export function createSignal<T>(initialValue: T): [() => T, (value: T | ((prev: T) => T)) => void] {
  const s = new Signal(initialValue);
  return [() => s.read(), (v) => s.write(v)];
}

export function createMemo<T>(fn: () => T | Promise<T>): () => T {
  const m = new Memo<T>(fn);
  m.computation.apply = (val) => {
    m.value = val;
    m.notifySubscribers();
  };
  return () => m.read();
}

export function isPending(accessor: () => any): boolean {
  let pending = false;
  const prev = currentListener;
  const tempComp: Computation = {
    id: nextCompId++,
    fn: accessor,
    cleanups: [],
    sources: new Set(),
    stale: false,
    pending: false,
    epoch: 0,
    disposed: false,
  };
  currentListener = tempComp;
  try {
    accessor();
    for (const src of tempComp.sources) {
      if (src instanceof Memo && src.computation.pending) {
        pending = true;
        break;
      }
    }
  } finally {
    currentListener = prev;
    const srcs = Array.from(tempComp.sources);
    for (let i = 0; i < srcs.length; i++) {
      srcs[i].subscribers.delete(tempComp);
    }
  }
  return pending;
}

export function latest<T>(accessor: () => T): T {
  flush();
  return accessor();
}

export function createEffect<T>(
  depsOrFn: () => T | Promise<T>,
  apply?: (value: T) => void | (() => void)
): () => void {
  const isSplit = typeof apply === "function";
  const c: Computation = {
    id: nextCompId++,
    fn: depsOrFn,
    apply: isSplit ? apply : undefined,
    cleanups: [],
    sources: new Set(),
    stale: false,
    pending: false,
    epoch: 0,
    disposed: false,
  };

  runComputation(c);

  return () => {
    c.disposed = true;
    for (let i = 0; i < c.cleanups.length; i++) {
      c.cleanups[i]();
    }
    c.cleanups = [];
    const srcs = Array.from(c.sources);
    for (let i = 0; i < srcs.length; i++) {
      srcs[i].subscribers.delete(c);
    }
    c.sources.clear();
    pendingComputations.delete(c);
  };
}

export function batch<T>(fn: () => T): T {
  batchDepth++;
  try {
    return fn();
  } finally {
    batchDepth--;
    if (batchDepth === 0) {
      flush();
    }
  }
}

export function onCleanup(fn: () => void): void {
  if (currentListener) {
    currentListener.cleanups.push(fn);
  }
}

export class Node {
  readonly id: number;
  readonly tag: string;
  parent: Node | null = null;
  children: Node[] = [];
  properties: Record<string, any> = {};
  private listeners: Record<string, Function[]> = {};
  private propertyCleanups: Map<string, () => void> = new Map();

  constructor(tag: string) {
    this.id = nextNodeId++;
    this.tag = tag;
    nodeRegistry.set(this.id, this);
  }

  dispose(): void {
    for (const cleanup of this.propertyCleanups.values()) {
      cleanup();
    }
    this.propertyCleanups.clear();
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
    const existingCleanup = this.propertyCleanups.get(name);
    if (existingCleanup) {
      existingCleanup();
      this.propertyCleanups.delete(name);
    }

    if (name.startsWith("on") && typeof value === "function") {
      let eventName = name.slice(2).toLowerCase();
      if (eventName === "press") eventName = "click";
      this.addEventListener(eventName, value);
      if (eventName === "change") {
        this.addEventListener("input", value);
      }
      return;
    }

    // Solid 2.0: If value is a reactive signal or accessor function, bind reactively
    if (typeof value === "function") {
      const stop = createEffect(value, (resolvedVal: any) => {
        this.applyRawProperty(name, resolvedVal);
      });
      if (stop) {
        this.propertyCleanups.set(name, stop);
      }
      return;
    }

    this.applyRawProperty(name, value);
  }

  private applyRawProperty(name: string, value: any): void {
    this.properties[name] = value;
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

  on(event: string, handler: Function): void {
    this.addEventListener(event, handler);
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

  off(event: string, handler: Function): void {
    this.removeEventListener(event, handler);
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

  get style(): any {
    return this.properties.style;
  }
  set style(val: any) {
    this.setProperty("style", val);
  }

  private _content: Node | null = null;

  get content(): Node | null {
    if (this._content) return this._content;
    if (this.children.length === 0) return null;
    if (this.children.length === 1 && !this.properties.style) {
      return this.children[0];
    }
    const root = new VStack({
      spacing: (this.properties.style && this.properties.style.gap !== undefined) ? this.properties.style.gap : 14,
    });
    const childList = [...this.children];
    for (let i = 0; i < childList.length; i++) {
      root.appendChild(childList[i]);
    }
    this._content = root;
    return root;
  }
  set content(node: Node | null) {
    this._content = node;
    while (this.children.length > 0) {
      this.removeChild(this.children[0]);
    }
    if (node) {
      super.appendChild(node);
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

  override appendChild(child: Node): void {
    if (this._content && this._content !== child) {
      this._content.appendChild(child);
      return;
    }
    super.appendChild(child);
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

  snapshot(path: string): boolean {
    const driver = getActiveDriver();
    if (typeof (driver as any).snapshotWindow === "function") {
      return (driver as any).snapshotWindow(this, path);
    }
    return false;
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
  constructor(options?: { spacing?: number; alignment?: string; padding?: number; width?: number; height?: number; children?: Node[] }) {
    super("vstack");
    if (options) {
      if (options.spacing !== undefined) this.setProperty("spacing", options.spacing);
      if (options.alignment !== undefined) this.setProperty("alignment", options.alignment);
      if (options.padding !== undefined) this.setProperty("padding", options.padding);
      if (options.width !== undefined) this.setProperty("width", options.width);
      if (options.height !== undefined) this.setProperty("height", options.height);
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
  constructor(options?: { spacing?: number; alignment?: string; padding?: number; width?: number; height?: number; children?: Node[] }) {
    super("hstack");
    if (options) {
      if (options.spacing !== undefined) this.setProperty("spacing", options.spacing);
      if (options.alignment !== undefined) this.setProperty("alignment", options.alignment);
      if (options.padding !== undefined) this.setProperty("padding", options.padding);
      if (options.width !== undefined) this.setProperty("width", options.width);
      if (options.height !== undefined) this.setProperty("height", options.height);
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

export class Card extends Node {
  private _inner: VStack | null = null;

  constructor(options?: { width?: number; height?: number; child?: Node; children?: Node[] }) {
    super("card");
    if (options) {
      if (options.width !== undefined) this.setProperty("width", options.width);
      if (options.height !== undefined) this.setProperty("height", options.height);
      if (options.child) {
        this.appendChild(options.child);
      } else if (options.children) {
        for (let i = 0; i < options.children.length; i++) {
          this.appendChild(options.children[i]);
        }
      }
    }
  }

  override appendChild(child: Node): void {
    if (!this._inner) {
      this._inner = new VStack({ spacing: 6 });
      super.appendChild(this._inner);
    }
    this._inner.appendChild(child);
  }
}

export class Box extends Node {
  constructor(options?: { style?: any; width?: number; height?: number; children?: Node[] } | Node[]) {
    super("box");
    if (Array.isArray(options)) {
      for (let i = 0; i < options.length; i++) {
        this.appendChild(options[i]);
      }
    } else if (options) {
      if (options.style !== undefined) this.setProperty("style", options.style);
      if (options.width !== undefined) this.setProperty("width", options.width);
      if (options.height !== undefined) this.setProperty("height", options.height);
      if (options.children) {
        for (let i = 0; i < options.children.length; i++) {
          this.appendChild(options.children[i]);
        }
      }
    }
  }

  get style(): any {
    return this.properties.style;
  }
  set style(val: any) {
    this.setProperty("style", val);
  }
}

export class Grid extends Node {
  constructor(options?: { columns?: string; rows?: string; gap?: number | string; style?: any; width?: number; height?: number; children?: Node[] } | Node[]) {
    super("grid");
    if (Array.isArray(options)) {
      for (let i = 0; i < options.length; i++) {
        this.appendChild(options[i]);
      }
    } else if (options) {
      if (options.columns !== undefined) this.setProperty("columns", options.columns);
      if (options.rows !== undefined) this.setProperty("rows", options.rows);
      if (options.gap !== undefined) this.setProperty("gap", options.gap);
      if (options.style !== undefined) this.setProperty("style", options.style);
      if (options.width !== undefined) this.setProperty("width", options.width);
      if (options.height !== undefined) this.setProperty("height", options.height);
      if (options.children) {
        for (let i = 0; i < options.children.length; i++) {
          this.appendChild(options.children[i]);
        }
      }
    }
  }

  get columns(): string {
    return this.properties.columns || "";
  }
  set columns(val: string) {
    this.setProperty("columns", val);
  }

  get rows(): string {
    return this.properties.rows || "";
  }
  set rows(val: string) {
    this.setProperty("rows", val);
  }

  get gap(): any {
    return this.properties.gap;
  }
  set gap(val: any) {
    this.setProperty("gap", val);
  }

  get style(): any {
    return this.properties.style;
  }
  set style(val: any) {
    this.setProperty("style", val);
  }
}

export class Divider extends Node {
  constructor(options?: { width?: number }) {
    super("divider");
    if (options && options.width !== undefined) {
      this.setProperty("width", options.width);
    }
  }
}

export class Text extends Node {
  constructor(textOrOptions?: string | (() => string | Promise<string>) | { text?: string | (() => string | Promise<string>); variant?: string; color?: string | (() => string); size?: number; bold?: boolean; width?: number; height?: number }) {
    super("text");
    if (typeof textOrOptions === "string" || typeof textOrOptions === "function") {
      this.text = textOrOptions;
    } else if (textOrOptions) {
      if (textOrOptions.text !== undefined) this.text = textOrOptions.text;
      if (textOrOptions.variant !== undefined) this.setProperty("variant", textOrOptions.variant);
      if (textOrOptions.color !== undefined) this.setProperty("color", textOrOptions.color);
      if (textOrOptions.size !== undefined) this.setProperty("size", textOrOptions.size);
      if (textOrOptions.bold !== undefined) this.setProperty("bold", textOrOptions.bold);
      if (textOrOptions.width !== undefined) this.setProperty("width", textOrOptions.width);
      if (textOrOptions.height !== undefined) this.setProperty("height", textOrOptions.height);
    }
  }

  get text(): string {
    return this.properties.text || "";
  }
  set text(val: string | (() => string | Promise<string>)) {
    this.setProperty("text", val);
  }
}

export class Button extends Node {
  constructor(labelOrOptions?: string | (() => string) | (ButtonOptions & { width?: number; height?: number; icon?: string; disabled?: boolean | (() => boolean) })) {
    super("button");
    if (typeof labelOrOptions === "string" || typeof labelOrOptions === "function") {
      this.label = labelOrOptions;
    } else if (labelOrOptions) {
      if (labelOrOptions.label !== undefined) this.label = labelOrOptions.label;
      if (labelOrOptions.variant !== undefined) this.variant = labelOrOptions.variant;
      if (labelOrOptions.size !== undefined) this.size = labelOrOptions.size;
      if (labelOrOptions.disabled !== undefined) this.disabled = labelOrOptions.disabled;
      if (labelOrOptions.platform !== undefined) this.platform = labelOrOptions.platform;
      if ((labelOrOptions as any).icon !== undefined) this.setProperty("icon", (labelOrOptions as any).icon);
      if ((labelOrOptions as any).width !== undefined) this.setProperty("width", (labelOrOptions as any).width);
      if ((labelOrOptions as any).height !== undefined) this.setProperty("height", (labelOrOptions as any).height);
      if (labelOrOptions.onPress) this.onPress = labelOrOptions.onPress;
    }
  }

  get label(): string {
    return this.properties.label || "";
  }
  set label(val: string | (() => string)) {
    this.setProperty("label", val);
  }

  get variant(): ButtonVariant | undefined {
    return this.properties.variant;
  }
  set variant(val: ButtonVariant | undefined) {
    this.setProperty("variant", val);
  }

  get size(): ControlSize | undefined {
    return this.properties.size;
  }
  set size(val: ControlSize | undefined) {
    this.setProperty("size", val);
  }

  get disabled(): boolean {
    return !!this.properties.disabled;
  }
  set disabled(val: boolean) {
    this.setProperty("disabled", val);
  }

  get platform(): PlatformStyles | undefined {
    return this.properties.platform;
  }
  set platform(val: PlatformStyles | undefined) {
    this.setProperty("platform", val);
  }

  get onPress(): (() => void) | null {
    return this.properties.onPress || null;
  }
  set onPress(handler: (() => void) | null) {
    this.setProperty("onPress", handler);
  }

  click(): void {
    if (!this.disabled) {
      this.dispatchEvent("click");
    }
  }
}

export class TextInput extends Node {
  constructor(options?: { placeholder?: string; value?: string; width?: number; height?: number; onChange?: (val: string) => void; onInput?: (val: string) => void }) {
    super("textinput");
    if (options) {
      if (options.placeholder !== undefined) this.setProperty("placeholder", options.placeholder);
      if (options.value !== undefined) this.value = options.value;
      if (options.width !== undefined) this.setProperty("width", options.width);
      if (options.height !== undefined) this.setProperty("height", options.height);
      if (options.onChange) this.setProperty("onChange", options.onChange);
      if (options.onInput) this.setProperty("onInput", options.onInput);
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
  constructor(options?: { min?: number | (() => number); max?: number | (() => number); value?: number | (() => number); width?: number; height?: number; onChange?: (val: number) => void; onInput?: (val: number) => void }) {
    super("slider");
    if (options) {
      if (options.min !== undefined) this.setProperty("min", options.min);
      if (options.max !== undefined) this.setProperty("max", options.max);
      if (options.value !== undefined) this.value = options.value;
      if (options.width !== undefined) this.setProperty("width", options.width);
      if (options.height !== undefined) this.setProperty("height", options.height);
      if (options.onChange) this.setProperty("onChange", options.onChange);
      if (options.onInput) this.setProperty("onInput", options.onInput);
    }
  }

  get value(): number {
    return this.properties.value !== undefined ? this.properties.value : 0;
  }
  set value(val: number | (() => number)) {
    this.setProperty("value", val);
  }

  get min(): number {
    return this.properties.min !== undefined ? this.properties.min : 0;
  }
  set min(val: number | (() => number)) {
    this.setProperty("min", val);
  }

  get max(): number {
    return this.properties.max !== undefined ? this.properties.max : 100;
  }
  set max(val: number | (() => number)) {
    this.setProperty("max", val);
  }
}

export class Toggle extends Node {
  constructor(options?: { checked?: boolean | (() => boolean); width?: number; height?: number; onToggle?: (checked: boolean) => void }) {
    super("toggle");
    if (options) {
      if (options.checked !== undefined) this.checked = options.checked;
      if (options.width !== undefined) this.setProperty("width", options.width);
      if (options.height !== undefined) this.setProperty("height", options.height);
      if (options.onToggle) this.setProperty("onToggle", options.onToggle);
    }
  }

  get checked(): boolean {
    return this.properties.checked === true || this.properties.checked === "true" || this.properties.checked === 1;
  }
  set checked(val: boolean | (() => boolean)) {
    this.setProperty("checked", typeof val === "function" ? val : Boolean(val));
  }
}

export class Spacer extends Node {
  constructor(options?: { minWidth?: number; minHeight?: number }) {
    super("spacer");
    if (options) {
      if (options.minWidth !== undefined) this.setProperty("width", options.minWidth);
      if (options.minHeight !== undefined) this.setProperty("height", options.minHeight);
    }
  }
}

export class ScrollView extends Node {
  constructor(options?: { width?: number; height?: number; child?: Node; children?: Node[] }) {
    super("scroll");
    if (options) {
      if (options.width !== undefined) this.setProperty("width", options.width);
      if (options.height !== undefined) this.setProperty("height", options.height);
      if (options.child) {
        this.appendChild(options.child);
      } else if (options.children) {
        const inner = new VStack({ spacing: 12, children: options.children });
        this.appendChild(inner);
      }
    }
  }
}

export class SegmentedControl extends Node {
  constructor(options?: { items: string[]; selectedIndex?: number; width?: number; onChange?: (index: number, label: string) => void }) {
    super("segmented");
    if (options) {
      if (options.items) {
        this.text = JSON.stringify(options.items);
        this.setProperty("items", JSON.stringify(options.items));
      }
      if (options.selectedIndex !== undefined) this.setProperty("selectedIndex", options.selectedIndex);
      if (options.width !== undefined) this.setProperty("width", options.width);
      if (options.onChange) {
        this.on("change", (e: any) => {
          const raw = typeof e === "string" ? e : (e?.value ?? "0");
          const idx = parseInt(raw, 10);
          const label = options.items[idx] ?? "";
          options.onChange!(idx, label);
        });
      }
    }
  }

  get selectedIndex(): number {
    return this.properties.selectedIndex !== undefined ? this.properties.selectedIndex : 0;
  }
  set selectedIndex(val: number) {
    this.setProperty("selectedIndex", val);
  }
}

export class ProgressBar extends Node {
  constructor(options?: { value?: number | (() => number); max?: number | (() => number); min?: number | (() => number); indeterminate?: boolean | (() => boolean); width?: number; height?: number }) {
    super("progressbar");
    if (options) {
      if (options.value !== undefined) this.value = options.value;
      if (options.max !== undefined) this.max = options.max;
      if (options.min !== undefined) this.setProperty("min", options.min);
      if (options.indeterminate !== undefined) this.setProperty("indeterminate", options.indeterminate);
      if (options.width !== undefined) this.setProperty("width", options.width);
      if (options.height !== undefined) this.setProperty("height", options.height);
    }
  }

  get value(): number {
    return this.properties.value !== undefined ? this.properties.value : 0;
  }
  set value(val: number | (() => number)) {
    this.setProperty("value", val);
  }

  setValue(val: number | (() => number)): void {
    this.value = val;
  }

  get max(): number {
    return this.properties.max !== undefined ? this.properties.max : 100;
  }
  set max(val: number | (() => number)) {
    this.setProperty("max", val);
  }
}

export class Select extends Node {
  constructor(options?: { options: string[]; selectedIndex?: number; width?: number; onChange?: (index: number, value: string) => void }) {
    super("select");
    if (options) {
      if (options.options) {
        this.text = JSON.stringify(options.options);
        this.setProperty("options", JSON.stringify(options.options));
      }
      if (options.selectedIndex !== undefined) this.setProperty("selectedIndex", options.selectedIndex);
      if (options.width !== undefined) this.setProperty("width", options.width);
      if (options.onChange) {
        this.on("change", (e: any) => {
          const raw = typeof e === "string" ? e : (e?.value ?? "0:");
          const colon = raw.indexOf(":");
          let idx = 0;
          let val = raw;
          if (colon !== -1) {
            idx = parseInt(raw.slice(0, colon), 10);
            val = raw.slice(colon + 1);
          }
          options.onChange!(idx, val);
        });
      }
    }
  }

  get selectedIndex(): number {
    return this.properties.selectedIndex !== undefined ? this.properties.selectedIndex : 0;
  }
  set selectedIndex(val: number) {
    this.setProperty("selectedIndex", val);
  }
}

export class Checkbox extends Node {
  constructor(labelOrOptions?: string | (() => string) | { label?: string | (() => string); checked?: boolean | (() => boolean); width?: number; onToggle?: (checked: boolean) => void }) {
    super("checkbox");
    if (typeof labelOrOptions === "string" || typeof labelOrOptions === "function") {
      this.text = labelOrOptions;
    } else if (labelOrOptions) {
      if (labelOrOptions.label !== undefined) this.text = labelOrOptions.label;
      if (labelOrOptions.checked !== undefined) this.checked = labelOrOptions.checked;
      if (labelOrOptions.width !== undefined) this.setProperty("width", labelOrOptions.width);
      if (labelOrOptions.onToggle) {
        this.on("toggle", (e: any) => {
          const checked = typeof e === "boolean" ? e : (typeof e === "string" ? e === "true" : Boolean(e?.checked ?? e?.value === "true"));
          labelOrOptions.onToggle!(checked);
        });
      }
    }
  }

  get checked(): boolean {
    const val = this.properties.checked;
    return val === true || val === "true" || val === 1;
  }
  set checked(val: boolean | (() => boolean)) {
    this.setProperty("checked", typeof val === "function" ? val : Boolean(val));
  }
}

export class Icon extends Node {
  constructor(nameOrOptions?: string | { name?: string; symbol?: string; width?: number; height?: number }) {
    super("icon");
    if (typeof nameOrOptions === "string") {
      this.text = nameOrOptions;
    } else if (nameOrOptions) {
      const name = nameOrOptions.name || nameOrOptions.symbol || "";
      if (name) this.text = name;
      if (nameOrOptions.width !== undefined) this.setProperty("width", nameOrOptions.width);
      if (nameOrOptions.height !== undefined) this.setProperty("height", nameOrOptions.height);
    }
  }
}

export const widgetConstructors: Record<string, new (...args: any[]) => Node> = {
  window: Window,
  vstack: VStack,
  hstack: HStack,
  card: Card,
  box: Box,
  grid: Grid,
  divider: Divider,
  separator: Divider,
  button: Button,
  text: Text,
  textinput: TextInput,
  slider: Slider,
  toggle: Toggle,
  spacer: Spacer,
  scroll: ScrollView,
  scrollview: ScrollView,
  segmented: SegmentedControl,
  progressbar: ProgressBar,
  progress: ProgressBar,
  select: Select,
  dropdown: Select,
  checkbox: Checkbox,
  icon: Icon,
  image: Icon,
};

// Canonical 10-method host operation contract for SolidJS official 'solid-js/universal' createRenderer
export const universalDriver = {
  createElement: (tag: string): Node => {
    const lower = tag.toLowerCase();
    const Ctor = widgetConstructors[lower];
    if (Ctor) {
      return new Ctor();
    }
    return new Node(lower);
  },
  createTextNode: (value: string): Node => {
    return new Text(value);
  },
  isTextNode: (node: Node): boolean => {
    return node instanceof Text;
  },
  replaceText: (node: Node, value: string): void => {
    if (node instanceof Text) {
      node.text = value;
    } else {
      node.setProperty("text", value);
    }
  },
  insertNode: (parent: Node, node: Node, anchor?: Node | null): void => {
    parent.insertBefore(node, anchor || null);
  },
  removeNode: (parent: Node, node: Node): void => {
    parent.removeChild(node);
  },
  setProperty: (node: Node, name: string, value: any): void => {
    node.setProperty(name, value);
  },
  getParentNode: (node: Node): Node | null => {
    return node.parent;
  },
  getFirstChild: (node: Node): Node | null => {
    return node.firstChild;
  },
  getNextSibling: (node: Node): Node | null => {
    return node.nextSibling;
  },
};

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
  const driver = config || universalDriver;
  return {
    ...driver,
    render: (code: () => T, container: T): (() => void) => {
      const root = code();
      container.appendChild(root);
      return () => {
        container.removeChild(root);
      };
    },
  };
}

const defaultRenderer = createRenderer();

function appendChildRecursive(parent: Node, child: any): void {
  if (child === null || child === undefined || child === false || child === true) return;
  if (typeof child === "string" || typeof child === "number") {
    parent.appendChild(new Text(String(child)));
  } else if (typeof child === "function") {
    parent.appendChild(new Text(child));
  } else if (child instanceof Node) {
    parent.appendChild(child);
  } else if (Array.isArray(child)) {
    for (let i = 0; i < child.length; i++) {
      appendChildRecursive(parent, child[i]);
    }
  }
}

export function createElement(tag: any, props?: any, ...children: any[]): Node {
  let node: Node;
  const isClass = typeof tag === "function" && tag.prototype && (tag.prototype instanceof Node || tag.prototype.constructor === Node);

  const allChildren = children.length > 0 ? children : (props && props.children ? (Array.isArray(props.children) ? props.children : [props.children]) : []);

  if (isClass) {
    node = new tag(props);
  } else if (typeof tag === "function") {
    const combinedProps = { ...(props || {}) };
    if (allChildren.length > 0) combinedProps.children = allChildren.length === 1 ? allChildren[0] : allChildren;
    return tag(combinedProps);
  } else if (typeof tag === "string") {
    node = defaultRenderer.createElement(tag);
  } else {
    node = new Node("unknown");
  }

  if (props) {
    for (const key of Object.keys(props)) {
      if (key !== "children" && key !== "key" && key !== "ref") {
        node.setProperty(key, props[key]);
      }
    }
    if (props.ref) {
      if (typeof props.ref === "function") {
        props.ref(node);
      } else if (typeof props.ref === "object" && props.ref !== null) {
        props.ref.current = node;
      }
    }
  }

  if (node instanceof Text && allChildren.length > 0) {
    const first = allChildren[0];
    if (typeof first === "string" || typeof first === "number" || typeof first === "function") {
      node.text = first;
      return node;
    }
  }

  if (node instanceof Button && !node.label && allChildren.length > 0) {
    const first = allChildren[0];
    if (typeof first === "string" || typeof first === "function") {
      node.label = first;
      return node;
    }
  }

  for (let i = 0; i < allChildren.length; i++) {
    appendChildRecursive(node, allChildren[i]);
  }

  return node;
}

export const h = createElement;
export const Fragment = (props: any) => props?.children;

export function jsx(type: any, props: any, key?: any): any {
  if (key !== undefined) {
    props = { ...(props || {}), key };
  }
  return createElement(type, props);
}

export const jsxs = jsx;
export const jsxDEV = jsx;


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

export const gtk = {
  addCss(css: string): boolean {
    const driver = getActiveDriver();
    if (driver && typeof (driver as any).addGlobalCss === "function") {
      return (driver as any).addGlobalCss(css);
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
  pumpEvents(waitMs = 0): boolean {
    const driver = getActiveDriver();
    const driverHad = driver ? driver.pumpEvents(waitMs) : false;
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
      flush();
    } finally {
      isExecutingTurn = false;
    }
  },
  get pendingCount(): number {
    return mailboxQueue.length;
  },
};

export function runLoop(win?: Window, intervalMs = 16): Promise<void> {
  return new Promise((resolve) => {
    let currentInterval = intervalMs;
    let timerId: any = null;

    const tick = () => {
      const waitMs = currentInterval > intervalMs ? 20 : 0;
      const hadEvents = turnMailbox.pumpEvents(waitMs);
      flush();
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

export function runApp(root: Node | (() => Node)): Promise<void> {
  const win = typeof root === "function" ? (root as any)() : root;
  if (win && typeof (win as any).show === "function") {
    (win as any).show();
  }
  if (typeof process !== "undefined" && process.env && process.env.KILN_SNAPSHOT && win && typeof (win as any).snapshot === "function") {
    setTimeout(() => {
      (win as any).snapshot(process.env.KILN_SNAPSHOT!);
      process.exit(0);
    }, 400);
  }
  return runLoop(win as any);
}

export default {
  Node,
  Window,
  VStack,
  HStack,
  Card,
  Box,
  Grid,
  Divider,
  Spacer,
  ScrollView,
  SegmentedControl,
  ProgressBar,
  Select,
  Checkbox,
  Icon,
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
  jsx,
  jsxs,
  jsxDEV,
  app,
  globalShortcut,
  gtk,
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
  isPending,
  flush,
  latest,
  runApp,
  runLoop,
  getActiveDriver,
  universalDriver,
};

