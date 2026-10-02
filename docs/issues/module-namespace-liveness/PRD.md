# PRD: Live Module Namespace Object Bindings

Status: resolved
Branch: `fix/module-namespace-liveness`

## Problem Statement

Named imports track exported module variables live, but module namespace objects (`import * as ns`) previously froze exported values at link time. Consequently, mutations in the exporting module were not reflected on `ns.counter`.

## Resolution

- Implemented "refresh-on-read" architecture: `ObjectData` maintains `namespace_cells: Vec<(String, Value)>`.
- Before reading a property from an object tracking namespace cells, `refresh_namespace()` updates property slots from the underlying cell values.
- Covered reflection (`Object.keys`, `Object.values`, `getOwnPropertyDescriptor`, spread, `for...in`) automatically without changing `Value::Cell` abstractions.
- Gained +74 Test262 rows in `language/expressions/dynamic-import`.
