const { transform } = require('sucrase');

console.log('--- TypeScript stripping ---');
const tsCode = `
interface User { id: number; name: string; }
type Status = 'open' | 'closed';
function getInfo(u: User, status: Status): string {
  return u.name + ' is ' + status;
}
const val = { id: 1, name: 'Alice' } as User;
`;
console.log(transform(tsCode, { transforms: ['typescript'] }).code.trim());

console.log('--- TypeScript Enum & Class Parameter Properties ---');
const tsAdvanced = `
enum LogLevel {
  Debug = 10,
  Info = 20,
  Error = 30
}
class Logger {
  constructor(public prefix: string, private level: LogLevel) {}
  log(msg: string) { console.log(this.prefix + msg); }
}
`;
console.log(transform(tsAdvanced, { transforms: ['typescript'] }).code.trim());

console.log('--- JSX Transformation (production) ---');
const jsxCode = `
const Element = (
  <div id="root" className="main-box">
    <Header active={true} />
    <section>
      <span>Item 1</span>
      <span>Item 2</span>
    </section>
  </div>
);
`;
console.log(transform(jsxCode, { transforms: ['jsx'], production: true }).code.trim());

console.log('--- JSX Fragment ---');
const fragCode = 'const Frags = () => <><b>Bold</b><i>Italic</i></>;';
console.log(transform(fragCode, { transforms: ['jsx'], production: true }).code.trim());

console.log('--- Imports & Exports ---');
const importCode = `
import defaultItem, { named1, named2 as alias } from './module';
import * as all from './all';
export const constant = 100;
export function run() { return constant; }
export default defaultItem;
`;
console.log(transform(importCode, { transforms: ['imports'] }).code.trim());

console.log('--- Combined TS + JSX + Imports ---');
const fullCode = `
import React from 'react';
interface CardProps { title: string; count: number; }
export const Card: React.FC<CardProps> = ({ title, count }) => (
  <div className="card">
    <h3>{title}</h3>
    <span>{count}</span>
  </div>
);
`;
console.log(transform(fullCode, { transforms: ['typescript', 'jsx', 'imports'], production: true }).code.trim());
