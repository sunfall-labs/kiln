console.log('number no match', 'a day'.replace(/%d/i, 1));
console.log('number match', '%d days'.replace(/%d/i, 2));
console.log('string no match', 'a day'.replace(/%d/i, '1'));
console.log('string match', '%d days'.replace(/%d/i, '2'));
