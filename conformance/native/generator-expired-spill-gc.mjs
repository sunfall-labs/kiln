import { setFlagsFromString } from 'node:v8';
import { runInNewContext } from 'node:vm';
setFlagsFromString('--expose_gc');
const collect = runInNewContext('gc');
setFlagsFromString('--no-expose_gc');
function makeControl(){ return new WeakRef({}); }
const control = makeControl();
const refs=[];
const create=()=>{ const target={}; refs.push(new WeakRef(target)); return target; };
function consume(first, second){ console.log(first === refs[0].deref(), second); }
function* values(){ consume(yield create(), yield 0); yield 1; yield 2; }
const generator=values();
function advance(){ const value=generator.next().value; generator.next(value); generator.next(); }
advance();
async function finish(){
for(let i=0;i<8;i++){ await new Promise(r=>setTimeout(r,0)); collect(); }
console.log(control.deref()===undefined, refs[0].deref()===undefined, generator.next().value);
}
finish().catch(console.error);
