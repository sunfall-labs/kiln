export function makeModuleMatch() {
  return /([a-z]+)-(\d+)/.test("module-81");
}

export function readModuleCaptures() {
  return [RegExp.$1, RegExp.$2].join(":");
}
