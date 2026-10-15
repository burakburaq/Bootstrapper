import { copyFile, mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { resolve } from "node:path";
import sharp from "sharp";
import { escapeHtml, loadProduct, validateTrust } from "./product.ts";

const root = fileURLToPath(new URL("../", import.meta.url));
const productPath = resolve(
  process.env.TALAYLABS_INSTALLER_PRODUCT ||
    resolve(root, "examples/demo/product.json"),
);
const { product, iconPath } = await loadProduct(productPath);
const trustPath = resolve(
  process.env.TALAYLABS_INSTALLER_TRUST ||
    process.env.BAYKUS_INSTALLER_TRUST ||
    resolve(root, "examples/demo/trust.json"),
);
const trust = validateTrust(
  JSON.parse(await readFile(trustPath, "utf8")),
  product,
);
const output = resolve(root, "build");
const ui = resolve(output, "ui");
await mkdir(ui, { recursive: true });
const sourceFiles = await Promise.all(
  ["index.html", "styles.css", "app.ts"].map((name) =>
    readFile(resolve(root, "ui", name), "utf8"),
  ),
);
const html = sourceFiles[0].replaceAll(
  "{{DISPLAY_NAME}}",
  escapeHtml(product.displayName),
);
const assets = new Set(
  sourceFiles
    .flatMap((text) =>
      Array.from(text.matchAll(/assets\/([\w.-]+)/g), (match) => match[1]),
    )
    .filter((name) => name !== "application.webp"),
);
await rm(resolve(ui, "assets"), { recursive: true, force: true });
await mkdir(resolve(ui, "assets"), { recursive: true });
await writeFile(resolve(ui, "index.html"), html);
await copyFile(resolve(root, "ui/styles.css"), resolve(ui, "styles.css"));
for (const asset of assets)
  await copyFile(
    resolve(root, "ui/assets", asset),
    resolve(ui, "assets", asset),
  );
await sharp(iconPath)
  .resize(256, 256, { fit: "contain", background: "#00000000" })
  .webp()
  .toFile(resolve(ui, "assets/application.webp"));
const png = await sharp(iconPath)
  .resize(256, 256, { fit: "contain", background: "#00000000" })
  .png()
  .toBuffer();
const icoHeader = Buffer.alloc(22);
icoHeader.writeUInt16LE(1, 2);
icoHeader.writeUInt16LE(1, 4);
icoHeader.writeUInt16LE(1, 10);
icoHeader.writeUInt16LE(32, 12);
icoHeader.writeUInt32LE(png.length, 14);
icoHeader.writeUInt32LE(22, 18);
await writeFile(
  resolve(output, "application.ico"),
  Buffer.concat([icoHeader, png]),
);
const identifier = `com.talaylabs.${product.appId}`;
const constants = {
  APP_ID: product.appId,
  DISPLAY_NAME: product.displayName,
  PRODUCT_DIRECTORY: product.productDirectory,
  EXECUTABLE: product.executable,
  APPLICATION_IDENTIFIER: identifier,
  INSTALLER_IDENTIFIER: `${identifier}.installer`,
};
await writeFile(
  resolve(output, "product.rs"),
  Object.entries(constants)
    .map(
      ([name, value]) => `pub const ${name}: &str = ${JSON.stringify(value)};`,
    )
    .join("\n") + "\n",
);
await writeFile(
  resolve(output, "product.json"),
  JSON.stringify(product, null, 2) + "\n",
);
await writeFile(
  resolve(output, "trust.json"),
  JSON.stringify(trust, null, 2) + "\n",
);
await writeFile(
  resolve(output, "tauri-config.json"),
  JSON.stringify(
    {
      productName: `${product.displayName} Kurulum`,
      identifier: `${identifier}.installer`,
      bundle: { publisher: product.publisher },
    },
    null,
    2,
  ) + "\n",
);
await writeFile(
  resolve(output, "package-inputs.json"),
  JSON.stringify(
    {
      appId: product.appId,
      productDirectory: product.productDirectory,
      executable: product.executable,
      publisher: product.publisher,
      displayName: product.displayName,
    },
    null,
    2,
  ) + "\n",
);
console.log(`Prepared installer for ${product.displayName} (${product.appId})`);
