import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// @ts-expect-error process is a nodejs global
const host = process.env.TAURI_DEV_HOST;
const developmentCsp = [
  "default-src 'self' http://localhost:1420",
  "script-src 'self' 'unsafe-inline' http://localhost:1420",
  "connect-src 'self' ipc: http://ipc.localhost http://localhost:1420 ws://localhost:1420 ws://localhost:1421",
  "img-src 'self' data: blob: http://localhost:1420",
  "style-src 'self' 'unsafe-inline' http://localhost:1420",
  "font-src 'self' data: http://localhost:1420",
  "object-src 'none'",
  "base-uri 'none'",
  "form-action 'none'",
  "frame-ancestors 'none'",
].join("; ");

// https://vite.dev/config/
export default defineConfig(async () => ({
  plugins: [
    {
      name: "mendimaru-development-csp",
      apply: "serve",
      transformIndexHtml: {
        order: "pre",
        handler: () => [
          {
            tag: "meta",
            attrs: {
              "http-equiv": "Content-Security-Policy",
              content: developmentCsp,
            },
            injectTo: "head-prepend",
          },
        ],
      },
    },
    react(),
  ],

  // Vite options tailored for Tauri development and only applied in `tauri dev` or `tauri build`
  //
  // 1. prevent Vite from obscuring rust errors
  clearScreen: false,
  // 2. tauri expects a fixed port, fail if that port is not available
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    headers: {
      "Content-Security-Policy": developmentCsp,
    },
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1421,
        }
      : undefined,
    watch: {
      // 3. tell Vite to ignore watching `src-tauri`
      ignored: ["**/src-tauri/**"],
    },
  },
}));
