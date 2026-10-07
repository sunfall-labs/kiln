const input = String.fromCharCode(0xd800, 0x49, 0xdc00);
const lowered = input.toLocaleLowerCase("tr");
console.log(lowered.length, lowered.charCodeAt(0), lowered.charCodeAt(1), lowered.charCodeAt(2));
