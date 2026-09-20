// The environment `resources/testharness.js` expects from a JavaScript shell, for
// `scripts/wpt.py`. Prepended to every test file it assembles; runs under `--node`.
//
// testharness.js picks its environment by looking at the global object: no `document`, no
// worker scope, so it settles on `ShellTestEnvironment`, which needs `self` and reports through
// `add_completion_callback`. The tests themselves reach for `location` (the subset helpers read
// `location.search` for a variant) and `fetch` their fixtures relative to it, which a browser's
// WPT server would answer; here those relative fetches are answered from the checkout on disk,
// and everything else goes to the real `fetch`. `__KILN_WPT` is filled in by the runner.
(function () {
  const G = globalThis;
  const cfg = G.__KILN_WPT;
  G.self = G;
  G.GLOBAL = {
    isWindow: () => false,
    isWorker: () => false,
    isShadowRealm: () => false,
  };
  const href = "http://web-platform.test:8000" + cfg.path + cfg.query;
  G.location = {
    href,
    origin: "http://web-platform.test:8000",
    protocol: "http:",
    host: "web-platform.test:8000",
    hostname: "web-platform.test",
    port: "8000",
    pathname: cfg.path,
    search: cfg.query,
    hash: "",
    toString: () => href,
  };
  const fs = cfg.fs;
  const path = cfg.pathModule;
  const realFetch = G.fetch;
  G.fetch = function (input, init) {
    const url = typeof input === "string" ? input : input && input.url;
    let resolved;
    try {
      resolved = new URL(String(url), href);
    } catch {
      return realFetch.call(G, input, init);
    }
    if (resolved.origin !== "http://web-platform.test:8000") {
      return typeof realFetch === "function"
        ? realFetch.call(G, input, init)
        : Promise.reject(new TypeError("fetch failed"));
    }
    const file = path.join(cfg.root, decodeURIComponent(resolved.pathname));
    return new Promise((resolve, reject) => {
      let bytes;
      try {
        bytes = fs.readFileSync(file);
      } catch (error) {
        resolve(new Response("not found: " + resolved.pathname, { status: 404 }));
        return;
      }
      const type = file.endsWith(".json") ? "application/json" : file.endsWith(".js") ? "text/javascript" : "application/octet-stream";
      resolve(new Response(bytes, { status: 200, headers: { "content-type": type } }));
    });
  };
})();
