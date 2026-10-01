// Function-declaration instantiation currently supports function-body-level declarations. A
// declaration nested in a block must fail explicitly until block binding semantics are modeled;
// it must not be evaluated as an expression and silently discarded.
Function("if (true) { function blockOnly() { return 1; } }")();
