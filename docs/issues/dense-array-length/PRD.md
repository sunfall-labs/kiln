# PRD: Array Length Limits in Dense Array Storage

Status: wontfix
Branch: `none`

## Problem Statement

`ArrayData` keeps elements in `items: RefCell<Vec<Value>>` and derives `length` from `items.borrow().len()`. A length beyond `MAX_DENSE_ARRAY` (such as `2**32 - 2`) cannot be represented, causing `array_set_length` to refuse with:
`array of length 4294967294 exceeds this implementation's dense representation`.

## Rationale for Wontfix

No real-world program constructs a four-billion-element dense array. Separating `length` from dense storage requires decoupling 30+ hot paths in both interpreter and C native backends, risking substantial regressions in the hottest engine structure for zero real-world benefit.
