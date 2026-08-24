import react from "@vitejs/plugin-react";
import { defineConfig } from "vitest/config";

export default defineConfig({
  plugins: [react()],
  test: {
    environment: "jsdom",
    setupFiles: ["./src/test/setup.ts"],
    clearMocks: true,
    pool: "threads",
    // Large native builds can starve concurrent Windows worker startup.
    maxWorkers: 1,
  },
});
