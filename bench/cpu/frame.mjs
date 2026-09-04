import { bench } from "../harness.mjs";

function plain() { return arguments.length + ":" + arguments[0]; }
function viaRest(...rest) { return rest.join(","); }
function outer() { const arrow = () => arguments[0] + "/" + arguments.length; return arrow(); }
function Ctor() { return new.target === undefined ? "call" : "new"; }
function nested() { function inner() { return arguments[1]; } return inner("a", "b"); }
function deep(n) { return n === 0 ? arguments.length : deep(n - 1); }

// Each call is cheap, so a single pass is far below the clock's resolution; the batch
// calibration in the harness is what makes this measurable rather than a repeat count here.
bench("frame", () => [plain(7, 8), viaRest(1, 2, 3), outer("x", "y"), Ctor(), new Ctor(), nested(), deep(3)].join(" "));
