// kiln npm functional test: rxjs
const { of, from, Subject, BehaviorSubject, ReplaySubject } = require('rxjs');
const { map, filter, mergeMap, concatMap, catchError, retry, take, toArray, finalize, tap } = require('rxjs');

// 1. BehaviorSubject & ReplaySubject state retention
const bs = new BehaviorSubject('initial');
const bsLog = [];
bs.subscribe(v => bsLog.push('sub1:' + v));
bs.next('second');
bs.subscribe(v => bsLog.push('sub2:' + v));
console.log('bs-current-value:', bs.getValue());
console.log('bs-log:', JSON.stringify(bsLog));

const rs = new ReplaySubject(2);
rs.next('r1');
rs.next('r2');
rs.next('r3');
const rsLog = [];
rs.subscribe(v => rsLog.push(v));
console.log('rs-replay-last-two:', JSON.stringify(rsLog));

// 2. Higher-order flattening: concatMap and mergeMap
const outConcat = [];
of(1, 2, 3)
  .pipe(
    concatMap(n => of(n * 10, n * 10 + 1)),
    toArray()
  )
  .subscribe(arr => outConcat.push(...arr));
console.log('concatMap-order:', JSON.stringify(outConcat));

const outMerge = [];
of('a', 'b')
  .pipe(
    mergeMap(letter => of(letter + '1', letter + '2')),
    toArray()
  )
  .subscribe(arr => outMerge.push(...arr));
console.log('mergeMap-results:', JSON.stringify(outMerge));

// 3. Error recovery with catchError and retry
const attempts = [];
let errorCount = 0;
of(1, 2, 3)
  .pipe(
    map(n => {
      attempts.push(n);
      if (n === 2 && errorCount === 0) {
        errorCount++;
        throw new Error('transient');
      }
      return n * 100;
    }),
    retry(1),
    catchError(err => of(999)),
    toArray()
  )
  .subscribe(res => console.log('retry-and-recovered:', JSON.stringify(res), 'attempts:', JSON.stringify(attempts)));

// 4. Stream combination and early termination with finalize
const finalizeLog = [];
of(10, 20, 30, 40, 50)
  .pipe(
    take(3),
    finalize(() => finalizeLog.push('cleanup-done')),
    toArray()
  )
  .subscribe(res => {
    console.log('take-3:', JSON.stringify(res));
  });
console.log('finalize:', JSON.stringify(finalizeLog));
