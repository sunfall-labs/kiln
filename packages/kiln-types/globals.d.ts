// Hand-written: the globals kiln installs as extensions (`Kiln`, `Bun`), declared in
// `kiln_builtin_surface::KILN_EXTENSIONS` and defined by crates/semantics/kiln-builtins/src/web.ts
// (`serve`). A test in kiln-type-checker holds this file to that registry in both directions.
// Not generated: `serve` is installed by property definition, which carries no static types.

/** What `serve` returns: the listening server (the same object `node:http`'s `createServer` makes). */
interface KilnServer {
  /** The address being listened on, once listening; `port: 0` picks a free one. */
  address(): { address: string; family: string; port: number } | string | null;
  close(callback?: (error?: Error) => void): this;
  on(event: string, listener: (...args: any[]) => void): this;
  once(event: string, listener: (...args: any[]) => void): this;
}

interface KilnServeOptions {
  /** Called once per request; the Response is sent back whole. */
  fetch(request: Request): Response | Promise<Response>;
  /** Defaults to 3000. */
  port?: number;
  hostname?: string;
}

/** `Kiln.serve` and `Bun.serve`: one implementation, so programs written against Bun run unchanged. */
interface KilnServeApi {
  serve(options: KilnServeOptions | ((request: Request) => Response | Promise<Response>), callback?: () => void): KilnServer;
}

declare var Kiln: KilnServeApi;
declare var Bun: KilnServeApi;
