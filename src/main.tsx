import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";

if (import.meta.env.DEV && import.meta.env.VITE_MENDIMARU_E2E === "1") {
  const testWindow = window as Window & {
    __MENDIMARU_CSP_PROBE__?: Promise<boolean>;
  };
  testWindow.__MENDIMARU_CSP_PROBE__ = new Promise((resolve) => {
    let complete = false;
    const finish = (blocked: boolean) => {
      if (complete) return;
      complete = true;
      resolve(blocked);
    };
    window.__mendimaruCspProbe = false;
    const script = document.createElement("script");
    script.src = "data:text/javascript,window.__mendimaruCspProbe=true";
    script.addEventListener("load", () => finish(false), { once: true });
    script.addEventListener("error", () => finish(true), { once: true });
    document.head.appendChild(script);
    window.setTimeout(() => finish(window.__mendimaruCspProbe !== true), 1_000);
  });
}

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
