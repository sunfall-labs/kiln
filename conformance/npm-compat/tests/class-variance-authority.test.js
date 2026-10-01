// kiln eval smoke test (authored for kiln, not from porffor).
const {cva} = require('class-variance-authority');
const btn = cva('base', {
  variants: {size: {sm: 's', lg: 'l'}, tone: {a: 'ta'}},
  defaultVariants: {size: 'sm'},
});
console.log(JSON.stringify(btn()));
console.log(JSON.stringify(btn({size: 'lg'})));
console.log(JSON.stringify(btn({size: 'lg', class: 'extra'})));
console.log(JSON.stringify(btn({size: 'lg', tone: 'a'})));
console.log(JSON.stringify(btn({size: undefined})));
