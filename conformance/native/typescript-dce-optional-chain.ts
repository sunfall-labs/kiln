let calls = 0;
function argument(): number {
  calls += 1;
  return 5;
}

function invoke(value: { child: unknown }): number | undefined {
  return (value.child as { base: number; method?: (this: { base: number }, n: number) => number } | null)
    ?.method?.(argument());
}

const receiver = {
  base: 7,
  method(this: { base: number }, n: number) {
    return this.base + n;
  },
};
console.log(invoke({ child: null }), invoke({ child: receiver }), calls);
