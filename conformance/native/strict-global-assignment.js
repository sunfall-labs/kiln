// Strict code refuses to *create* a global by assignment and does not refuse to write one, and
// which of the two an unbound name is can only be answered while the program runs: a global
// installed a line earlier is an ordinary write, and one global code declared with `var` is too.
// Both engines have to decide it at the same moment, or `"use strict"` code throws under one and
// succeeds under the other. A module body is strict without saying so, which is why this probe
// needs no directive.
globalThis.planted = 1;

function write() {
  planted = 2;
  return planted;
}
console.log('write:', write());

function update() {
  planted += 1;
  return planted;
}
console.log('update:', update());

function refuse() {
  try {
    neverInstalled = 3;
    return 'no throw';
  } catch (error) {
    return error.constructor.name;
  }
}
console.log('refuse:', refuse());

function refuseUpdate() {
  try {
    alsoNeverInstalled += 1;
    return 'no throw';
  } catch (error) {
    return error.constructor.name;
  }
}
console.log('refuse update:', refuseUpdate());
