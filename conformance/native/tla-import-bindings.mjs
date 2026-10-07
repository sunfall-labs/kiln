import { ImportedRecord, importedValue } from './fixtures/tla-imports/barrel.mjs';

console.log(typeof ImportedRecord, typeof importedValue);
await Promise.resolve();
const record = new ImportedRecord(importedValue());
console.log(record.value, record instanceof ImportedRecord);
