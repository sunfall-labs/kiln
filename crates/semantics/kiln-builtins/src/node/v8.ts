// kiln-builtins: node:v8 implementation (Phase 5.2, ADR 0005)

// The private object is shared with node:vm through the builtin graph's private read window.
// Neither the flag nor the request primitive is installed on the user global.
const kilnPrivate: any = (globalThis as any).__kilnPrivate ?? {};

export function cachedDataVersionTag(): number {
  return 1;
}

export function getHeapStatistics() {
  return {
    total_heap_size: 1024 * 1024 * 32,
    total_heap_size_executable: 1024 * 1024 * 4,
    total_physical_size: 1024 * 1024 * 32,
    total_available_size: 1024 * 1024 * 1024,
    used_heap_size: 1024 * 1024 * 16,
    heap_size_limit: 1024 * 1024 * 1024 * 2,
    malloced_memory: 1024 * 1024,
    peak_malloced_memory: 1024 * 1024 * 2,
    does_zap_garbage: 0,
    number_of_native_contexts: 1,
    number_of_detached_contexts: 0,
    total_global_handles_size: 0,
    used_global_handles_size: 0,
    external_memory: 0,
  };
}

export function getHeapSpaceStatistics(): any[] {
  return [];
}

export function getHeapCodeStatistics() {
  return {
    code_and_metadata_size: 1024 * 1024,
    bytecode_and_metadata_size: 512 * 1024,
    external_script_source_size: 0,
    cpu_profiler_metadata_size: 0,
  };
}

export function getCppHeapStatistics() {
  return {
    allocated_object_size: 0,
    size_of_global_handles: 0,
    used_heap_size: 0,
  };
}

export function setFlagsFromString(flags: string): void {
  for (const flag of String(flags).trim().split(/\s+/)) {
    if (flag === "--expose_gc" || flag === "--expose-gc") kilnPrivate.gcExposed = true;
    else if (flag === "--no-expose_gc" || flag === "--no-expose-gc") kilnPrivate.gcExposed = false;
  }
}

export function serialize(value: any): Uint8Array {
  return new TextEncoder().encode(JSON.stringify(value));
}

export function deserialize(buffer: Uint8Array): any {
  return JSON.parse(new TextDecoder().decode(buffer));
}

export class Serializer {
  private _val: any;
  writeHeader(): void {}
  writeValue(val: any): void {
    this._val = val;
  }
  writeRawBytes(_bytes: Uint8Array): void {}
  writeUint32(_value: number): void {}
  writeUint64(_hi: number, _lo: number): void {}
  writeDouble(_value: number): void {}
  releaseBuffer(): Uint8Array {
    return serialize(this._val);
  }
  transferArrayBuffer(_id: number, _arrayBuffer: ArrayBuffer): void {}
}

export class Deserializer {
  private _buf: Uint8Array;
  constructor(buffer: Uint8Array) {
    this._buf = buffer;
  }
  readHeader(): boolean {
    return true;
  }
  readValue(): any {
    return deserialize(this._buf);
  }
  readRawBytes(_length: number): Uint8Array {
    return new Uint8Array(0);
  }
  readUint32(): number {
    return 0;
  }
  readUint64(): [number, number] {
    return [0, 0];
  }
  readDouble(): number {
    return 0;
  }
  transferArrayBuffer(_id: number, _arrayBuffer: ArrayBuffer): void {}
  getWireFormatVersion(): number {
    return 15;
  }
}

export class DefaultSerializer extends Serializer {}
export class DefaultDeserializer extends Deserializer {}

export function takeCoverage(): void {}
export function stopCoverage(): void {}
export function writeHeapSnapshot(filename?: string): string {
  return filename || "heapdump.heapsnapshot";
}
export function getHeapSnapshot(): any {
  return null;
}

export const promiseHooks = {
  onInit(_promise: any, _parent: any): void {},
  onSettled(_promise: any): void {},
  onBefore(_promise: any): void {},
  onAfter(_promise: any): void {},
  createHook(_callbacks: any): any {
    return { enable(): void {}, disable(): void {} };
  },
};

export const startupSnapshot = {
  isBuildingSnapshot(): boolean {
    return false;
  },
  addSerializeCallback(_callback: () => void): void {},
  addDeserializeCallback(_callback: () => void): void {},
  setDeserializeMainFunction(_callback: () => void): void {},
};

export function setHeapSnapshotNearHeapLimit(_limit: number): void {}

export class GCProfiler {
  start(): void {}
  stop(): any {
    return { version: 1, startTime: 0, endTime: 0, statistics: [] };
  }
}

export function isStringOneByteRepresentation(_str: string): boolean {
  return true;
}

export function queryObjects(_constructor: any, _options?: any): number | any[] {
  return 0;
}

export function startCpuProfile(): void {}

const v8Module = {
  cachedDataVersionTag,
  getHeapSnapshot,
  getHeapStatistics,
  getHeapSpaceStatistics,
  getHeapCodeStatistics,
  getCppHeapStatistics,
  setFlagsFromString,
  Serializer,
  Deserializer,
  DefaultSerializer,
  DefaultDeserializer,
  deserialize,
  takeCoverage,
  stopCoverage,
  serialize,
  writeHeapSnapshot,
  promiseHooks,
  queryObjects,
  startupSnapshot,
  setHeapSnapshotNearHeapLimit,
  GCProfiler,
  isStringOneByteRepresentation,
  startCpuProfile,
  default: undefined as any,
};
v8Module.default = v8Module;

export default v8Module;
