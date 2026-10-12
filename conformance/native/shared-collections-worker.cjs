// A sealed Map or Set crosses to a worker by reference (ADR 0073): the worker reads the same
// entries through its own realm's Map.prototype, and an echo comes back as the same object.
const { Worker, isMainThread, parentPort, workerData } = require('node:worker_threads');
const { share, isShared } = require('kiln:shared');
if (isMainThread) {
  const rates = share(new Map([['acme', 2], ['globex', 3]]));
  const tags = share(new Set(['a', 'b']));
  const orders = share(Array.from({ length: 1000 }, (_, i) => ({ customer: i % 3 ? 'globex' : 'acme', amount: i % 100 })));
  Map.prototype.parentMarker = 41;
  const worker = new Worker(__filename, { workerData: { rates, tags, orders } });
  worker.on('message', (message) => {
    console.log('worker', message.report.join(' '));
    console.log('echo', message.rates === rates, message.tags === tags, rates.get('acme'), rates.parentMarker, rates.childMarker);
  });
  worker.on('error', (error) => console.log('error', error.message));
  worker.on('exit', (code) => console.log('exit', code));
} else {
  const { rates, tags, orders } = workerData;
  Map.prototype.childMarker = 53;
  let revenue = 0;
  for (const order of orders) revenue += order.amount * (rates.get(order.customer) ?? 1);
  let refused = 'wrote';
  try { rates.set('acme', 9); } catch (error) { refused = error.message; }
  parentPort.postMessage({
    rates,
    tags,
    report: [isShared(rates), rates instanceof Map, tags instanceof Set, rates.size, tags.has('b'),
      revenue, rates.childMarker, rates.parentMarker, [...rates.keys()].join('+'), refused],
  });
}
