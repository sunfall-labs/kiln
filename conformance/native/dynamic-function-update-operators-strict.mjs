const namedUpdate = function namedUpdate() {
  try {
    eval("++namedUpdate");
  } catch (error) {
    return error instanceof TypeError;
  }
  return false;
};

if (!namedUpdate()) {
  throw new Error("strict module eval rejects updating a named function binding");
}

console.log("strict named function update throws");
