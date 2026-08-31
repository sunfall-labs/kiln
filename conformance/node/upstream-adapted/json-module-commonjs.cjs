const data = require('./json-module-data.json');
if (data.name !== 'kiln' || data.nested.port !== 3000) throw new Error('CommonJS JSON');
module.exports = data;
