import * as namespace from './generator-namespace-provider.ts'

function *argument() {
  yield 'argument'
  return 3
}

function *outer() {
  return yield* namespace.step(yield* argument())
}

const iterator = outer()
console.log(JSON.stringify([iterator.next(), iterator.next(), iterator.next()]))
