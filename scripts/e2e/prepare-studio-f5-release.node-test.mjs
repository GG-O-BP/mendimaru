import { execFile } from "node:child_process";
import process from "node:process";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";
import { test } from "node:test";

const execute = promisify(execFile);

test(
  "generic Studio release preparation selects exact identities and cleans up after failure",
  { skip: process.platform !== "linux", timeout: 30_000 },
  async () => {
    await execute(
      "python3",
      [
        fileURLToPath(
          new URL("prepare-studio-f5-release.test.py", import.meta.url),
        ),
      ],
      {
        env: { ...process.env, PYTHONDONTWRITEBYTECODE: "1" },
        timeout: 25_000,
      },
    );
  },
);
