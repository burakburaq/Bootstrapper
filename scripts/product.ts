import { readFile, realpath } from "node:fs/promises";
import { dirname, resolve } from "node:path";

export interface Product {
  schemaVersion: 1;
  appId: string;
  displayName: string;
  publisher: string;
  copyright: string;
  productDirectory: string;
  executable: string;
  icon: string;
  description: string;
}

export function validateProduct(value: unknown): Product {
  if (!value || typeof value !== "object" || Array.isArray(value))
    throw new Error("Product configuration must be an object");
  const product = value as Record<string, unknown>;
  const fields = [
    "schemaVersion",
    "appId",
    "displayName",
    "publisher",
    "copyright",
    "productDirectory",
    "executable",
    "icon",
    "description",
  ];
  if (Object.keys(product).some((key) => !fields.includes(key)))
    throw new Error("Unknown product configuration field");
  if (product.schemaVersion !== 1)
    throw new Error("Unsupported product schemaVersion");
  for (const field of fields.slice(1)) {
    if (
      typeof product[field] !== "string" ||
      !product[field].trim() ||
      /[\x00-\x1f\x7f]/.test(product[field])
    )
      throw new Error(`Invalid product field: ${field}`);
  }
  if (!/^[a-z][a-z0-9-]{0,63}$/.test(product.appId as string))
    throw new Error("Invalid appId");
  const directory = product.productDirectory as string;
  if (
    !/^[A-Za-z0-9_-]{1,64}$/.test(directory) ||
    /^(con|prn|aux|nul|com[1-9]|lpt[1-9])$/i.test(directory)
  )
    throw new Error(
      "productDirectory must be one safe Program Files directory component",
    );
  if (!/^[A-Za-z0-9_-]+\.exe$/i.test(product.executable as string))
    throw new Error("Invalid executable filename");
  for (const field of ["displayName", "publisher", "copyright"])
    if (Buffer.byteLength(product[field] as string) > 128)
      throw new Error(`${field} is too long`);
  if ((product.description as string).length > 512)
    throw new Error("Description is too long");
  if (!/\.(webp|png)$/i.test(product.icon as string))
    throw new Error("Product icon must be PNG or WebP");
  return product as unknown as Product;
}

export async function loadProduct(path: string) {
  const source = await realpath(path);
  const product = validateProduct(JSON.parse(await readFile(source, "utf8")));
  const iconPath = await realpath(resolve(dirname(source), product.icon));
  return { product, iconPath };
}

export function validateTrust(value: unknown, product: Product) {
  if (!value || typeof value !== "object" || Array.isArray(value))
    throw new Error("Invalid trust configuration");
  const trust = value as Record<string, any>;
  const fields = [
    "serverUrl",
    "channel",
    "metadataKeys",
    "packagePublicKey",
    "allowedDownloadHosts",
    "maxArtifactBytes",
    "windowsPolicy",
    "bootstrapHelper",
  ];
  if (Object.keys(trust).some((key) => !fields.includes(key)))
    throw new Error("Unknown trust configuration field");
  if (trust.serverUrl !== null) {
    const origin = new URL(trust.serverUrl);
    if (
      origin.protocol !== "https:" ||
      origin.username ||
      origin.password ||
      origin.search ||
      origin.hash ||
      origin.pathname !== "/"
    )
      throw new Error("Update server must be a plain HTTPS origin");
    if (!trust.windowsPolicy || !trust.bootstrapHelper)
      throw new Error(
        "Enabled installation requires a signed policy and helper descriptor",
      );
  }
  if (trust.windowsPolicy) {
    const policy = trust.windowsPolicy.body;
    if (
      policy?.app_id !== product.appId ||
      policy?.product_directory !== product.productDirectory ||
      policy?.executable !== product.executable
    )
      throw new Error(
        "Signed Windows policy does not match product configuration",
      );
  }
  // Cryptographic verification stays in the native client. This build-time
  // check never rewrites a signed policy or imports private keys.
  return trust;
}

export function escapeHtml(text: string) {
  return text.replace(
    /[&<>"']/g,
    (char) =>
      ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[
        char
      ]!,
  );
}
