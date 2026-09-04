// Receiver-aware method inline caching and fast length access verification

// 1. Array length and method IC
function processArray(arr) {
    let len = arr.length;
    let sum = 0;
    for (let i = 0; i < len; i++) {
        sum += arr[i];
    }
    arr.push(sum);
    return arr.pop();
}

const a1 = [1, 2, 3, 4, 5];
console.log('processArray 1:', processArray(a1));
const a2 = [10, 20, 30];
console.log('processArray 2:', processArray(a2));

// Loop to verify repeated method IC hits
for (let i = 0; i < 50; i++) {
    a1.push(i);
    a1.pop();
}
console.log('a1 length after loop:', a1.length);

// 2. String length and method IC
function processString(str) {
    let len = str.length;
    let code = str.charCodeAt(0);
    let sub = str.slice(1);
    let idx = str.indexOf('c');
    return len + code + sub.length + idx;
}

console.log('processString abcde:', processString('abcde'));
console.log('processString hello:', processString('hello'));

// 3. Number method IC
function formatNumber(n) {
    return n.toFixed(2);
}

console.log('formatNumber 3.14159:', formatNumber(3.14159));
console.log('formatNumber 42:', formatNumber(42));

// 4. Object length property should NOT collide with array/string length fast path
const objWithLength = { length: 99, value: 'custom' };
console.log('objWithLength.length:', objWithLength.length);
