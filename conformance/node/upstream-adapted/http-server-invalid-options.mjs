import http from 'node:http';

if (typeof http.Server !== 'function') {
  throw new Error('node:http did not export Server');
}

new http.Server(undefined, () => {});

for (const option of ['foo', 42, true, []]) {
  let thrown;
  try {
    new http.Server(option);
  } catch (error) {
    thrown = error;
  }
  if (!(thrown instanceof TypeError)) {
    throw new Error('http.Server did not throw TypeError for an invalid option');
  }
  if (thrown.code !== 'ERR_INVALID_ARG_TYPE') {
    throw new Error('http.Server did not expose ERR_INVALID_ARG_TYPE');
  }
}

console.log('pass');
