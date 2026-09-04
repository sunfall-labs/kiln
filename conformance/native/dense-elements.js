// Dense elements on shape-backed objects: separation of integer indices from named shape properties.

const obj = { x: 10, y: 20 };
// Integer indices set on shape-backed object
obj[0] = 'zero';
obj[1] = 'one';
obj[2] = 'two';

// 1. Direct indexed access
console.log('elements:', obj[0], obj[1], obj[2]);
console.log('named:', obj.x, obj.y);

// 2. Ordering in Object.keys (must be integer indices in ascending order, then named properties)
console.log('Object.keys:', JSON.stringify(Object.keys(obj)));
console.log('Object.values:', JSON.stringify(Object.values(obj)));
console.log('Object.entries:', JSON.stringify(Object.entries(obj)));

// 3. Spread and Object.assign
const spread = { ...obj };
console.log('spread keys:', JSON.stringify(Object.keys(spread)));
console.log('spread values:', spread[0], spread[1], spread[2], spread.x, spread.y);

const assigned = Object.assign({}, obj);
console.log('assigned keys:', JSON.stringify(Object.keys(assigned)));

// 4. JSON.stringify
console.log('JSON.stringify:', JSON.stringify(obj));

// 5. In operator & hasOwnProperty
console.log('0 in obj:', 0 in obj);
console.log('1 in obj:', 1 in obj);
console.log('5 in obj:', 5 in obj);
console.log('x in obj:', 'x' in obj);
console.log('z in obj:', 'z' in obj);

// 6. Delete element (leaves hole)
delete obj[1];
console.log('after delete obj[1]:', obj[1], 1 in obj);
console.log('keys after delete:', JSON.stringify(Object.keys(obj)));
console.log('json after delete:', JSON.stringify(obj));

// 7. Re-assign deleted element
obj[1] = 're-one';
console.log('re-assigned obj[1]:', obj[1], 1 in obj);
console.log('keys after reassign:', JSON.stringify(Object.keys(obj)));
