import { copyFile, cp, mkdir } from "node:fs/promises";

// TypeScript emits app.js first. Only runtime assets enter the native WebView.
const source = new URL("../ui/", import.meta.url);
const output = new URL("../build/ui/", import.meta.url);
await mkdir(output, { recursive: true });
await Promise.all([
  copyFile(new URL("index.html", source), new URL("index.html", output)),
  copyFile(new URL("styles.css", source), new URL("styles.css", output)),
  cp(new URL("assets/", source), new URL("assets/", output), {
    recursive: true,
  }),
]);
