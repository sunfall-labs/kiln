// TextDecoder in both engines resolves labels and decodes through `kiln-encoding` (ADR 0060),
// reached by the interpreter directly and by native code through the provider's
// `kiln_text_decoder_label` / `kiln_text_decode`. Every row prints code points, not text, so a
// terminal's encoding cannot matter, and an error prints its class and message.
const hex = (text) => Array.from(text, (c) => c.codePointAt(0).toString(16)).join(' ');
const bytes = (...values) => new Uint8Array(values);

function show(name, run) {
  try {
    console.log(name + ': ' + run());
  } catch (error) {
    console.log(name + ': throws ' + error.name + ': ' + error.message);
  }
}

const decode = (label, input, options) => hex(new TextDecoder(label, options).decode(input));

// UTF-16, with and without a byte order mark.
show('utf-16le bom', () => decode('utf-16le', bytes(0xff, 0xfe, 0x41, 0x00, 0x42, 0x00)));
show('utf-16le plain', () => decode('utf-16le', bytes(0x41, 0x00, 0x42, 0x00)));
show('utf-16be bom', () => decode('utf-16be', bytes(0xfe, 0xff, 0x00, 0x41)));
show('utf-16be plain', () => decode('utf-16be', bytes(0x00, 0x41, 0x20, 0xac)));
show('utf-16le ignoreBOM', () =>
  decode('utf-16le', bytes(0xff, 0xfe, 0x41, 0x00), { ignoreBOM: true }));
show('utf-16le odd length', () => decode('utf-16le', bytes(0x41, 0x00, 0x42)));

// Single-byte encodings.
show('windows-1252', () => decode('windows-1252', bytes(0x80, 0xe9, 0x41)));
show('iso-8859-2', () => decode('iso-8859-2', bytes(0xa1, 0x41)));
show('koi8-r', () => decode('koi8-r', bytes(0xc1, 0xc2)));
show('x-user-defined', () => decode('x-user-defined', bytes(0x80, 0x41)));

// Multi-byte legacy encodings.
show('gbk', () => decode('gbk', bytes(0xc4, 0xe3, 0xba, 0xc3)));
show('shift_jis', () => decode('shift_jis', bytes(0x82, 0xa0, 0x93, 0xfa, 0x96, 0x7b)));
show('euc-kr', () => decode('euc-kr', bytes(0xb0, 0xa1)));
show('iso-2022-jp', () =>
  decode('iso-2022-jp', bytes(0x1b, 0x24, 0x42, 0x24, 0x22, 0x1b, 0x28, 0x42, 0x41)));
show('replacement', () => decode('replacement', bytes(0x41, 0x42)));

// UTF-8: lossy by default, `fatal` throws a TypeError, and the BOM is the option's to keep.
show('utf-8 lossy', () => decode('utf-8', bytes(0x61, 0xff, 0x62)));
show('utf-8 fatal', () => decode('utf-8', bytes(0x61, 0xff, 0x62), { fatal: true }));
show('utf-8 fatal valid', () => decode('utf-8', bytes(0xe2, 0x82, 0xac), { fatal: true }));
show('utf-8 bom stripped', () => decode('utf-8', bytes(0xef, 0xbb, 0xbf, 0x41)));
show('utf-8 ignoreBOM', () => decode('utf-8', bytes(0xef, 0xbb, 0xbf, 0x41), { ignoreBOM: true }));
show('shift_jis fatal', () => decode('shift_jis', bytes(0x82), { fatal: true }));

// Streaming preserves an incomplete BOM, UTF-8 scalar, and legacy shift sequence across calls.
show('stream split', () => {
  const decoder = new TextDecoder();
  const first = decoder.decode(bytes(0xef), { stream: true });
  const second = decoder.decode(bytes(0xbb, 0xbf, 0xf0, 0x9f), { stream: true });
  const third = decoder.decode(bytes(0x98, 0x80));
  const next = decoder.decode(bytes(0xef, 0xbb, 0xbf, 0x42));
  return [hex(first), hex(second), hex(third), hex(next)].join(' | ');
});
show('stream fatal flush and reset', () => {
  const decoder = new TextDecoder('utf-8', { fatal: true });
  const partial = decoder.decode(bytes(0xe2), { stream: true });
  let flush;
  try {
    decoder.decode(bytes());
    flush = 'no error';
  } catch (error) {
    flush = error.name;
  }
  return [hex(partial), flush, hex(decoder.decode(bytes(0x41)))].join(' | ');
});
show('stream iso-2022-jp shift', () => {
  const decoder = new TextDecoder('iso-2022-jp');
  const first = decoder.decode(bytes(0x1b, 0x24), { stream: true });
  const second = decoder.decode(bytes(0x42, 0x24), { stream: true });
  const third = decoder.decode(bytes(0x22, 0x1b, 0x28, 0x42));
  return [hex(first), hex(second), hex(third)].join(' | ');
});

// Labels: trimmed, case-folded, and reported by their canonical lowercase name.
for (const label of [
  ' UTF8 ', 'unicode-1-1-utf-8', 'latin1', 'ascii', 'ucs-2', 'utf-16', 'utf-16be', 'sjis',
  'GBK', 'gb2312', 'big5', 'euc-jp', 'x-user-defined', 'replacement', 'iso-2022-kr', 'hz-gb-2312',
]) {
  show('label ' + JSON.stringify(label), () => new TextDecoder(label).encoding);
}
show('default label', () => new TextDecoder().encoding);
show('undefined label', () => new TextDecoder(undefined).encoding);

// Invalid labels are a RangeError naming the label.
show('invalid nope', () => new TextDecoder('nope').encoding);
show('invalid empty', () => new TextDecoder('').encoding);
show('invalid utf-7', () => new TextDecoder('utf-7').encoding);

// Attributes.
show('attributes default', () => {
  const decoder = new TextDecoder('utf-8');
  return [decoder.encoding, decoder.fatal, decoder.ignoreBOM].join(' ');
});
show('attributes set', () => {
  const decoder = new TextDecoder('windows-1252', { fatal: true, ignoreBOM: true });
  return [decoder.encoding, decoder.fatal, decoder.ignoreBOM].join(' ');
});

// Inputs: a view onto part of a buffer, a whole buffer, and nothing.
show('subarray view', () => decode('utf-8', bytes(0x41, 0x42, 0x43, 0x44).subarray(1, 3)));
show('array buffer', () => decode('utf-8', bytes(0xc3, 0xa9).buffer));
show('no input', () => decode('utf-8', undefined));
show('empty fatal', () => decode('utf-16le', bytes(), { fatal: true }));
// The message's tail names the offending type in the interpreter only, so print the class alone.
try {
  decode('utf-8', 'text');
} catch (error) {
  console.log('bad input: throws ' + error.name);
}
