// kiln eval smoke test (authored for kiln, not from porffor).
import { encodeXML, decodeXML, encodeHTML, decodeHTML } from 'entities';
console.log(encodeXML('<é & "ü">'));
console.log(decodeXML('&lt;é &amp; &quot;ü&quot;&gt;'));
console.log(encodeHTML('a>b ©'));
console.log(decodeHTML('&copy; &amp; &lt;tag&gt;'));
