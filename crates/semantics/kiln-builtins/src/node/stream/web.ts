export const ReadableStream = (globalThis as any).ReadableStream;
export const ReadableStreamDefaultReader = (globalThis as any).ReadableStreamDefaultReader;
export const ReadableStreamBYOBReader = (globalThis as any).ReadableStreamBYOBReader;
export const ReadableStreamDefaultController = (globalThis as any).ReadableStreamDefaultController;
export const ReadableByteStreamController = (globalThis as any).ReadableByteStreamController;
export const ReadableStreamBYOBRequest = (globalThis as any).ReadableStreamBYOBRequest;
export const WritableStream = (globalThis as any).WritableStream;
export const WritableStreamDefaultWriter = (globalThis as any).WritableStreamDefaultWriter;
export const WritableStreamDefaultController = (globalThis as any).WritableStreamDefaultController;
export const TransformStream = (globalThis as any).TransformStream;
export const TransformStreamDefaultController = (globalThis as any).TransformStreamDefaultController;
export const ByteLengthQueuingStrategy = (globalThis as any).ByteLengthQueuingStrategy;
export const CountQueuingStrategy = (globalThis as any).CountQueuingStrategy;
export const TextEncoderStream = (globalThis as any).TextEncoderStream;
export const TextDecoderStream = (globalThis as any).TextDecoderStream;
export const CompressionStream = (globalThis as any).CompressionStream;
export const DecompressionStream = (globalThis as any).DecompressionStream;

const streamWeb = {
  ByteLengthQueuingStrategy,
  CompressionStream,
  CountQueuingStrategy,
  DecompressionStream,
  ReadableByteStreamController,
  ReadableStream,
  ReadableStreamBYOBReader,
  ReadableStreamBYOBRequest,
  ReadableStreamDefaultController,
  ReadableStreamDefaultReader,
  TextDecoderStream,
  TextEncoderStream,
  TransformStream,
  TransformStreamDefaultController,
  WritableStream,
  WritableStreamDefaultController,
  WritableStreamDefaultWriter,
  default: undefined as any,
};
streamWeb.default = streamWeb;

export default streamWeb;
