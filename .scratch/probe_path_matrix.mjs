import p from "node:path";
const paths=["","/","a","a/","/a","a/b","a//b","/a/b/","..","../..",".","./a","/a/./b","/a/../b","a.js","/foo/bar.js","/foo/.js",".hidden","/foo/bar.","/a/b.c/d","//a//b//"];
const out={dirname:{},basename:{},extname:{}};
for(const x of paths){out.dirname[x]=p.dirname(x);out.basename[x]=p.basename(x);out.extname[x]=p.extname(x);}
const bx=[["/foo/bar.js","bar.js"],["bar.js","bar.js"],["/foo/.js",".js"],["/foo/bar.js",".js"],["/a/b/","b"],["/foo/bar.js","z.js"],["a.js",".js"]];
out.basename_ext=bx.map(([a,b])=>[a,b,p.basename(a,b)]);
const rel=[["/a/b","/a/b/c"],["/a/b/c","/a/b"],["/a","/b"],["a","b"],["/a/b","/a/b"],["/","/a"],["/a/b/c","/a/d"],["a/b","a/b/c"]];
out.relative=rel.map(([f,t])=>[f,t,p.relative(f,t)]);
console.log(JSON.stringify(out));
