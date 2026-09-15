import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { readFile } from "node:fs/promises";
import { createServer } from "node:http";
import process from "node:process";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import Ajv2020 from "ajv/dist/2020.js";
import addFormats from "ajv-formats";
import {
  FRONTEND_ACTIONS,
  endpointKind,
  networkFailure,
} from "./browser-frontend-health.mjs";

const schema = JSON.parse(
  await readFile(
    new URL("../schemas/frontend-health.schema.json", import.meta.url),
  ),
);
const ajv = new Ajv2020({ allErrors: true });
addFormats(ajv);
const validate = ajv.compile(schema);
const runner = fileURLToPath(new URL("./browser-runner.mjs", import.meta.url));
const secret = "secret-private-query-user-project";

test("endpoint categories and network causes discard URL secrets", () => {
  assert.deepEqual(
    endpointKind(
      `http://user:pass@host.lan:8000/Data/${secret}/deployment/web/a.js?q=${secret}#secret`,
      "http://localhost:8080",
    ),
    {
      scheme: "http",
      hostKind: "shared-unc",
      port: 8000,
      pathKind: "shared-deployment",
    },
  );
  assert.equal(
    endpointKind("https://example.invalid/private.css", "http://localhost")
      .pathKind,
    "stylesheet",
  );
  assert.equal(endpointKind("broken", "http://localhost").hostKind, "unknown");
  for (const [input, code] of [
    ["net::ERR_NAME_NOT_RESOLVED", "dns_failure"],
    ["net::ERR_CONNECTION_REFUSED", "connection_failure"],
    ["net::ERR_CERT_AUTHORITY_INVALID", "tls_failure"],
    ["net::ERR_FAILED", "request_failure"],
  ])
    assert.equal(networkFailure(input), code);
  assert.deepEqual(
    new Set(schema.properties.diagnostics.items.properties.code.enum),
    new Set(Object.keys(FRONTEND_ACTIONS)),
  );
});

// Real TCP/Chromium failures, with no route interception or host resolver rules.
// The optional binary runs these same pages through the public CLI boundary.
test(
  "ordinary Chromium and CLI separate HTTP readiness from frontend failures",
  { timeout: 120000 },
  async () => {
    const server = createServer((request, response) => {
      const pathname = new URL(request.url, "http://localhost").pathname;
      response.setHeader("Content-Type", "text/html; charset=utf-8");
      if (pathname === "/favicon.ico") {
        response.writeHead(204);
        response.end();
        return;
      }
      if (pathname === "/healthy")
        response.end(
          '<h1>Working app</h1><div role="dialog" hidden>An error occurred</div>',
        );
      else if (pathname === "/blank")
        response.end("<html><body></body></html>");
      else if (pathname === "/broken")
        response.end(
          `<h1>App</h1><div role="dialog">An error occurred, please contact your system administrator.</div><script type="module">import("/missing.js?token=${secret}"); throw Error("${secret}");</script><link rel="stylesheet" href="/widgets/com.mendix.css?token=${secret}">`,
        );
      else if (pathname === "/unc")
        response.end(
          `<h1>App</h1><script type="module">import("http://host.lan/Data/${secret}/deployment/web/widget.js");</script>`,
        );
      else if (pathname === "/dialog")
        response.end(`<h1>App</h1><script>alert("${secret}")</script>`);
      else if (pathname === "/transient")
        response.end(
          '<h1>App</h1><script>setTimeout(()=>{document.body.insertAdjacentHTML("beforeend", \'<div class="mx-dialog-error">Error</div>\');setTimeout(()=>document.querySelector(".mx-dialog-error").remove(),300)},50)</script>',
        );
      else if (pathname === "/flood")
        response.end(
          `<h1>App</h1><script>for(let n=0;n<11000;n++)console.error("${secret}")</script>`,
        );
      else if (pathname === "/pending")
        response.end(
          '<h1>Loading</h1><script>const script=document.createElement("script");script.src="/never.js";document.head.append(script)</script>',
        );
      else if (pathname === "/never.js" || pathname === "/timeout") return;
      else {
        response.writeHead(pathname === "/server-error" ? 503 : 404);
        response.end("Missing");
      }
    });
    await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
    const baseUrl = `http://127.0.0.1:${server.address().port}`;
    try {
      for (const [route, state, code] of [
        ["healthy", "healthy"],
        ["blank", "inconclusive", "observation_incomplete"],
        ["broken", "unhealthy", "mendix_error_dialog"],
        ["unc", "unhealthy", "shared_unc_asset_unreachable"],
        ["dialog", "unhealthy", "browser_dialog"],
        ["transient", "unhealthy", "mendix_error_dialog"],
        ["pending", "inconclusive", "observation_incomplete"],
        ["timeout", "inconclusive", "navigation_timeout"],
        ["flood", "unhealthy", "console_error"],
        ["server-error", "unhealthy", "http_failure"],
        ["not-found", "unhealthy", "http_failure"],
      ]) {
        process.stdout.write(`Observing ${route}\n`);
        const result = await invoke(`${baseUrl}/${route}`);
        const report = result.data;
        assert(
          validate(report),
          `${route}: ${ajv.errorsText(validate.errors)}`,
        );
        assert.equal(
          report.frontendState,
          state,
          `${route}: ${JSON.stringify(report)}`,
        );
        assert.equal(report.assetBypass, false);
        if (code)
          assert(
            report.diagnostics.some((d) => d.code === code),
            `${route}: ${JSON.stringify(report)}`,
          );
        assert(!JSON.stringify(result).includes(secret));
        assert(!JSON.stringify(report).includes(baseUrl));
        if (route === "broken") {
          assert.equal(report.documentStatus, 200);
          assert.equal(report.httpReady, true);
          assert(report.diagnostics.some((d) => d.code === "esm_failure"));
          assert(
            report.diagnostics.some(
              (d) => d.code === "mendix_widget_css_missing",
            ),
          );
        }
        if (route === "unc") assert.equal(report.diagnostics[0].code, code);
        if (route === "flood") {
          assert(report.truncated);
          assert(report.diagnostics.length <= 100);
        }
        if (route === "server-error") assert.equal(report.httpReady, false);
        if (route === "not-found") assert.equal(report.httpReady, true); // Preserved lightweight <500 contract.
        if (process.env.MENDIMARU_FRONTEND_TEST_BINARY)
          assert.equal(result.exitCode, state === "healthy" ? 0 : 1);
      }
    } finally {
      server.closeAllConnections();
      await new Promise((resolve) => server.close(resolve));
    }
  },
);

async function invoke(baseUrl) {
  const binary = process.env.MENDIMARU_FRONTEND_TEST_BINARY;
  const child = spawn(
    binary || process.execPath,
    binary
      ? [
          "browser",
          "frontend-health",
          "--base-url",
          baseUrl,
          "--navigation-timeout-ms",
          baseUrl.endsWith("/timeout") ? "200" : "5000",
          "--observation-ms",
          "700",
          "--json",
        ]
      : [runner, "frontend-health"],
    {
      env: {
        ...process.env,
        MENDIMARU_FRONTEND_REQUEST_JSON: JSON.stringify({
          baseUrl,
          navigationTimeoutMilliseconds: baseUrl.endsWith("/timeout")
            ? 200
            : 5000,
          observationMilliseconds: 700,
        }),
      },
      stdio: ["ignore", "pipe", "pipe"],
    },
  );
  let stdout = "",
    stderr = "";
  child.stdout.on("data", (bytes) => {
    stdout += bytes;
  });
  child.stderr.on("data", (bytes) => {
    stderr += bytes;
  });
  const exitCode = await new Promise((resolve, reject) => {
    child.on("error", reject);
    child.on("close", resolve);
  });
  assert.equal(stderr, "");
  const result = JSON.parse(stdout);
  assert.equal(result.ok, true, stdout);
  return {
    ...result,
    exitCode,
    data: {
      studioState: null,
      runtimeSessionId: null,
      httpReady:
        result.data.documentStatus === null
          ? null
          : result.data.documentStatus < 500,
      ...result.data,
    },
  };
}
