// Fixed input and salt keep the checksum identical for matched native controls.
// Measure this process from outside; --node's clock is deterministic.
import bcrypt from '../../../conformance/npm-compat/node_modules/bcryptjs/index.js';
const salt = '$2b$08$......................';
const hash = bcrypt.hashSync('secret', salt);
console.log(hash);
console.log(bcrypt.compareSync('wrong', hash));
