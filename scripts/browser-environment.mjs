// The private observer endpoint only supplies bounded, allowlisted host evidence.
import { setTimeout as delay } from "node:timers/promises";

export function observerUrl(value) {
  if (value === undefined) return undefined;
  if (
    typeof value !== "string" ||
    !/^http:\/\/127\.0\.0\.1:[1-9][0-9]{0,4}\/observation_[a-f0-9]{32}$/.test(
      value,
    )
  ) {
    throw new Error("invalid environment observer endpoint");
  }
  const url = new URL(value);
  if (Number(url.port) > 65535)
    throw new Error("invalid environment observer endpoint");
  return value;
}

export class EnvironmentMonitor {
  constructor(url, sessionId) {
    this.url = observerUrl(url);
    this.sessionId = sessionId;
    this.report = undefined;
    this.stopSignal = new AbortController();
    this.changed = new Promise((resolve) => {
      this.signalChange = resolve;
    });
  }
  get interrupted() {
    return Boolean(this.report?.events.length);
  }
  async sample() {
    if (!this.url) return;
    try {
      const response = await fetch(this.url, {
        signal: AbortSignal.timeout(6000),
        redirect: "error",
      });
      if (!response.ok) throw new Error();
      const chunks = [];
      let bytes = 0;
      for await (const chunk of response.body) {
        bytes += chunk.byteLength;
        if (bytes > 512 * 1024) throw new Error();
        chunks.push(chunk);
      }
      const buffer = new Uint8Array(bytes);
      let offset = 0;
      for (const chunk of chunks) {
        buffer.set(chunk, offset);
        offset += chunk.byteLength;
      }
      const report = JSON.parse(new TextDecoder().decode(buffer));
      if (
        report.version !== 1 ||
        !Array.isArray(report.events) ||
        report.events.length > 7 ||
        report.actor !== "unknown"
      )
        throw new Error();
      if (this.report && report.preparationId !== this.report.preparationId)
        throw new Error();
      // Losing observations is sticky: a resumed endpoint is a new preparation.
      if (!this.unavailable) this.report = report;
    } catch {
      this.unavailable = true;
      const now = new Date().toISOString();
      const snapshot = {
        observedAt: now,
        managedVm: null,
        managementGeneration: null,
        container: null,
        containerRunning: null,
        compose: null,
        publishedPorts: null,
        runtime: null,
        studio: null,
        build: null,
        buildWatchGeneration: null,
      };
      this.report ??= {
        version: 1,
        preparationId: this.sessionId.replace("session_", "preparation_"),
        baseline: snapshot,
        latest: snapshot,
        observations: 1,
        events: [],
        comparable: false,
        missing: [
          "managed-vm",
          "container",
          "compose",
          "published-ports",
          "runtime",
          "studio",
          "build",
        ],
        actor: "unknown",
        intervalMilliseconds: 1000,
      };
      this.report.latest = snapshot;
      this.report.observations += 1;
      if (!this.report.events.some((e) => e.component === "managed-vm"))
        this.report.events.push({
          classification: "observation-unavailable",
          component: "managed-vm",
          observation: snapshot,
        });
      this.report.comparable = false;
    }
    if (this.interrupted) this.signalChange();
  }
  async start() {
    if (!this.url) return;
    await this.sample();
    this.worker = (async () => {
      while (!this.stopSignal.signal.aborted) {
        try {
          await delay(1000, undefined, { signal: this.stopSignal.signal });
        } catch {
          break;
        }
        await this.sample();
      }
    })();
  }
  async finish() {
    if (!this.url) return;
    this.stopSignal.abort();
    await this.worker;
    await this.sample();
  }
  async step(work) {
    if (this.interrupted)
      throw new Error(
        "environment observation interrupted this test; see environment.events",
      );
    return Promise.race([
      work(),
      this.changed.then(() => {
        throw new Error(
          "environment observation interrupted this test; see environment.events",
        );
      }),
    ]);
  }
}
