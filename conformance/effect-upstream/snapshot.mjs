import {format} from '@vitest/pretty-format';
import {stripSnapshotIndentation} from '@vitest/snapshot';
import {expect} from './harness.mjs';
expect.extend({toMatchInlineSnapshot(received, expected) {
  if(typeof expected!=='string')throw new Error('HARNESS_UNSUPPORTED: inline snapshot without explicit string');
  const formatted=format(received,{printBasicPrototype:false,escapeString:false});
  const actual=formatted.includes('\n')?'\n'+formatted+'\n':formatted;
  const normalized=stripSnapshotIndentation(expected);
  return {pass:actual===normalized,message:()=>`Inline snapshot differs\nexpected: ${normalized}\nactual: ${actual}`};
}});
