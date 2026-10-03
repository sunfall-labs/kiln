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
export const beforeAll=fn=>current.hooks.beforeAll.push(fn);
export const afterAll=fn=>current.hooks.afterAll.push(fn);
export const beforeEach=fn=>current.hooks.beforeEach.push(fn);
export const afterEach=fn=>current.hooks.afterEach.push(fn);
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
    if(s.mode!=='skip' && !suiteError) {
      try {for(const hook of s.hooks.beforeAll) await hook();} catch(e){suiteError=e;}
    }
    for(const task of s.tasks) {
      if(task.type==='suite') {await visit(task,suiteError);continue;}
      if(task.mode==='skip'||task.mode==='todo'){skipped++;emit({kind:'case',id:task.id,outcome:'skip'});continue;}
      const controller=new AbortController();
      const finished=[];
      const context={task,signal:controller.signal,onTestFinished:fn=>finished.push(fn),expect,skip:()=>unsupported('context.skip')};
      const lineage=[];for(let q=task.parent;q;q=q.parent)lineage.unshift(q);
      let error=suiteError;
      let timer;
      try {
        if(error) throw error;
        const execute=async()=>{for(const q of lineage)for(const hook of q.hooks.beforeEach)await hook(context);await task.callback(context);};
        const timeout=Math.max(1,Number(task.options.timeout)||5000);
        await Promise.race([execute(),new Promise((_,reject)=>{timer=realSetTimeout(()=>{controller.abort();reject(new Error('TEST_TIMEOUT: '+timeout+'ms'));},timeout);})]);
        if(task.failing) error=new Error('Expected test to fail');
      } catch(e) {error=task.failing && !String(e).includes('HARNESS_UNSUPPORTED') ? undefined:e;}
      finally {
        realClearTimeout(timer);
        try {
          for(const q of [...lineage].reverse())for(const hook of q.hooks.afterEach)await hook(context);
          for(const hook of [...finished].reverse())await hook();
        } catch(e){error=error||e;}
      }
      if(error){failed++;emit({kind:'case',id:task.id,outcome:'fail',error:String(error),stack:typeof error.stack==='string'?error.stack.split('\n').slice(0,8).join('\n'):null});}
      else{passed++;emit({kind:'case',id:task.id,outcome:'pass'});}
    }
    if(s.mode!=='skip')for(const hook of s.hooks.afterAll){try{await hook();}catch(e){emit({kind:'hook_error',suite:titles(s).join(' > '),error:String(e)});}}
  }
  await visit(root);
  emit({kind:'summary',discovered:allTests.length,passed,failed,skipped});
  process.exitCode=failed?1:0;
}
