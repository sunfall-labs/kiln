// kiln eval smoke test (authored for kiln, not from porffor).
import { encode, decode } from 'html-entities';

console.log(encode('<div class="x">a & b</div>'));
console.log(decode('&lt;div&gt; &amp; &quot;hi&quot;'));
console.log(encode('© Δ'));
console.log(decode(encode('<>&"\'©')));
console.log(encode('plain text'));
