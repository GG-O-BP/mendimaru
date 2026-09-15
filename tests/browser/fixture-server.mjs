import http from "node:http";
import { Buffer } from "node:buffer";
import process from "node:process";
import { setTimeout } from "node:timers";
import { URL } from "node:url";

const page = `<!doctype html>
<html lang="en">
<head><meta charset="utf-8"><title>Mendix fixture</title></head>
<body>
  <main>
    <section id="login-panel">
      <h1>Fixture sign in</h1>
      <form id="login-form">
        <label>Username <input id="username" autocomplete="username"></label>
        <label>Password <input id="password" type="password" autocomplete="current-password"></label>
        <button type="submit">Sign in</button>
      </form>
      <button type="button" data-testid="cross-origin">Cross origin</button>
    </section>
    <section id="app-panel" hidden>
      <h1>Task board</h1>
      <p id="private-echo" data-testid="private-output"></p>
      <label>Task <input class="mx-name-TaskInput"></label>
      <button class="mx-name-AddTask" type="button">Add task</button>
      <label><input class="mx-name-TaskDone" type="checkbox"> Done</label>
      <p role="status" aria-label="Result">Waiting</p>
      <button type="button" data-testid="console-error">Console error</button>
      <button type="button" data-testid="page-error">Page error</button>
      <button type="button" data-testid="network-error">Network error</button>
    </section>
  </main>
  <script>
    const loginPanel = document.querySelector('#login-panel');
    const appPanel = document.querySelector('#app-panel');
    const result = document.querySelector('[role="status"]');
    document.querySelector('#login-form').addEventListener('submit', event => {
      event.preventDefault();
      const username = document.querySelector('#username').value;
      const password = document.querySelector('#password').value;
      if (!username || !password) return;
      document.querySelector('#private-echo').textContent = password;
      loginPanel.hidden = true;
      appPanel.hidden = false;
    });
    document.querySelector('.mx-name-AddTask').addEventListener('click', async () => {
      const task = document.querySelector('.mx-name-TaskInput').value;
      const response = await fetch('/api/save', { method: 'POST', body: task });
      if (response.ok) result.textContent = 'Saved: ' + task;
    });
    document.querySelector('[data-testid="console-error"]').addEventListener('click', () => {
      console.error('fixture console failure');
    });
    document.querySelector('[data-testid="page-error"]').addEventListener('click', () => {
      setTimeout(() => {
        result.textContent = 'Page error observed';
        throw new Error('fixture uncaught failure');
      }, 0);
    });
    document.querySelector('[data-testid="network-error"]').addEventListener('click', async () => {
      await fetch('/api/fail');
      result.textContent = 'Network handled';
    });
    document.querySelector('[data-testid="cross-origin"]').addEventListener('click', () => {
      globalThis.location.href = 'about:blank';
    });
  </script>
</body>
</html>`;

const compressiblePagePrefix = `<!doctype html>
<html lang="en">
<head><meta charset="utf-8"><title>Compressible trace fixture</title></head>
<body>
  <p role="status" aria-label="Download">Loading</p>
  <div hidden>`;
const compressiblePageSuffix = `</div>
  <script>
    document.querySelector('[role="status"]').textContent = 'Loaded';
    setTimeout(() => { throw new Error('compressible trace fixture failure'); }, 0);
  </script>
</body>
</html>`;
const COMPRESSIBLE_RESPONSE_BYTES = 36 * 1024 * 1024;
const COMPRESSIBLE_CHUNK = Buffer.alloc(64 * 1024, 0x61);

const server = http.createServer((request, response) => {
  const url = new URL(request.url, "http://127.0.0.1");
  if (url.pathname === "/widget-css-fixture") {
    const kind = url.searchParams.get("kind");
    const target =
      kind === "present"
        ? "/present/dist/widgets.css"
        : kind === "unavailable"
          ? "/unavailable/dist/widgets.css"
          : kind === "unrelated"
            ? "/other.css"
            : kind === "nested"
              ? "/app/dist/widgets.css"
              : "/dist/widgets.css";
    const href =
      kind === "foreign"
        ? `http://localhost:${server.address().port}${target}`
        : target;
    response.writeHead(200, { "content-type": "text/html; charset=utf-8" });
    response.end(`<!doctype html><link rel="icon" href="data:,"><h1>Loading</h1>
      <script>
        const done = () => { document.querySelector('h1').textContent = 'Loaded'; };
        ${
          kind === "fetch"
            ? `fetch('${href}').then(done);`
            : `
        const css = document.createElement('link');
        css.rel = 'stylesheet'; css.href = '${href}?build-stamp=private-stamp';
        css.onload = css.onerror = done; document.head.append(css);`
        }
      </script>`);
    return;
  }
  if (url.pathname === "/present/dist/widgets.css") {
    response.writeHead(200, { "content-type": "text/css" });
    response.end("h1 { color: rgb(17, 34, 51); }");
    return;
  }
  if (url.pathname === "/unavailable/dist/widgets.css") {
    response.writeHead(503, { "content-type": "text/plain" });
    response.end("unavailable");
    return;
  }
  if (url.pathname === "/api/save" && request.method === "POST") {
    request.resume();
    response.writeHead(200, { "content-type": "application/json" });
    response.end('{"saved":true}');
    return;
  }
  if (url.pathname === "/api/fail") {
    response.writeHead(503, { "content-type": "text/plain" });
    response.end("fixture failure");
    return;
  }
  if (url.pathname === "/") {
    response.writeHead(200, { "content-type": "text/html; charset=utf-8" });
    response.end(page);
    return;
  }
  if (url.pathname === "/slow") {
    setTimeout(() => {
      if (response.destroyed) return;
      response.writeHead(200, { "content-type": "text/html; charset=utf-8" });
      response.end("<!doctype html><title>Delayed fixture</title>");
    }, 1_500);
    return;
  }
  if (url.pathname === "/compressible") {
    response.writeHead(200, {
      "content-length":
        Buffer.byteLength(compressiblePagePrefix) +
        COMPRESSIBLE_RESPONSE_BYTES +
        Buffer.byteLength(compressiblePageSuffix),
      "content-type": "text/html; charset=utf-8",
    });
    response.write(compressiblePagePrefix);
    streamCompressibleResponse(response, COMPRESSIBLE_RESPONSE_BYTES, () =>
      response.end(compressiblePageSuffix),
    );
    return;
  }
  if (url.pathname === "/storage-auth") {
    const authenticated = (request.headers.cookie || "")
      .split(";")
      .some((cookie) => cookie.trim().startsWith("fixture_auth="));
    response.writeHead(authenticated ? 200 : 401, {
      "content-type": "text/html; charset=utf-8",
    });
    response.end(
      authenticated
        ? "<!doctype html><h1>Storage authenticated</h1>"
        : "<!doctype html><h1>Authentication required</h1>",
    );
    return;
  }
  response.writeHead(404, { "content-type": "text/plain" });
  response.end("not found");
});

function streamCompressibleResponse(response, totalBytes, complete) {
  let remaining = totalBytes;
  const write = () => {
    while (remaining > 0 && !response.destroyed) {
      const bytes = Math.min(remaining, COMPRESSIBLE_CHUNK.length);
      remaining -= bytes;
      if (!response.write(COMPRESSIBLE_CHUNK.subarray(0, bytes))) {
        response.once("drain", write);
        return;
      }
    }
    if (!response.destroyed) complete();
  };
  write();
}

const requestedPort = Number.parseInt(process.argv[2] ?? "", 10);
const listenPort =
  Number.isInteger(requestedPort) && requestedPort > 0 ? requestedPort : 0;
server.listen(listenPort, "127.0.0.1", () => {
  const address = server.address();
  process.stdout.write(`${JSON.stringify({ port: address.port })}\n`);
});

for (const signal of ["SIGINT", "SIGTERM"]) {
  process.on(signal, () => server.close(() => process.exit(0)));
}
