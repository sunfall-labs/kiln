import * as spy from '@vitest/spy';
import {installVi} from './harness.mjs';
const host = globalThis.__kilnEffectHarnessHost;
delete globalThis.__kilnEffectHarnessHost;

const nativeModules = globalThis.process?.__kilnNativeModules;
const nativeLoader = nativeModules?.__kilnHostModuleLoader;
function callNativeLoader(...args) { return nativeModules.__kilnHostModuleLoader(...args); }
const adapterReferrer = import.meta.url;
if (typeof nativeLoader === 'function') callNativeLoader('registerSpyAdapter', adapterReferrer);
const namespaceFacades = new WeakMap();
function moduleNamespaceFacade(namespace) {
  let facade = namespaceFacades.get(namespace);
  if (facade) return facade;
  facade = new Proxy(Object.create(null), {
    get(_target, key) { return Reflect.get(namespace, key, namespace); },
    has(_target, key) { return Reflect.has(namespace, key); },
    ownKeys() { return Reflect.ownKeys(namespace); },
    getOwnPropertyDescriptor(_target, key) {
      const descriptor = Reflect.getOwnPropertyDescriptor(namespace, key);
      if (!descriptor) return undefined;
      return {
        configurable: true,
        enumerable: descriptor.enumerable,
        writable: true,
        value: Reflect.get(namespace, key, namespace),
      };
    },
    defineProperty(_target, key, descriptor) {
      if (typeof key !== 'string' || !Reflect.has(namespace, key)
          || !Object.prototype.hasOwnProperty.call(descriptor, 'value')
          || typeof nativeLoader !== 'function') return false;
      if (callNativeLoader('spyExport', namespace, key, descriptor.value, adapterReferrer) !== true) return false;
      return true;
    },
  });
  namespaceFacades.set(namespace, facade);
  return facade;
}
function nativeSpyOn(target, key, accessor) {
  if (typeof nativeLoader === 'function' && target
      && target[Symbol.toStringTag] === 'Module'
      && typeof key === 'string'
      && (accessor === undefined || accessor === 'value')) {
    return spy.spyOn(moduleNamespaceFacade(target), key, accessor);
  }
  return spy.spyOn(target, key, accessor);
}
const spyOn = host?.spy?.spyOn ?? nativeSpyOn;
installVi({...spy,...host?.spy,spyOn,resetModules(){throw new Error('HARNESS_UNSUPPORTED: vi.resetModules');},importActual(){throw new Error('HARNESS_UNSUPPORTED: vi.importActual');}});
