function assignAfterInitialization() {
  value = 1;
}
function updateAfterInitialization() {
  value++;
}

let effects = 0;
let assignmentCaught = false;
let updateCaught = false;
let readCaught = false;
function assignBeforeInitialization() {
  try {
    later = (++effects, 1);
  } catch (error) {
    if (!(error instanceof ReferenceError)) throw error;
    assignmentCaught = true;
  }
}
function updateBeforeInitialization() {
  try {
    later++;
  } catch (error) {
    if (!(error instanceof ReferenceError)) throw error;
    updateCaught = true;
  }
}
function readBeforeInitialization() {
  try {
    return later;
  } catch (error) {
    if (!(error instanceof ReferenceError)) throw error;
    readCaught = true;
  }
}
function readAfterInitialization() {
  return later;
}

let value = 0;
assignAfterInitialization();
updateAfterInitialization();
if (value !== 2) throw new Error('function compiled before module binding failed after init');
assignBeforeInitialization();
updateBeforeInitialization();
readBeforeInitialization();
let later = 7;
if (!assignmentCaught || !updateCaught || !readCaught || effects !== 1)
  throw new Error('early module TDZ operations had incorrect ordering');
if (readAfterInitialization() !== 7)
  throw new Error('late module read failed after initialization');
console.log('tdz-module-write-order:ok', value, later, effects);
