import { dlopen } from "node:ffi";

const opened = dlopen("libc.so.6", {
  strlen: { arguments: ["string"], return: "u64" },
});

console.log(opened.functions.strlen("hello"));
opened.lib.close();
