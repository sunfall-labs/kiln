// Kiln Universal JSX Runtime (ADR 0074 + Solid 2.0 Reactivity)
import { createElement, Fragment } from "kiln:gui";

export function jsx(type: any, props: any, key?: any): any {
  if (key !== undefined) {
    props = { ...(props || {}), key };
  }
  return createElement(type, props);
}

export const jsxs = jsx;
export const jsxDEV = jsx;
export { Fragment };
export default { jsx, jsxs, jsxDEV, Fragment };
