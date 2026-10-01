/** Ambient types for Kiln's engine-owned native GUI extension and SolidJS universal renderer. */
declare module "kiln:gui" {
  export type Accessor<T> = () => T;
  export type Setter<T> = (value: T | ((prev: T) => T)) => void;

  export class Node {
    readonly id: number;
    readonly tag: string;
    parent: Node | null;
    children: Node[];
    properties: Record<string, any>;
    dispose(): void;
    appendChild(child: Node): void;
    removeChild(child: Node): void;
    insertBefore(child: Node, anchor: Node | null): void;
    setProperty(name: string, value: any): void;
    getProperty(name: string): any;
    addEventListener(event: string, handler: Function): void;
    removeEventListener(event: string, handler: Function): void;
    dispatchEvent(event: string, ...args: any[]): void;
    click?(): void;
  }

  export interface WindowOptions {
    title?: string | Accessor<string>;
    width?: number | Accessor<number>;
    height?: number | Accessor<number>;
    content?: Node;
  }
  export class Window extends Node {
    constructor(options?: WindowOptions);
    title: string;
    width: number;
    height: number;
    visible: boolean;
    content: Node | null;
    show(): void;
    close(): void;
    minimize(): void;
    maximize(): void;
    unmaximize(): void;
    setFullscreen(fullscreen: boolean): void;
    setResizable(resizable: boolean): void;
    snapshot(outPath: string): void;
  }

  export interface StackOptions {
    spacing?: number | Accessor<number>;
    padding?: number | Accessor<number>;
    children?: Node[];
  }
  export class VStack extends Node {
    constructor(options?: StackOptions | Node[]);
  }
  export class HStack extends Node {
    constructor(options?: StackOptions | Node[]);
  }

  export interface CardOptions {
    padding?: number | Accessor<number>;
    cornerRadius?: number | Accessor<number>;
    translucent?: boolean | Accessor<boolean>;
    borderWidth?: number | Accessor<number>;
    children?: Node[];
  }
  export class Card extends Node {
    constructor(options?: CardOptions | Node[]);
  }

  export interface DividerOptions {
    orientation?: "horizontal" | "vertical";
  }
  export class Divider extends Node {
    constructor(options?: DividerOptions);
  }

  export class Spacer extends Node {
    constructor();
  }

  export interface ScrollViewOptions {
    content?: Node;
    showsHorizontalScrollIndicator?: boolean;
    showsVerticalScrollIndicator?: boolean;
  }
  export class ScrollView extends Node {
    constructor(options?: ScrollViewOptions | Node);
  }

  export interface SegmentedControlOptions {
    items?: string[] | Accessor<string[]>;
    selectedIndex?: number | Accessor<number>;
    selectedItem?: string | Accessor<string>;
    onChange?: (item: string, index: number) => void;
  }
  export class SegmentedControl extends Node {
    constructor(options?: SegmentedControlOptions);
  }

  export interface ProgressBarOptions {
    value?: number | Accessor<number>;
    min?: number | Accessor<number>;
    max?: number | Accessor<number>;
    indeterminate?: boolean | Accessor<boolean>;
  }
  export class ProgressBar extends Node {
    constructor(options?: ProgressBarOptions);
  }

  export interface SelectOptions {
    items?: string[] | Accessor<string[]>;
    selectedIndex?: number | Accessor<number>;
    selectedItem?: string | Accessor<string>;
    onChange?: (item: string, index: number) => void;
  }
  export class Select extends Node {
    constructor(options?: SelectOptions);
  }

  export interface CheckboxOptions {
    label?: string | Accessor<string>;
    checked?: boolean | Accessor<boolean>;
    onChange?: (checked: boolean) => void;
  }
  export class Checkbox extends Node {
    constructor(options?: CheckboxOptions);
  }

  export interface IconOptions {
    name?: string | Accessor<string>;
    symbol?: string | Accessor<string>;
    width?: number | Accessor<number>;
    height?: number | Accessor<number>;
  }
  export class Icon extends Node {
    constructor(nameOrOptions?: string | IconOptions);
  }

  export interface TextOptions {
    text?: string | Accessor<string>;
    bold?: boolean | Accessor<boolean>;
    size?: number | Accessor<number>;
    color?: string | Accessor<string>;
  }
  export class Text extends Node {
    constructor(textOrOptions?: string | TextOptions);
    text: string;
  }

  export interface ButtonOptions {
    label?: string | Accessor<string>;
    variant?: "primary" | "secondary" | "destructive" | "ghost" | Accessor<string>;
    size?: "sm" | "md" | "lg" | Accessor<string>;
    icon?: string | Accessor<string>;
    onPress?: () => void;
  }
  export class Button extends Node {
    constructor(options?: ButtonOptions);
    label: string;
    variant: string;
    size: string;
    icon: string;
  }

  export interface TextInputOptions {
    placeholder?: string | Accessor<string>;
    value?: string | Accessor<string>;
    onChange?: (val: string) => void;
  }
  export class TextInput extends Node {
    constructor(options?: TextInputOptions);
    placeholder: string;
    value: string;
  }

  export interface SliderOptions {
    min?: number | Accessor<number>;
    max?: number | Accessor<number>;
    value?: number | Accessor<number>;
    onChange?: (val: number) => void;
  }
  export class Slider extends Node {
    constructor(options?: SliderOptions);
    min: number;
    max: number;
    value: number;
  }

  export interface ToggleOptions {
    checked?: boolean | Accessor<boolean>;
    onToggle?: (checked: boolean) => void;
  }
  export class Toggle extends Node {
    constructor(options?: ToggleOptions);
    checked: boolean;
  }

  // Reactivity (Solid 2.0 async-first)
  export function createSignal<T>(initialValue: T): [Accessor<T>, Setter<T>];
  export function createMemo<T>(fn: () => T | Promise<T>): Accessor<T>;
  export function createEffect<T>(
    depsOrFn: () => T | Promise<T>,
    apply?: (value: T) => void | (() => void)
  ): () => void;
  export function batch<T>(fn: () => T): T;
  export function untrack<T>(fn: () => T): T;
  export function onCleanup(fn: () => void): void;
  export function isPending(accessor: () => any): boolean;
  export function flush(): void;
  export function latest<T>(accessor: () => T): T;

  // Solid Universal Driver (10-method host operation contract)
  export const universalDriver: {
    createElement: (tag: string) => Node;
    createTextNode: (value: string) => Node;
    isTextNode: (node: Node) => boolean;
    replaceText: (node: Node, value: string) => void;
    insertNode: (parent: Node, node: Node, anchor?: Node | null) => void;
    removeNode: (parent: Node, node: Node) => void;
    setProperty: (node: Node, name: string, value: any) => void;
    getParentNode: (node: Node) => Node | null;
    getFirstChild: (node: Node) => Node | null;
    getNextSibling: (node: Node) => Node | null;
  };

  export function createRenderer<T extends Node = Node>(config?: any): {
    render: (code: () => T, container?: T) => () => void;
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
  };

  export function render(code: () => Node, container?: Node): () => void;
  export function runLoop(window?: Window): Promise<void>;
  export const systemTheme: { get(): string; onChange(cb: (t: string) => void): () => void };
  export const clipboard: { readText(): string; writeText(t: string): void; clear(): void };
  export const dialog: {
    openFiles(opts?: any): Promise<string[]>;
    saveFile(opts?: any): Promise<string | null>;
    pickFolder(opts?: any): Promise<string | null>;
    showMessage(opts?: any): Promise<number>;
  };
  export function getActiveDriver(): { name: string; [key: string]: any };
}

declare global {
  namespace JSX {
    interface Element extends import("kiln:gui").Node {}
    interface IntrinsicElements {
      window: import("kiln:gui").WindowOptions & { children?: any };
      vstack: import("kiln:gui").StackOptions & { children?: any };
      hstack: import("kiln:gui").StackOptions & { children?: any };
      card: import("kiln:gui").CardOptions & { children?: any };
      box: import("kiln:gui").CardOptions & { children?: any };
      divider: import("kiln:gui").DividerOptions;
      separator: import("kiln:gui").DividerOptions;
      button: import("kiln:gui").ButtonOptions;
      text: import("kiln:gui").TextOptions & { value?: string; children?: any };
      textinput: import("kiln:gui").TextInputOptions;
      slider: import("kiln:gui").SliderOptions;
      toggle: import("kiln:gui").ToggleOptions;
      spacer: {};
      scroll: import("kiln:gui").ScrollViewOptions & { children?: any };
      scrollview: import("kiln:gui").ScrollViewOptions & { children?: any };
      segmented: import("kiln:gui").SegmentedControlOptions;
      progressbar: import("kiln:gui").ProgressBarOptions;
      progress: import("kiln:gui").ProgressBarOptions;
      select: import("kiln:gui").SelectOptions;
      dropdown: import("kiln:gui").SelectOptions;
      checkbox: import("kiln:gui").CheckboxOptions;
      icon: import("kiln:gui").IconOptions;
      image: import("kiln:gui").IconOptions;
    }
  }
}
