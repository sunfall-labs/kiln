const {Worker,isMainThread,parentPort,workerData}=require('node:worker_threads');
const {share,isShared}=require('kiln:shared');
if (isMainThread) {
  Object.prototype.parentMarker=41;
  const root=share({array:[1,2],nested:{x:7}});
  const w=new Worker(__filename,{workerData:{root},env:{}});
  w.on('message',x=>{console.log('parent', x.join(','), root.nested.childMarker, root.nested.parentMarker, root.array instanceof Array);});
  w.on('error',e=>console.log('error',e.message));
  w.on('exit',c=>console.log('exit',c));
}else{
  const root=workerData.root;
  Object.prototype.childMarker=53;
  parentPort.postMessage([isShared(root), root.nested.x, root.nested.parentMarker, root.nested.childMarker, Object.getPrototypeOf(root)===Object.prototype, Object.getPrototypeOf(root.array)===Array.prototype, root.array instanceof Array]);
}
