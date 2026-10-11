// The WHATWG platform globals (fetch, streams, atob...) are installed only when a native program
// can reach them. Sloppy code reaches the global object as `this`, so a property name alone must
// keep them -- this file names none of them as a global.
function platform() {
  return [typeof this.btoa, typeof this.ReadableStream, typeof this.Response, typeof this.EventTarget];
}
console.log(platform().join(" "));
