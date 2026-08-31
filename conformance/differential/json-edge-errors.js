// JSON.parse follows JSON's grammar, not JavaScript's numeric/string literal grammar, and
// JSON.stringify rejects BigInt values with a catchable TypeError.
function caught(label, run) {
	try {
		console.log(label, run());
	} catch (error) {
		console.log(label, error.constructor.name, error instanceof SyntaxError, error instanceof TypeError);
	}
}

caught('leading-plus', () => JSON.parse('+1'));
caught('raw-control', () => JSON.parse('"\x01"'));
caught('bigint', () => JSON.stringify(1n));
console.log('continued');
