// Public formatter objects and parts are TypeScript; option resolution and calendar/zone output
// come from the same kiln-intl engine as the interpreter.
const date = Date.UTC(2024, 0, 2, 3, 4, 5);
const formatter = new Intl.DateTimeFormat("en-US", {
  timeZone: "UTC", weekday: "long", year: "numeric", month: "long", day: "numeric",
  hour: "2-digit", minute: "2-digit", second: "2-digit", timeZoneName: "short",
});
console.log(formatter.format(date));
console.log(formatter.formatToParts(date).map((part) => `${part.type}=${part.value}`).join("|"));
console.log(formatter.resolvedOptions());
console.log(new Intl.DateTimeFormat("de-DE", { timeZone: "UTC", dateStyle: "medium" }).format(date));
