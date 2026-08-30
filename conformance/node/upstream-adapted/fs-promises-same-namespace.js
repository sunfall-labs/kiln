'use strict';
const first = process.getBuiltinModule('fs/promises');
const second = process.getBuiltinModule('node:fs/promises');
if (first !== second) throw new Error('builtin module namespace identity changed');
console.log('pass');
