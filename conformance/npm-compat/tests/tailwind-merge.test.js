const { twMerge, twJoin, extendTailwindMerge } = require('tailwind-merge');

console.log('--- Overrides & Hierarchies ---');
console.log(twMerge('px-2 py-1 p-3'));
console.log(twMerge('p-3 px-2'));
console.log(twMerge('m-2 mx-4 -my-2'));
console.log(twMerge('-inset-2 inset-4'));

console.log('--- Colors & Opacity ---');
console.log(twMerge('text-red-500 text-blue-600 font-bold'));
console.log(twMerge('bg-red-500/50 bg-blue-500/80'));
console.log(twMerge('border-t-2 border-red-500 border-t-blue-500 border-4'));

console.log('--- Modifiers & Pseudo-classes ---');
console.log(twMerge('hover:bg-red-500 hover:bg-blue-500'));
console.log(twMerge('dark:focus:text-white dark:focus:text-black'));
console.log(twMerge('hover:focus:p-2 focus:hover:p-4'));
console.log(twMerge('hover:p-2 p-4'));

console.log('--- Arbitrary Values & Important ---');
console.log(twMerge('m-[2px] m-[4px]'));
console.log(twMerge('!p-2 !p-4'));
console.log(twMerge('p-2 !p-4'));
console.log(twMerge('bg-[#123456] bg-[#abcdef]'));

console.log('--- twJoin conditionals and arrays ---');
console.log(twJoin(
  'btn',
  true && 'btn-primary',
  false && 'btn-disabled',
  null,
  undefined,
  ['mt-2', ['text-base', false, 'font-medium']]
));

console.log('--- Custom TailwindMerge instance ---');
const customTwMerge = extendTailwindMerge({
  extend: {
    classGroups: {
      shadow: [{ shadow: ['custom-lg', 'custom-xl'] }]
    }
  }
});
console.log(customTwMerge('shadow-custom-lg shadow-custom-xl'));
