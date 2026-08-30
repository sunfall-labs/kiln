import { produce, enableMapSet } from "immer";
enableMapSet();
const base = { list: [1, 2, 3], nested: { flag: false } };
const next = produce(base, (draft) => {
  draft.list.push(4);
  draft.nested.flag = true;
});
console.log(JSON.stringify(next), JSON.stringify(base));
console.log(next.list !== base.list, next.nested !== base.nested);
