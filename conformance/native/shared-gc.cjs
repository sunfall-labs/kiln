const {share,isShared}=require('kiln:shared');
const {MessageChannel}=require('node:worker_threads');
let holder={edge:share({child:{n:7}})};
for(let i=0;i<48;i++) { const scratch={a:[i,i+1],b:{x:i}}; if(scratch.b.x<0) throw Error('bad'); }
console.log('held',isShared(holder.edge.child),holder.edge.child.n);
const {port1,port2}=new MessageChannel();
port1.postMessage({edge:holder.edge});
port2.close();
port1.close();
holder=null;
for(let i=0;i<48;i++) { const scratch={a:[i,i+1],b:{x:i}}; if(scratch.b.x<0) throw Error('bad'); }
console.log('discarded');
