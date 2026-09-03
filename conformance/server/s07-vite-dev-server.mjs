import http from "node:http";

// Vite-like Dev Server:
// Demonstrates modern frontend dev-server architecture:
// 1. Virtual FS / module map
// 2. ESM import rewrite middleware (rewriting bare specifiers to /@modules/...)
// 3. Transform pipeline for CSS / JSON / JS
// 4. Client HMR ping route (/@vite/client)
// 5. Self-driving end-to-end verification

const virtualFiles = new Map([
  ["/index.html", {
    type: "text/html",
    content: `<!DOCTYPE html><html><head><script type="module" src="/src/main.js"></script></head><body><div id="app"></div></body></html>`
  }],
  ["/src/main.js", {
    type: "application/javascript",
    content: `import { ref } from "vue";\nimport "./style.css";\nimport config from "./config.json";\nconsole.log("app started", ref(config.version).value);`
  }],
  ["/src/style.css", {
    type: "text/css",
    content: `body { margin: 0; background: #1a1a1a; color: #ffffff; }`
  }],
  ["/src/config.json", {
    type: "application/json",
    content: JSON.stringify({ version: "1.0.0", name: "kiln-vite-demo" })
  }],
]);

function transformImports(source) {
  return source.replace(/from\s+["']([^"']+)["']/g, (match, specifier) => {
    if (!specifier.startsWith(".") && !specifier.startsWith("/")) {
      return `from "/@modules/${specifier}.js"`;
    }
    return match;
  });
}

function transform(url, file) {
  if (url === "/@vite/client") {
    return {
      status: 200,
      type: "application/javascript",
      content: `// [vite] client\nexport function createHotContext() { return { accept() {} }; }`
    };
  }
  if (url.startsWith("/@modules/")) {
    const pkg = url.slice("/@modules/".length).replace(/\.js$/, "");
    return {
      status: 200,
      type: "application/javascript",
      content: `// [vite] synthetic bundle for ${pkg}\nexport function ref(v) { return { value: v }; }`
    };
  }
  if (file && file.type === "application/javascript") {
    const transformed = transformImports(file.content);
    return {
      status: 200,
      type: "application/javascript",
      content: transformed
    };
  }
  if (file && file.type === "text/css") {
    const js = `const css = ${JSON.stringify(file.content)};\nconst style = document.createElement("style");\nstyle.textContent = css;\ndocument.head.appendChild(style);\nexport default css;`;
    return {
      status: 200,
      type: "application/javascript",
      content: js
    };
  }
  if (file && file.type === "application/json") {
    return {
      status: 200,
      type: "application/javascript",
      content: `export default ${file.content};`
    };
  }
  if (file) {
    return {
      status: 200,
      type: file.type,
      content: file.content
    };
  }
  return {
    status: 404,
    type: "text/plain",
    content: "Not Found"
  };
}

const server = http.createServer((req, res) => {
  const url = new URL(req.url, "http://127.0.0.1");
  const pathname = url.pathname === "/" ? "/index.html" : url.pathname;
  const file = virtualFiles.get(pathname);
  const result = transform(pathname, file);

  res.writeHead(result.status, {
    "content-type": result.type,
    "x-powered-by": "kiln-vite-engine"
  });
  res.end(result.content);
});

function request(port, path) {
  return new Promise((resolve) => {
    http.get({ host: "127.0.0.1", port, path }, (res) => {
      let body = "";
      res.setEncoding("utf8");
      res.on("data", (chunk) => { body += chunk; });
      res.on("end", () => {
        resolve({
          status: res.statusCode,
          type: res.headers["content-type"],
          poweredBy: res.headers["x-powered-by"],
          bodySnippet: body.slice(0, 40).replace(/\n/g, "\\n")
        });
      });
    });
  });
}

server.listen(0, "127.0.0.1", async () => {
  const { port } = server.address();
  const routes = [
    "/",
    "/@vite/client",
    "/src/main.js",
    "/@modules/vue.js",
    "/src/style.css",
    "/src/config.json",
    "/missing.js"
  ];

  for (const path of routes) {
    const res = await request(port, path);
    console.log(path, res.status, res.type, res.poweredBy, res.bodySnippet);
  }

  server.close();
});
