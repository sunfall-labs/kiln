/*---
description: unsupported language features stay visible
flags: [raw]
---*/
class FixtureWithPrivateAccessorProperty {
  accessor #value = 1;
}
