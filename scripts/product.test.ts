import assert from "node:assert/strict";
import test from "node:test";
import { escapeHtml, validateProduct, validateTrust } from "./product.js";

const academy = {
  schemaVersion: 1,
  appId: "owllig-academy",
  displayName: "Owllig Academy",
  publisher: "TalayLabs",
  copyright: "© 2026 TalayLabs",
  productDirectory: "OwlligAcademy",
  executable: "academy.exe",
  icon: "./icon.webp",
  description: "Learning platform",
};

test("different applications retain distinct identities and install directories", () => {
  const first = validateProduct(academy);
  const second = validateProduct({
    ...academy,
    appId: "owllig-campus",
    displayName: "Owllig Campus",
    productDirectory: "OwlligCampus",
    executable: "campus.exe",
  });
  assert.notEqual(first.appId, second.appId);
  assert.notEqual(first.productDirectory, second.productDirectory);
});

test("unsafe directories, executable paths, control characters and secrets are rejected", () => {
  for (const directory of [
    "../Academy",
    "C:\\Academy",
    "TalayLabs\\Academy",
    "NUL",
    "Academy.",
  ])
    assert.throws(() =>
      validateProduct({ ...academy, productDirectory: directory }),
    );
  assert.throws(() =>
    validateProduct({ ...academy, executable: "../academy.exe" }),
  );
  assert.throws(() =>
    validateProduct({ ...academy, displayName: "Academy\nInjected" }),
  );
  assert.throws(() => validateProduct({ ...academy, privateKey: "secret" }));
  assert.equal(escapeHtml('A & <B> "C"'), "A &amp; &lt;B&gt; &quot;C&quot;");
});

test("signed policy cannot be silently reassigned to another product", () => {
  const product = validateProduct(academy);
  const trust = {
    serverUrl: null,
    windowsPolicy: {
      body: {
        app_id: product.appId,
        product_directory: product.productDirectory,
        executable: product.executable,
      },
    },
  };
  assert.doesNotThrow(() => validateTrust(trust, product));
  assert.throws(() =>
    validateTrust(
      trust,
      validateProduct({ ...academy, appId: "owllig-campus" }),
    ),
  );
  assert.throws(() =>
    validateTrust(
      { ...trust, serverUrl: "http://updates.example.com" },
      product,
    ),
  );
  assert.throws(() =>
    validateTrust({ serverUrl: "https://updates.example.com" }, product),
  );
});
