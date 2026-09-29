/** Ambient types for Kiln's engine-owned actor extension. Include this package in `types`. */
declare module "kiln:actors" {
  export abstract class Actor {}

  declare const actorAddressBrand: unique symbol;
  declare const isolatedBrand: unique symbol;
  declare const transferBrand: unique symbol;

  export type ActorAddress<T extends Actor = Actor> = Readonly<{
    readonly [actorAddressBrand]: T;
  }>;
  export type Isolated<T> = { readonly [isolatedBrand]: T };
  export type TransferTicket<T> = { readonly [transferBrand]: T };

  type Methods<T> = {
    [K in keyof T & string]: T[K] extends (...args: any[]) => any ? K : never;
  }[keyof T & string];

  export interface ActorRef<T extends Actor> {
    readonly address: ActorAddress<T>;
    call<K extends Methods<T>>(
      method: K,
      args: T[K] extends (...args: infer A) => any ? A : never,
      options?: { signal?: AbortSignal },
    ): Promise<T[K] extends (...args: any[]) => infer R ? Awaited<R> : never>;
  }

  export interface ActorOwner<T extends Actor> {
    readonly ref: ActorRef<T>;
    stop(options?: { force?: boolean }): Promise<void>;
  }

  export function spawnActor<T extends Actor>(options: {
    module: string | URL;
    export: string;
    args?: unknown[];
    mailboxCapacity?: number;
  }): Promise<ActorOwner<T>>;
  export function connectActor<T extends Actor>(address: ActorAddress<T>): ActorRef<T>;
  export function isolate<T>(factory: () => T): Isolated<T>;
  export function prepareTransfer<T>(value: T): Isolated<T>;
  export function transfer<T>(value: Isolated<T>): TransferTicket<T>;
  export function transfer(value: ArrayBuffer): TransferTicket<ArrayBuffer>;
}
