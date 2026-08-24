import { realpath } from "node:fs/promises";
import { performance } from "node:perf_hooks";

export async function sameWindowsPath(left, right) {
  const [canonicalLeft, canonicalRight] = await Promise.all([
    realpath(left),
    realpath(right),
  ]);
  return (
    canonicalLeft.replaceAll("/", "\\").toLowerCase() ===
    canonicalRight.replaceAll("/", "\\").toLowerCase()
  );
}

export async function waitForWebDriverSession({
  client,
  statusUrl,
  getEarlyExit,
  timeoutMs,
  onSessionAttempt = () => undefined,
  fetchImpl = fetch,
  pollIntervalMs = 250,
}) {
  const started = performance.now();
  let lastError;
  while (performance.now() - started < timeoutMs) {
    const earlyExit = getEarlyExit();
    if (earlyExit) {
      throw new Error(
        `tauri dev exited before WebDriver was ready (${JSON.stringify(earlyExit)})`,
      );
    }

    try {
      const response = await fetchImpl(statusUrl);
      if (response?.ok) {
        onSessionAttempt();
        await client.createSession();
        return;
      }
    } catch (error) {
      lastError = error;
    }
    await delay(pollIntervalMs);
  }
  throw new Error(
    `Timed out after ${timeoutMs} ms waiting for embedded WebDriver session for the main Tauri window${
      lastError ? `: ${lastError}` : ""
    }`,
  );
}

export function delay(milliseconds) {
  return new Promise((resolveDelay) => setTimeout(resolveDelay, milliseconds));
}
