import react from "@vitejs/plugin-react";
import { defineConfig } from "vitest/config";

export default defineConfig({
  plugins: [react()],
  test: {
    environment: "jsdom",
    setupFiles: ["./src/test/setup.ts"],
    clearMocks: true,
    pool: "threads",
    // Avoid sporadic worker-start timeouts after a full Rust build on Windows.
    maxWorkers: 2,
  },
});
