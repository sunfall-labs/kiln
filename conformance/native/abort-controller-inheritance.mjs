import process from 'node:process';
class Controller extends AbortController {}
const controller = new Controller();
console.log(controller instanceof Controller, controller instanceof AbortController);
console.log(Object.getPrototypeOf(controller) === Controller.prototype);
console.log(Object.prototype.toString.call(controller));
console.log(Object.keys(controller).join(','));
console.log(Object.getOwnPropertyNames(controller).join(','));
console.log(Object.getOwnPropertyNames(AbortController.prototype).join(','));
console.log(AbortController.length, AbortController.prototype.abort.length);
const signal = controller.signal;
console.log(signal === controller.signal, signal.aborted);
let count = 0;
signal.addEventListener('abort', () => count++);
const reason = { reason: 'abort' };
controller.abort(reason);
controller.abort('again');
console.log(signal.aborted, signal.reason === reason, count);
for (const operation of [
  () => AbortController(),
  () => AbortController.prototype.abort.call({ signal }),
  () => Object.getOwnPropertyDescriptor(AbortController.prototype, 'signal').get.call({}),
]) {
  try { operation(); console.log('unexpected success'); } catch (error) { console.log(error instanceof TypeError); }
}
const originalGet = WeakMap.prototype.get;
WeakMap.prototype.get = () => undefined;
console.log(controller.signal === signal);
WeakMap.prototype.get = originalGet;
console.log(process.version.length > 0);
