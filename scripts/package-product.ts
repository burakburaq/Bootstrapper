import { spawnSync } from "node:child_process";
import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { resolve } from "node:path";

const root = fileURLToPath(new URL("../", import.meta.url));
const product = JSON.parse(
  await readFile(resolve(root, "build/package-inputs.json"), "utf8"),
);
const result = spawnSync(
  "cargo",
  [
    "run",
    "--manifest-path",
    resolve(root, "updates/Cargo.toml"),
    "--locked",
    "-p",
    "release-cli",
    "--",
    "windows-package",
    "--app-id",
    product.appId,
    "--product-directory",
    product.productDirectory,
    "--executable",
    product.executable,
    "--display-name",
    product.displayName,
    "--publisher",
    product.publisher,
    ...process.argv.slice(2),
  ],
  { stdio: "inherit", shell: false },
);
if (result.error) throw result.error;
if (result.status !== 0)
  throw new Error(`NSIS product preparation failed (${result.status})`);
