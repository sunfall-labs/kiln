console.log((255).toString(16), (255).toString(2), (0.5).toString(2), (-10.25).toString(4), (0).toString(8));
console.log((1.005).toFixed(2), (1.5).toFixed(0), (1234.5678).toFixed(3), (0).toFixed(2), (1/0).toFixed(2));
console.log((42).valueOf(), (3.7).toString(), Number('12'), Number(''), Number('x'), Number(null), Number(true));
console.log(parseInt('12px'), parseInt('0x1f'), parseInt('ff', 16), parseInt('-42'), parseInt('x'), parseInt('  7 '));
console.log(parseFloat('1.5e3abc'), parseFloat('.5'), parseFloat('-Infinity'), parseFloat('x'), parseFloat('1e'));
console.log(isNaN('x'), Number.isNaN('x'), isFinite('1'), Number.isFinite('1'), Number.isInteger(5), Number.isSafeInteger(2**53));
console.log(Math.round(-1.5), Math.round(1.5), Math.round(-0.5), Math.sign(-0), Math.min(), Math.max(), Math.min(1,NaN));
