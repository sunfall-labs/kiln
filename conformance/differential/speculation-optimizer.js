// Probe for optimizer speculation and guard exit conformance (Step 2.2).
// Tests numeric loops, typed function calls, and fallback when non-numbers are passed.

function mathCalc(x, y, z) {
    let acc = x * 2.5 + y;
    for (let i = 0; i < 3; i++) {
        acc += i * 1.5;
    }
    acc = Math.abs(acc - z);
    acc = Math.min(acc, 50);
    return acc;
}

function polyAdd(a, b) {
    let acc = a;
    for (let i = 0; i < 2; i++) {
        acc = acc + b;
    }
    return acc;
}

// 1. Pure numeric calls (exercising guarded fast path / typed entry)
console.log("calc_num1:", mathCalc(2.0, 3.5, 1.0));
console.log("calc_num2:", mathCalc(10.0, 5.0, 4.0));
console.log("poly_num:", polyAdd(10.5, 2.5));

// 2. Deoptimization / guard exit calls (non-numeric parameters)
console.log("poly_str:", polyAdd("start_", "x"));
console.log("poly_null:", polyAdd(null, 5));
console.log("poly_bool:", polyAdd(true, 10));

// 3. Mixed calls
console.log("mixed:", polyAdd(mathCalc(1.0, 2.0, 0.5), 10));
