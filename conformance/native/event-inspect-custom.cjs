// `Event.prototype[util.inspect.custom]`: an event shows as its class and four fields, on one
// line only when they fit in 80 columns; and a console prints what an object's custom inspection
// answers. (Timestamps are node's clock, so they are replaced before printing.)
const util = require("util");
const shown = (value) => util.inspect(value).replace(/timeStamp: [\d.e+-]+/, "timeStamp: T");
const event = new Event("ping", { cancelable: true });
event.preventDefault();
console.log(typeof Event.prototype[util.inspect.custom]);
console.log(shown(event));
console.log(shown(new CustomEvent("it's", { detail: 1 })));
class Tick extends Event {}
console.log(shown(new Tick("a\nb")));
console.log(new MessageEvent("message")[util.inspect.custom](-1, {}));
console.log(new Event("x")[util.inspect.custom](2, { breakLength: 200 }).replace(/timeStamp: [\d.e+-]+/, "timeStamp: T"));
console.log({ [util.inspect.custom]: (depth) => "custom at depth " + depth });
console.log("beside", { [util.inspect.custom]: () => "custom" });
