console.log("small:", { a: 1, b: 2 });
console.log("nested:", { outer: { value: "a value long enough to make this nested object wrap when combined with its parent" } });

class SemVer {
  constructor() {
    this.options = {};
    this.loose = false;
    this.includePrerelease = false;
    this.raw = "3.4.0";
    this.major = 3;
    this.minor = 4;
    this.patch = 0;
    this.prerelease = [];
    this.build = [];
    this.version = "3.4.0";
  }
}
console.log(new SemVer());
