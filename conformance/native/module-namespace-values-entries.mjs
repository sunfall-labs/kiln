import * as namespace from './fixtures/module-namespace-values-entries-source.mjs';
import { update } from './fixtures/module-namespace-values-entries-source.mjs';

function check(expected) {
    const keys = Object.keys(namespace);
    const ownKeys = Reflect.ownKeys(namespace);
    const values = Object.values(namespace);
    const entries = Object.entries(namespace);

    if (keys.length !== 2 || keys.join(',') !== 'current,update' ||
        new Set(keys).size !== keys.length) {
        throw new Error(`namespace keys are not unique: ${keys}`);
    }
    if (ownKeys.length !== 3 || new Set(ownKeys).size !== ownKeys.length ||
        ownKeys[0] !== 'current' || ownKeys[1] !== 'update' ||
        ownKeys[2] !== Symbol.toStringTag) {
        throw new Error(`namespace own keys are not unique and sorted: ${ownKeys}`);
    }
    if (values.length !== 2 || values[0] !== expected || values[1] !== update) {
        throw new Error(`Object.values did not read live exports: ${values}`);
    }
    if (entries.length !== 2 || entries[0][0] !== 'current' ||
        entries[0][1] !== expected || entries[1][0] !== 'update' ||
        entries[1][1] !== update) {
        throw new Error(`Object.entries did not read live exports: ${entries}`);
    }
    console.log(keys.join(','), values[0], entries[0][1] === expected);
}

check(1);
update(42);
check(42);
