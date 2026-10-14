import { copyFile, mkdir, readFile, rm } from "node:fs/promises";

// TypeScript emits app.js first. Only runtime assets enter the native WebView.
const source = new URL("../ui/", import.meta.url);
const output = new URL("../build/ui/", import.meta.url);
await mkdir(output, { recursive: true });
// Keep source artwork, but package only assets referenced by the runtime UI.
const references = await Promise.all(
  ["index.html", "styles.css", "app.ts"].map((file) =>
    readFile(new URL(file, source), "utf8"),
  ),
);
const assets = new Set(
  references.flatMap((text) =>
    Array.from(text.matchAll(/assets\/([\w.-]+)/g), (match) => match[1]),
  ),
);
await rm(new URL("assets/", output), { recursive: true, force: true });
await mkdir(new URL("assets/", output), { recursive: true });
await Promise.all([
  copyFile(new URL("index.html", source), new URL("index.html", output)),
  copyFile(new URL("styles.css", source), new URL("styles.css", output)),
  ...Array.from(assets, (asset) =>
    copyFile(
      new URL(`assets/${asset}`, source),
      new URL(`assets/${asset}`, output),
    ),
  ),
]);
