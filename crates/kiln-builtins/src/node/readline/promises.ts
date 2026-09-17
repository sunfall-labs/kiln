import { Interface as BaseInterface, type ReadLineOptions } from "node:readline";

export class Interface extends BaseInterface {
  question(query: string): Promise<string> {
    return Promise.resolve("");
  }
}

export function createInterface(options: ReadLineOptions | any): Interface {
  return new Interface(options);
}

const promisesModule = {
  Interface,
  createInterface,
  default: undefined as any,
};
promisesModule.default = promisesModule;

export default promisesModule;
