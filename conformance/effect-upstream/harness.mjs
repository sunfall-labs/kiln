import * as chai from 'chai';
export const assert = chai.assert;
const unsupported = name => { throw new Error('HARNESS_UNSUPPORTED: ' + name); };
export let expect = () => unsupported('expect feature was not loaded');
expect.addEqualityTesters = () => unsupported('expect equality testers were not loaded');
export const installExpect = value => { expect=value; };
export const vi = {};
export const installVi = value => Object.assign(vi,value);
export const vitest = vi;
const realSetTimeout = globalThis.setTimeout;
const realClearTimeout = globalThis.clearTimeout;
const realNow = Date.now;
const noError = {};
const defaultHookTimeout = 10000;
function withDeadline(fn, timeout, kind, onTimeout) {
  if (timeout <= 0 || timeout === Infinity) return fn;
  return function (...args) {
    const started = realNow();
    return new Promise((resolve, reject) => {
      let settled = false;
      let timer;
      const expired = () => new Error(kind + '_TIMEOUT: ' + timeout + 'ms');
      const finish = (rejected, value) => {
        if (settled) return;
        settled = true;
        realClearTimeout(timer);
        if (!rejected && realNow() - started >= timeout) {
          rejected = true; value = expired(); onTimeout?.(value);
        }
        if (rejected) reject(value); else resolve(value);
      };
      timer = realSetTimeout(() => {
        if (settled) return;
        const error = expired(); onTimeout?.(error); finish(true, error);
      }, timeout);
      try {
        const result = fn(...args);
        if (typeof result === 'object' && result !== null && typeof result.then === 'function') {
          result.then(value => finish(false, value), error => finish(true, error));
        } else finish(false, result);
      } catch (error) { finish(true, error); }
    });
  };
}
const hook = (fn, timeout) => withDeadline(fn, timeout ?? defaultHookTimeout, 'HOOK');
const root = {type:'suite', name:'', tasks:[], hooks:{beforeAll:[],afterAll:[],beforeEach:[],afterEach:[]}, mode:'run', parent:null};
let current = root;
const allTests = [];
export const TestRunner = {getCurrentSuite: () => current};
const emit = value => console.log('KILN_EFFECT ' + JSON.stringify(value));
const titles = suite => suite.parent ? [...titles(suite.parent), suite.name] : [];
function suite(name, options, callback, mode='run') {
  if (typeof options === 'function') callback = options;
  if (typeof callback !== 'function') unsupported('suite callback: ' + name);
  const previous = current;
  current = {type:'suite',name:String(name),tasks:[],hooks:{beforeAll:[],afterAll:[],beforeEach:[],afterEach:[]},parent:previous,mode:previous.mode==='skip'?'skip':mode};
  previous.tasks.push(current);
  current.collectCallback=callback;
  current=previous;
}
function testRegister(name, options, callback, mode='run', failing=false) {
  if (typeof options === 'function') { const timeout=callback; callback=options; options=typeof timeout==='number'?{timeout}:{}; }
  if (typeof callback !== 'function' && mode !== 'skip' && mode !== 'todo') unsupported('test callback: ' + name);
  const title = [...titles(current),String(name)].join(' > ');
  const occurrence = allTests.filter(t=>t.title===title).length;
  const task = {type:'test',name:String(name),title,id:occurrence?title+' [occurrence '+(occurrence+1)+']':title,callback,options:options||{},parent:current,mode:current.mode==='skip'?'skip':mode,failing};
  current.tasks.push(task); allTests.push(task);
}
// Registration IDs must not depend on the runtime's inspection/formatting API.
// The upstream callback still receives its unchanged original case arguments.
const caseName = (name, value, index) => String(name)+' [case '+index+']';
function makeTest(mode='run', failing=false) {
  const register = (name, options, callback) => testRegister(name,options,callback,mode,failing);
  register.skip=(name,options,callback)=>testRegister(name,options,callback,'skip');
  register.todo=(name)=>testRegister(name,{},undefined,'todo');
  register.only=register;
  register.concurrent=register;
  register.sequential=register;
  register.fails=(name,options,callback)=>testRegister(name,options,callback,mode,true);
  register.skipIf=condition=>condition?register.skip:register;
  register.runIf=condition=>condition?register:register.skip;
  register.each=cases=>(name,options,callback)=>{
    if(typeof options==='function'){callback=options;options={};}
    cases.forEach((value,index)=>register(caseName(name,value,index),options,ctx=>callback(...(Array.isArray(value)?value:[value]),ctx)));
  };
  register.for=cases=>(name,options,callback)=>{
    if(typeof options==='function'){callback=options;options={};}
    cases.forEach((value,index)=>register(caseName(name,value,index),options,ctx=>callback(value,ctx)));
  };
  register.extend=()=>unsupported('test fixtures');
  return register;
}
export const it=makeTest();
export const test=it;
export const describe=Object.assign((name,options,callback)=>suite(name,options,callback),{
  skip:(name,options,callback)=>suite(name,options,callback,'skip'),
  only:(name,options,callback)=>suite(name,options,callback),
  concurrent:(name,options,callback)=>suite(name,options,callback),
  sequential:(name,options,callback)=>suite(name,options,callback)
});
describe.skipIf=condition=>condition?describe.skip:describe;
describe.runIf=condition=>condition?describe:describe.skip;
describe.each=cases=>(name,fn)=>cases.forEach((value,index)=>describe(caseName(name,value,index),()=>fn(...(Array.isArray(value)?value:[value]))));
export const beforeAll=(fn,timeout)=>{current.hooks.beforeAll.push(hook(fn,timeout));};
export const afterAll=(fn,timeout)=>{current.hooks.afterAll.push(hook(fn,timeout));};
export const beforeEach=(fn,timeout)=>{current.hooks.beforeEach.push(hook(fn,timeout));};
export const afterEach=(fn,timeout)=>{current.hooks.afterEach.push(hook(fn,timeout));};
export const onTestFinished=()=>unsupported('global onTestFinished');
export async function run() {
  function collect(s) {
    const previous=current;current=s;
    try { if(s.collectCallback)s.collectCallback(); for(const child of s.tasks)if(child.type==='suite')collect(child); } finally {current=previous;}
  }
  collect(root);
  const ordered=[];function flatten(s){for(const task of s.tasks){if(task.type==='suite')flatten(task);else ordered.push(task);}}
  flatten(root);allTests.splice(0,allTests.length,...ordered);
  emit({kind:'inventory',tests:allTests.map(t=>({id:t.id,mode:t.mode}))});
  if(process.env.KILN_EFFECT_LIST==='1') return;
  let passed=0,failed=0,skipped=0;
  async function visit(s, inheritedError) {
    let suiteError=inheritedError;
    if(s.mode!=='skip' && suiteError===noError) {
      try {for(const hook of s.hooks.beforeAll) await hook();} catch(e){suiteError=e;}
    }
    for(const task of s.tasks) {
      if(task.type==='suite') {await visit(task,suiteError);continue;}
      if(task.mode==='skip'||task.mode==='todo'){skipped++;emit({kind:'case',id:task.id,outcome:'skip'});continue;}
      const controller=new AbortController();
      const finished=[];
      const context={task,signal:controller.signal,onTestFinished:(fn,timeout)=>{finished.push(withDeadline(fn,timeout??defaultHookTimeout,'HOOK',error=>controller.abort(error)));},expect,skip:()=>unsupported('context.skip')};
      const lineage=[];for(let q=task.parent;q;q=q.parent)lineage.unshift(q);
      let error=suiteError;
      try {
        if(error!==noError) throw error;
        for(const q of lineage)for(const before of q.hooks.beforeEach)await before(context);
        const timeout=task.options.timeout??5000;
        await withDeadline(task.callback,timeout,'TEST',next=>controller.abort(next))(context);
        if(task.failing) error=new Error('Expected test to fail');
      } catch(e) {error=task.failing && !String(e).includes('HARNESS_UNSUPPORTED') ? noError:e;}
      finally {
        try {for(const q of [...lineage].reverse())for(const after of q.hooks.afterEach)await after(context);}
        catch(e){if(error===noError)error=e;}
        for(const finishedHook of [...finished].reverse()) {
          try {await finishedHook(context);} catch(e){if(error===noError)error=e;}
        }
      }
      if(error!==noError){failed++;emit({kind:'case',id:task.id,outcome:'fail',error:String(error),stack:typeof error?.stack==='string'?error.stack.split('\n').slice(0,8).join('\n'):null});}
      else{passed++;emit({kind:'case',id:task.id,outcome:'pass'});}
    }
    if(s.mode!=='skip')for(const hook of s.hooks.afterAll){try{await hook();}catch(e){emit({kind:'hook_error',suite:titles(s).join(' > '),error:String(e)});}}
  }
  await visit(root,noError);
  emit({kind:'summary',discovered:allTests.length,passed,failed,skipped});
  process.exitCode=failed?1:0;
}
