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
    style?: BoxStyle;
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

  export interface BoxStyle {
    display?: "flex" | "grid" | "none";
    flexDirection?: "row" | "column" | "row-reverse" | "column-reverse";
    flexWrap?: "nowrap" | "wrap" | "wrap-reverse";
    justifyContent?:
      | "flex-start"
      | "start"
      | "flex-end"
      | "end"
      | "center"
      | "space-between"
      | "space-around"
      | "space-evenly";
    alignItems?:
      | "flex-start"
      | "start"
      | "flex-end"
      | "end"
      | "center"
      | "baseline"
      | "stretch";
    alignSelf?:
      | "flex-start"
      | "start"
      | "flex-end"
      | "end"
      | "center"
      | "baseline"
      | "stretch";
    gap?: number | string;
    rowGap?: number | string;
    columnGap?: number | string;
    padding?: number | string | [number, number] | [number, number, number, number];
    margin?: number | string;
    width?: number | string;
    height?: number | string;
    minWidth?: number | string;
    minHeight?: number | string;
    maxWidth?: number | string;
    maxHeight?: number | string;
    flexGrow?: number;
    flexShrink?: number;
    flexBasis?: number | string;
    gridTemplateColumns?: string;
    gridTemplateRows?: string;
  }

  export interface BoxOptions {
    style?: BoxStyle | Accessor<BoxStyle>;
    width?: number | Accessor<number>;
    height?: number | Accessor<number>;
    children?: Node[];
  }
  export class Box extends Node {
    constructor(options?: BoxOptions | Node[]);
    style: BoxStyle;
  }

  export interface GridOptions {
    columns?: string | Accessor<string>;
    rows?: string | Accessor<string>;
    gap?: number | string | Accessor<number | string>;
    style?: BoxStyle | Accessor<BoxStyle>;
    width?: number | Accessor<number>;
    height?: number | Accessor<number>;
    children?: Node[];
  }
  export class Grid extends Node {
    constructor(options?: GridOptions | Node[]);
    columns: string;
    rows: string;
    gap: number | string;
    style: BoxStyle;
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
  export function runLoop(win?: Window, intervalMs?: number): Promise<void>;
  export function runApp(root: any): Promise<void>;
  export function jsx(type: any, props: any, key?: any): any;
  export function jsxs(type: any, props: any, key?: any): any;
  export function jsxDEV(type: any, props: any, key?: any, isStatic?: boolean, source?: any, self?: any): any;
}

declare namespace JSX {
  type Element = import("kiln:gui").Node;
  interface ElementChildrenAttribute {
    children: {};
  }
  interface IntrinsicElements {
    window: import("kiln:gui").WindowOptions & { style?: import("kiln:gui").BoxStyle | any; children?: any };
    vstack: import("kiln:gui").StackOptions & { style?: import("kiln:gui").BoxStyle | any; children?: any };
    hstack: import("kiln:gui").StackOptions & { style?: import("kiln:gui").BoxStyle | any; children?: any };
    card: import("kiln:gui").CardOptions & { style?: import("kiln:gui").BoxStyle | any; children?: any };
    box: import("kiln:gui").BoxOptions & { style?: import("kiln:gui").BoxStyle | any; children?: any };
    grid: import("kiln:gui").GridOptions & { style?: import("kiln:gui").BoxStyle | any; children?: any };
    divider: import("kiln:gui").DividerOptions & { style?: import("kiln:gui").BoxStyle | any };
    separator: import("kiln:gui").DividerOptions & { style?: import("kiln:gui").BoxStyle | any };
    button: import("kiln:gui").ButtonOptions & { style?: import("kiln:gui").BoxStyle | any; children?: any };
    text: import("kiln:gui").TextOptions & { value?: string; style?: import("kiln:gui").BoxStyle | any; children?: any };
    textinput: import("kiln:gui").TextInputOptions & { style?: import("kiln:gui").BoxStyle | any };
    slider: import("kiln:gui").SliderOptions & { style?: import("kiln:gui").BoxStyle | any };
    toggle: import("kiln:gui").ToggleOptions & { style?: import("kiln:gui").BoxStyle | any };
    spacer: { style?: import("kiln:gui").BoxStyle | any };
    scroll: import("kiln:gui").ScrollViewOptions & { style?: import("kiln:gui").BoxStyle | any; children?: any };
    scrollview: import("kiln:gui").ScrollViewOptions & { style?: import("kiln:gui").BoxStyle | any; children?: any };
    segmented: import("kiln:gui").SegmentedControlOptions & { style?: import("kiln:gui").BoxStyle | any };
    progressbar: import("kiln:gui").ProgressBarOptions & { style?: import("kiln:gui").BoxStyle | any };
    progress: import("kiln:gui").ProgressBarOptions & { style?: import("kiln:gui").BoxStyle | any };
    select: import("kiln:gui").SelectOptions & { style?: import("kiln:gui").BoxStyle | any };
    dropdown: import("kiln:gui").SelectOptions & { style?: import("kiln:gui").BoxStyle | any };
    checkbox: import("kiln:gui").CheckboxOptions & { style?: import("kiln:gui").BoxStyle | any };
    icon: import("kiln:gui").IconOptions & { style?: import("kiln:gui").BoxStyle | any };
    image: import("kiln:gui").IconOptions & { style?: import("kiln:gui").BoxStyle | any };
  }
}

declare module "kiln/jsx-runtime" {
  export { jsx, jsxs, jsxDEV, Fragment } from "kiln:gui";
  import { jsx, jsxs, jsxDEV, Fragment } from "kiln:gui";
  export default { jsx, jsxs, jsxDEV, Fragment };
}

declare module "kiln:jsx-runtime" {
  export { jsx, jsxs, jsxDEV, Fragment } from "kiln:gui";
  import { jsx, jsxs, jsxDEV, Fragment } from "kiln:gui";
  export default { jsx, jsxs, jsxDEV, Fragment };
}

declare module "kiln:gui/jsx-runtime" {
  export { jsx, jsxs, jsxDEV, Fragment } from "kiln:gui";
  import { jsx, jsxs, jsxDEV, Fragment } from "kiln:gui";
  export default { jsx, jsxs, jsxDEV, Fragment };
}

