# Worker deploy acceptance

This app pins pino and piscina's real worker paths. Run from this directory:

```sh
npm ci --ignore-scripts
node pino.cjs
node piscina.cjs
kiln run --one-shot --node pino.cjs
kiln run --one-shot --node piscina.cjs
kiln build --node --output pino-native pino.cjs && ./pino-native
kiln build --node --output piscina-native piscina.cjs && ./piscina-native
```

The expected stdout is `{"level":30,"answer":42,"msg":"worker transport"}` for pino
and `42` for piscina. `kiln.include` declares the worker scripts and the runtime targets
they load. The parent must not evaluate these modules at startup.
