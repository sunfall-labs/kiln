import * as namespace from '../module-namespace-inherited-tdz.mjs';

let observed = 'no error';
try {
  Object.create(namespace).value;
} catch (error) {
  observed = error.name;
}

export { observed };
