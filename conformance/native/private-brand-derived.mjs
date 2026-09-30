import { PrivateBrandBase } from './private-brand-base.mjs';

// This class owns no private names. Its compiler still allocates a class-local scope, which used
// to reuse the base module's scope 0 and overwrite the base class's brand property.
export class EmptyPrivateBrandChild extends PrivateBrandBase {}

// Private names in separately compiled base/derived modules must also remain distinct even when
// their class-local ordinal and spelling match.
export class SameNamePrivateBrandChild extends PrivateBrandBase {
  #value = 11;
  readChild() { return this.#value; }
}
