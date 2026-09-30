// kiln eval smoke test (authored for kiln, not from porffor).
const { twMerge, twJoin } = require('tailwind-merge');

console.log(twMerge('px-2 px-4'));
console.log(twMerge('text-sm text-lg font-bold'));
console.log(twMerge('bg-red-500', 'bg-blue-500'));
console.log(twJoin('a', false && 'b', 'c'));
console.log(twMerge('p-1 p-2 m-3 m-4'));
