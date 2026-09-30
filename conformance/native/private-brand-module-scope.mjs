import {
  EmptyPrivateBrandChild,
  SameNamePrivateBrandChild,
} from './private-brand-derived.mjs';

const empty = new EmptyPrivateBrandChild();
const sameName = new SameNamePrivateBrandChild();
console.log(empty.readBase());
console.log(sameName.readBase(), sameName.readChild());
