// kiln eval smoke test (authored for kiln, not from porffor).
const dotenv = require('dotenv');
console.log(JSON.stringify(dotenv.parse('A=1\nB=hello world\nC="quoted"')));
console.log(JSON.stringify(dotenv.parse('# comment\nD=multi\nE=\'single\'\nEMPTY=')));
console.log(JSON.stringify(dotenv.parse('PORT=8080\nHOST=localhost')));
