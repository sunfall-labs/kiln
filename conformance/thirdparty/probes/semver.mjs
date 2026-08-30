import semver from "semver";
console.log(semver.valid("1.2.3"), semver.gt("2.0.0", "1.9.9"));
console.log(semver.satisfies("1.2.3", "^1.0.0"), semver.coerce("v3.1").version);
console.log(semver.sort(["2.0.0", "1.0.0", "1.5.0"]).join(","));
