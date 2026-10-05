// kiln npm functional test: papaparse
const Papa = require('papaparse');

// 1. Quoting with embedded delimiters, newlines, and escaped quotes
const csvInput = 'id,name,bio\n1,\"Turing, Alan\",\"Pioneer in \"\"computing\"\"\nSecond line.\"\n2,Lovelace,Mathematician';
const parsed = Papa.parse(csvInput, { header: true, skipEmptyLines: true });
console.log('row-count:', parsed.data.length);
console.log('row-1-name:', parsed.data[0].name);
console.log('row-1-bio-lines:', parsed.data[0].bio.split('\n').length);
console.log('row-1-bio-contains-quote:', parsed.data[0].bio.includes('\"computing\"'));

// 2. Dynamic typing and header transformation
const typedCsv = 'item,quantity,price,active\n\"widget\",10,19.99,true\n\"gadget\",0,5.50,false';
const typedParsed = Papa.parse(typedCsv, {
  header: true,
  dynamicTyping: true,
  transformHeader: h => h.toUpperCase()
});
console.log('types:', typeof typedParsed.data[0].QUANTITY, typeof typedParsed.data[0].PRICE, typeof typedParsed.data[0].ACTIVE);
console.log('typed-values:', typedParsed.data[0].QUANTITY, typedParsed.data[0].PRICE, typedParsed.data[0].ACTIVE);

// 3. Delimiter auto-detection with comments and preview
const tsvWithComments = '# config export\n# generated 2026\nalpha\tbeta\tgamma\n100\t200\t300\n400\t500\t600';
const detected = Papa.parse(tsvWithComments, {
  comments: '#',
  preview: 2,
  header: true
});
console.log('detected-delimiter:', JSON.stringify(detected.meta.delimiter));
console.log('preview-rows:', detected.data.length);
console.log('preview-data:', JSON.stringify(detected.data));

// 4. Unparsing (serialization) with custom quote options
const dataToUnparse = [
  { symbol: 'AAPL', company: 'Apple, Inc.', pe: 32.5 },
  { symbol: 'GOOG', company: 'Alphabet \"Class A\"', pe: 24.1 }
];
const unparsed = Papa.unparse(dataToUnparse, { quotes: [false, true, false] });
console.log('unparsed-lines:');
console.log(unparsed);
