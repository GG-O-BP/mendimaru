import { setTimeout as delay } from "node:timers/promises";

export const FRONTEND_ACTIONS = {
  shared_unc_asset_unreachable:
    "Check host.lan shared UNC imports. Use the opt-in generated-import repair described in docs/winboat-assets.md, then diagnose again; this result does not establish a broken widget.",
  dns_failure: "Check DNS resolution for the asset host from the browser host.",
  connection_failure:
    "Check the target port, Runtime forwarding and server availability.",
  tls_failure: "Check the server certificate and HTTPS configuration.",
  request_failure:
    "Check browser network access, CORS and mixed-content policy for this resource.",
  http_failure:
    "Check the HTTP status, asset deployment and access permissions.",
  mendix_widget_css_missing:
    "Check MPK CSS and generated CSS imports; see docs/widget-css-diagnostics.md. Do not create empty CSS or ignore the failure.",
  esm_failure:
    "Check failed module requests, JavaScript MIME types, CORS and generated imports.",
  page_error:
    "Inspect application client errors and the failed requests in this report.",
  console_error:
    "Inspect the application browser console and related failed requests.",
  mendix_error_dialog:
    "Inspect the related asset and module failures before diagnosing a widget defect.",
  browser_dialog:
    "Inspect the application's startup dialog in an ordinary browser.",
  navigation_timeout:
    "Check server response and startup resources, then retry with a longer navigation timeout.",
  observation_incomplete:
    "Loading or observation did not complete. Retry with a longer observation window and use a declarative browser suite to assert app-specific readiness.",
};

export function endpointKind(value, baseUrl, resourceType = "other") {
  try {
    const url = new URL(value);
    const base = new URL(baseUrl);
    const shared =
      url.hostname.toLowerCase() === "host.lan" &&
      /^\/Data\//i.test(url.pathname);
    return {
      scheme: ["http:", "https:"].includes(url.protocol)
        ? url.protocol.slice(0, -1)
        : "other",
      hostKind: shared
        ? "shared-unc"
        : url.origin === base.origin
          ? "same-origin"
          : ["localhost", "127.0.0.1", "[::1]"].includes(url.hostname)
            ? "loopback"
            : "external",
      port: Number(
        url.port ||
          (url.protocol === "https:" ? 443 : url.protocol === "http:" ? 80 : 0),
      ),
      pathKind: shared
        ? "shared-deployment"
        : /\.css$/i.test(url.pathname)
          ? "stylesheet"
          : /\.(?:m?js)$/i.test(url.pathname)
            ? "script"
            : resourceType === "document"
              ? "document"
              : "other",
    };
  } catch {
    return { scheme: "other", hostKind: "unknown", port: 0, pathKind: "other" };
  }
}

export function networkFailure(text) {
  if (/ERR_NAME_NOT_RESOLVED|ERR_NAME_RESOLUTION_FAILED/.test(text))
    return "dns_failure";
  if (
    /ERR_CONNECTION_|ERR_ADDRESS_|ERR_NETWORK_CHANGED|ERR_EMPTY_RESPONSE/.test(
      text,
    )
  )
    return "connection_failure";
  if (/ERR_CERT_|ERR_SSL_/.test(text)) return "tls_failure";
  return "request_failure";
}

export async function diagnoseFrontend(chromium, request) {
  const { baseUrl, navigationTimeoutMilliseconds, observationMilliseconds } =
    request;
  const url = new URL(baseUrl);
  if (
    !["http:", "https:"].includes(url.protocol) ||
    url.username ||
    url.password ||
    url.search ||
    url.hash ||
    baseUrl.length > 4096 ||
    !Number.isInteger(navigationTimeoutMilliseconds) ||
    navigationTimeoutMilliseconds < 100 ||
    navigationTimeoutMilliseconds > 30000 ||
    !Number.isInteger(observationMilliseconds) ||
    observationMilliseconds < 100 ||
    observationMilliseconds > 10000 ||
    Object.keys(request).some(
      (key) =>
        ![
          "baseUrl",
          "navigationTimeoutMilliseconds",
          "observationMilliseconds",
        ].includes(key),
    )
  ) {
    throw new Error("invalid frontend request");
  }
  const startedAt = new Date().toISOString();
  const findings = new Map();
  const counts = {
    pageErrors: 0,
    consoleErrors: 0,
    failedRequests: 0,
    httpErrors: 0,
    errorDialogs: 0,
  };
  let observing = true;
  let truncated = false;
  let navigationComplete = false;
  let documentStatus = null;
  let contentVisible = false;
  let pendingAssets = 0;
  const count = (key) => {
    if (!observing) return;
    if (counts[key] < 10000) counts[key]++;
    else truncated = true;
  };
  const add = (code, details = {}) => {
    if (!observing) return;
    const key = JSON.stringify([code, details]);
    const previous = findings.get(key);
    if (previous) {
      if (previous.occurrences < 10000) previous.occurrences++;
      else truncated = true;
    } else if (findings.size < 100) {
      findings.set(key, {
        code,
        action: FRONTEND_ACTIONS[code],
        occurrences: 1,
        ...details,
      });
    } else truncated = true;
  };
  const esm = (text) =>
    /dynamically imported module|module script|Failed to load module|Importing a module script|error loading dynamically imported/i.test(
      text,
    );
  let browser;
  try {
    browser = await chromium.launch({ headless: true, timeout: 10000 });
    // Fresh browser context, ordinary network resolution: no mirror, routes,
    // hosts overrides, stored credentials, project writes or Studio calls.
    const context = await browser.newContext({ acceptDownloads: false });
    const page = await context.newPage();
    page.setDefaultTimeout(1000);
    const critical = (request) =>
      ["script", "stylesheet", "document"].includes(request.resourceType());
    context.on("request", (request) => {
      if (critical(request)) pendingAssets++;
    });
    context.on("requestfinished", (request) => {
      if (critical(request)) pendingAssets = Math.max(0, pendingAssets - 1);
    });
    context.on("requestfailed", (request) => {
      if (critical(request)) pendingAssets = Math.max(0, pendingAssets - 1);
      count("failedRequests");
      const endpoint = endpointKind(
        request.url(),
        baseUrl,
        request.resourceType(),
      );
      const failure = networkFailure(request.failure()?.errorText ?? "");
      add(
        endpoint.hostKind === "shared-unc"
          ? "shared_unc_asset_unreachable"
          : failure,
        { endpoint, failure },
      );
    });
    context.on("response", (response) => {
      if (
        response.request().isNavigationRequest() &&
        response.frame() === page.mainFrame()
      )
        documentStatus = response.status();
      if (response.status() < 400) return;
      count("httpErrors");
      const endpoint = endpointKind(
        response.url(),
        baseUrl,
        response.request().resourceType(),
      );
      const missingCss =
        response.status() === 404 &&
        /\/widgets\/(?:com\.)?mendix\.css$/.test(
          new URL(response.url()).pathname,
        );
      add(
        endpoint.hostKind === "shared-unc"
          ? "shared_unc_asset_unreachable"
          : missingCss
            ? "mendix_widget_css_missing"
            : "http_failure",
        { endpoint, failure: "http_failure", status: response.status() },
      );
    });
    context.on("weberror", (event) => {
      count("pageErrors");
      add(
        esm(event.error().message.slice(0, 8192))
          ? "esm_failure"
          : "page_error",
      );
    });
    context.on("console", (message) => {
      if (message.type() !== "error") return;
      count("consoleErrors");
      add(esm(message.text().slice(0, 8192)) ? "esm_failure" : "console_error");
    });
    context.on("dialog", (dialog) => {
      count("errorDialogs");
      add("browser_dialog");
      void dialog.dismiss().catch(() => {});
    });
    page.on("crash", () => add("observation_incomplete"));
    try {
      const response = await page.goto(baseUrl, {
        waitUntil: "domcontentloaded",
        timeout: navigationTimeoutMilliseconds,
      });
      documentStatus = response?.status() ?? null;
      navigationComplete = response !== null;
    } catch (error) {
      add(
        error.name === "TimeoutError"
          ? "navigation_timeout"
          : networkFailure(error.message),
      );
    }
    let dialogObserved = false;
    const deadline = Date.now() + observationMilliseconds;
    while (navigationComplete && Date.now() < deadline) {
      try {
        await Promise.race([
          (async () => {
            const errorDialog = page
              .locator(
                '.mx-dialog-error:visible, .mx-error:visible, [role="alertdialog"]:visible',
              )
              .first();
            const genericDialog = page
              .locator('[role="dialog"]:visible, .modal-dialog:visible')
              .filter({
                hasText:
                  /an error occurred|system administrator|오류가 발생|시스템 관리자|エラーが発生|システム管理者/i,
              })
              .first();
            if (
              !dialogObserved &&
              ((await errorDialog.count()) > 0 ||
                (await genericDialog.count()) > 0)
            ) {
              dialogObserved = true;
              count("errorDialogs");
              add("mendix_error_dialog");
            }
            contentVisible = await page.locator("body").evaluate(
              (body) => {
                const bounds = body.getBoundingClientRect();
                const style =
                  body.ownerDocument.defaultView.getComputedStyle(body);
                return (
                  bounds.width > 0 &&
                  bounds.height > 0 &&
                  style.visibility !== "hidden" &&
                  style.display !== "none" &&
                  (body.innerText.trim().length > 0 ||
                    body.querySelector("input, button, canvas, img, svg") !==
                      null)
                );
              },
              undefined,
              { timeout: 500 },
            );
          })(),
          delay(600).then(() => {
            throw new Error("observation timeout");
          }),
        ]);
      } catch {
        add("observation_incomplete");
        break;
      }
      await delay(Math.min(100, Math.max(0, deadline - Date.now())));
    }
    if (
      !navigationComplete ||
      !contentVisible ||
      pendingAssets > 0 ||
      truncated
    )
      add("observation_incomplete");
  } finally {
    observing = false;
    await browser?.close().catch(() => {});
  }
  const diagnostics = [...findings.values()].sort((a, b) =>
    a.code === "shared_unc_asset_unreachable"
      ? -1
      : b.code === "shared_unc_asset_unreachable"
        ? 1
        : a.code.localeCompare(b.code),
  );
  const unhealthy = diagnostics.some(
    ({ code }) =>
      !["observation_incomplete", "navigation_timeout"].includes(code),
  );
  return {
    schemaVersion: "5.0.0",
    startedAt,
    finishedAt: new Date().toISOString(),
    frontendState: unhealthy
      ? "unhealthy"
      : diagnostics.length
        ? "inconclusive"
        : "healthy",
    navigationComplete,
    documentStatus,
    observationMilliseconds,
    assetBypass: false,
    counts,
    truncated,
    diagnostics,
  };
}
