// Verification of array iterator preservation across GC cycles and operations

function testArrayIterator() {
    const arr = [10, 20, 30, 40, 50];
    const it = arr[Symbol.iterator]();
    
    // Step through the iterator
    let res = it.next();
    console.log('first:', res.value, res.done);
    
    // Allocate heavily to trigger GC collection epochs
    let sink = [];
    for (let i = 0; i < 5000; i++) {
        sink.push({ a: i, b: 'str' + i, c: [i] });
    }
    
    // Next step after GC
    res = it.next();
    console.log('second after gc:', res.value, res.done);
    
    // Finish iterating
    while (!res.done) {
        res = it.next();
    }
    console.log('finished:', res.value, res.done);
}

function testApplyWithIterator() {
    function fn(a, b, c) {
        return a + b + c;
    }
    const arr = [1, 2, 3];
    console.log('apply:', fn.apply(null, arr));
    console.log('spread:', fn(...arr));
}

testArrayIterator();
testApplyWithIterator();
