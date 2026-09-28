const {share,isShared}=require('kiln:shared');
const root=share({child:{x:7}});
const ref=new WeakRef(root);
const map=new WeakMap([[root,{n:9}]]);
const set=new WeakSet([root]);
console.log('weak', ref.deref()===root, ref.deref().child.x, map.get(root).n, set.has(root), Object.prototype.toString.call(ref));
console.log('hook', typeof globalThis.__kilnSharedWeak);
