class BaseField {
  static get min() { throw new Error("base static min must not run"); }
  static get constraints() {
    if (this !== DerivedField) throw new Error("inherited static getter lost its receiver");
    return { min: this.min };
  }
  get min() { return this.constructor.min; }
}
class DerivedField extends BaseField {
  static get min() {
    if (this !== DerivedField) throw new Error("own static getter lost its receiver");
    return 0;
  }
}
const instance = new DerivedField();
if (instance.constructor !== DerivedField) throw new Error("derived instance constructor was lost");
if (instance.min !== 0) throw new Error("derived instance getter used the base constructor");
if (DerivedField.constraints.min !== 0) throw new Error("inherited static getter returned the wrong min");
console.log("derived-static-accessors:ok");
