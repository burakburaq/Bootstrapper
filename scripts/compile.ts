import { spawnSync } from "node:child_process";
import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { resolve } from "node:path";

const root = fileURLToPath(new URL("../", import.meta.url));
const args = process.argv.slice(2);
const env = { ...process.env };
let prepareOnly = false;
for (let index = 0; index < args.length; index++) {
  if (args[index] === "--prepare-only") {
    prepareOnly = true;
    continue;
  }
  if (!["--product", "--trust"].includes(args[index]) || !args[index + 1])
    throw new Error(
      "Usage: npm run compile -- --product FILE --trust FILE [--prepare-only]",
    );
  const name =
    args[index++] === "--product"
      ? "TALAYLABS_INSTALLER_PRODUCT"
      : "TALAYLABS_INSTALLER_TRUST";
  env[name] = resolve(args[index]);
}
function run(program: string, args: string[]) {
  const result = spawnSync(program, args, {
    cwd: root,
    env,
    stdio: "inherit",
    shell: false,
  });
  if (result.error) throw result.error;
  if (result.status !== 0)
    throw new Error(`${program} failed (${result.status})`);
}
run(process.execPath, [
  resolve(root, "node_modules/typescript/bin/tsc"),
  "-p",
  "tsconfig.json",
]);
run(process.execPath, ["scripts/build.ts"]);
if (!prepareOnly) {
  env.TAURI_CONFIG = await readFile(
    resolve(root, "build/tauri-config.json"),
    "utf8",
  );
  run("cargo", [
    "build",
    "--manifest-path",
    "src-tauri/Cargo.toml",
    "--release",
    "--locked",
  ]);
}
