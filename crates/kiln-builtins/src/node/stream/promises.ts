import { pipeline as cbPipeline, finished as cbFinished } from "node:stream";

export function finished(stream: any, options?: any): Promise<void> {
  return new Promise((resolve, reject) => {
    cbFinished(stream, (err?: any) => {
      if (err) reject(err);
      else resolve();
    });
  });
}

export function pipeline(...streams: any[]): Promise<void> {
  return new Promise((resolve, reject) => {
    cbPipeline(...streams, (err?: any) => {
      if (err) reject(err);
      else resolve();
    });
  });
}

const promisesModule = {
  finished,
  pipeline,
  default: undefined as any,
};
promisesModule.default = promisesModule;

export default promisesModule;
