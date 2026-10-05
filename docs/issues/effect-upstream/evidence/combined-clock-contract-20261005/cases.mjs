export function register({it,describe,beforeEach}) {
  let cleaned=false;
  it("hung cleanup", ctx=>{
    ctx.onTestFinished(()=>{cleaned=true;});
    ctx.onTestFinished(()=>new Promise(()=>{}),10);
  });
  it("cleanup continues",()=>{if(!cleaned)throw new Error("earlier cleanup was skipped");});
  describe("hook budget",()=>{
    beforeEach(()=>new Promise(resolve=>setTimeout(resolve,20)),100);
    it("hook does not spend body budget",()=>{},5);
  });
  it("sync overrun",()=>{const start=Date.now();while(Date.now()-start<20){};},5);
  it("undefined throw",()=>{throw undefined;});
}
