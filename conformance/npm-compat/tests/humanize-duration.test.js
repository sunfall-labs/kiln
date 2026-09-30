// kiln eval smoke test (authored for kiln, not from porffor).
const humanize = require('humanize-duration');
console.log(humanize(3000));
console.log(humanize(65000));
console.log(humanize(3600000));
console.log(humanize(0));
console.log(humanize(90000, { units: ['m'], round: true }));
console.log(humanize(1000, { language: 'es' }));
