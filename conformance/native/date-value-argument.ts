// `new Date(object)` converts its single argument via `ToPrimitive(..., "default")`,
// distinguishing between date strings and numeric instants only after the reduction.
// The field form (two or more arguments) converts arguments left-to-right via `ToNumber`.

// 1. Symbol.toPrimitive receives the "default" hint
let hintSeen = "none";
const spyPrimitive: any = {
    [Symbol.toPrimitive](hint: string) {
        hintSeen = hint;
        return 5000;
    },
};
const d1 = new Date(spyPrimitive);
console.log(hintSeen, d1.getTime());

// 2. Symbol.toPrimitive returning a date string parses as a date
const spyStringPrimitive: any = {
    [Symbol.toPrimitive](_hint: string) {
        return "2024-01-01T00:00:00.000Z";
    },
};
const d2 = new Date(spyStringPrimitive);
console.log(d2.toISOString());

// 3. Plain valueOf returning a number
let valueOfCalls = 0;
const spyValueOf: any = {
    valueOf() {
        valueOfCalls++;
        return 123456789;
    },
};
const d3 = new Date(spyValueOf);
console.log(valueOfCalls, d3.getTime());

// 4. Fallback to toString when valueOf is not defined
const spyToString: any = {
    toString() {
        return "2025-06-15T12:00:00.000Z";
    },
};
const d4 = new Date(spyToString);
console.log(d4.toISOString());

// 5. Existing Date object is copied without calling ToPrimitive
const original = new Date(987654321);
const copy = new Date(original);
console.log(copy.getTime(), copy === original);

// 6. Multi-argument field form converts left-to-right
const order: string[] = [];
const y: any = { valueOf() { order.push("y"); return 2026; } };
const m: any = { valueOf() { order.push("m"); return 7; } };
const dt: any = { valueOf() { order.push("dt"); return 29; } };
const h: any = { valueOf() { order.push("h"); return 12; } };
const min: any = { valueOf() { order.push("min"); return 34; } };
const s: any = { valueOf() { order.push("s"); return 56; } };
const ms: any = { valueOf() { order.push("ms"); return 789; } };
const dField = new Date(y, m, dt, h, min, s, ms);
console.log(order.join(","), dField.getTime());

// 7. Multi-argument default fields (year and month specified; day defaults to 1, rest to 0)
const dDefaults = new Date(2024, 0);
console.log(dDefaults.getUTCFullYear(), dDefaults.getUTCMonth(), dDefaults.getUTCDate(), dDefaults.getUTCHours());

// 8. Trailing arguments beyond 7 are also reduced left-to-right
const extraOrder: string[] = [];
const e1: any = { valueOf() { extraOrder.push("e1"); return 2024; } };
const e2: any = { valueOf() { extraOrder.push("e2"); return 0; } };
const e3: any = { valueOf() { extraOrder.push("e3"); return 1; } };
const e4: any = { valueOf() { extraOrder.push("e4"); return 0; } };
const e5: any = { valueOf() { extraOrder.push("e5"); return 0; } };
const e6: any = { extra: 1 };
const e7: any = { valueOf() { extraOrder.push("e7"); return 0; } };
const e8: any = { valueOf() { extraOrder.push("e8"); return 0; } };
new Date(e1, e2, e3, e4, e5, e6, e7, e8);
console.log(extraOrder.join(","));

// 9. Symbol and BigInt type error handling
try {
    new Date(Symbol() as any);
} catch (e: any) {
    console.log(e.name, e.message);
}

try {
    new Date(10n as any);
} catch (e: any) {
    console.log(e.name, e.message);
}

try {
    new Date(2024, Symbol() as any);
} catch (e: any) {
    console.log(e.name, e.message);
}

try {
    new Date({
        valueOf() {
            throw new Error("custom error in valueOf");
        },
    } as any);
} catch (e: any) {
    console.log(e.name, e.message);
}

try {
    new Date(Object.create(null));
} catch (e: any) {
    console.log(e.name, e.message);
}
