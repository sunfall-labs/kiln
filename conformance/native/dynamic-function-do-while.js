const runOnce = Function("events", `
  var count = 0;
  do { events.push("body"); count++; } while (false);
  return count;
`);
const onceEvents = [];
if (runOnce(onceEvents) !== 1 || onceEvents.join(",") !== "body") {
  throw new Error("do-while executes the body once before testing its condition");
}

const continued = Function("events", `
  var index = 0;
  do {
    events.push(index++);
    if (index < 3) continue;
  } while (index < 3);
  return index;
`);
const continueEvents = [];
if (continued(continueEvents) !== 3 || continueEvents.join(",") !== "0,1,2") {
  throw new Error("do-while continue evaluates the condition before repeating");
}

const breakSkipsTest = Function(`
  var tested = false;
  do { break; } while ((tested = true));
  return tested;
`);
if (breakSkipsTest() !== false) throw new Error("do-while break skips its condition");

const labeled = Function(`
  var count = 0;
  outer: do {
    count++;
    if (count < 3) continue outer;
    break outer;
  } while (true);
  return count;
`);
if (labeled() !== 3) throw new Error("do-while consumes its labeled continue and break");

const stackedLabels = Function(`
  var count = 0;
  outer: inner: do {
    count++;
    if (count < 2) continue outer;
  } while (count < 3);
  return count;
`);
if (stackedLabels() !== 3) {
  throw new Error("stacked labels on do-while remain iteration targets");
}

const nestedContinue = Function(`
  var count = 0, innerTests = 0, outerTests = 0;
  outer: do {
    count++;
    inner: do { continue outer; } while ((innerTests++, true));
  } while ((outerTests++, count < 3));
  return [count, innerTests, outerTests];
`);
if (nestedContinue().join(",") !== "3,0,3") {
  throw new Error("a nested do-while propagates continue to the owning outer condition");
}

console.log("dynamic do-while:", onceEvents.join(","), continueEvents.join(","),
  breakSkipsTest(), labeled(), stackedLabels());
