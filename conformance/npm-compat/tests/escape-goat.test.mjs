// kiln eval smoke test (authored for kiln, not from porffor).
import {htmlEscape, htmlUnescape} from 'escape-goat';
console.log(htmlEscape('<a href="x">&'));
console.log(htmlUnescape('&lt;a&gt;'));
console.log(htmlEscape('plain text'));
console.log(htmlUnescape(htmlEscape('<b>"quoted"</b>')));
