// A closure exported from another module retains the authority of its captured bindings.
import { makeReadonly, makeNamed } from './fixtures/directeval-readonly-module.mjs';
const assign = makeReadonly('(function () { captured = "changed"; })');
let rejected = false;
try { assign(); } catch (error) { rejected = error instanceof TypeError; }
if (!rejected) throw new Error('imported eval closure lost captured const authority');
const named = makeNamed('(function self() { self = 1; return self; })');
if (named() !== named) throw new Error('imported eval closure lost its immutable self binding');
console.log('imported eval binding authority: ok');
