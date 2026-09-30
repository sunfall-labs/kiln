function make() { return new RegExp("(?<letter>[a-z])", "g"); }
const first = make();
const second = make();
console.log(first !== second, first.exec("ab").groups.letter, first.lastIndex, second.lastIndex);
console.log(second.exec("xy").groups.letter, second.lastIndex);
console.log(first.exec("ab").groups.letter, first.lastIndex);
console.log(second.exec("xy").groups.letter, second.lastIndex);
console.log(first.exec("ab") === null, first.lastIndex, second.lastIndex);
