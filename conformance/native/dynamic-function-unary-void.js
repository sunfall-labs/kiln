const evaluate = new Function("state", "state.count++; return void state.count;");
const state = { count: 0 };
const result = evaluate(state);

console.log(JSON.stringify({ resultType: typeof result, count: state.count }));
