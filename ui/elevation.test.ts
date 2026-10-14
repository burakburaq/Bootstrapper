import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";
import vm from "node:vm";

type TestUpdate = Pick<InstallerNative.UpdateStatus, "operationId" | "stage"> &
  Partial<InstallerNative.UpdateStatus>;
interface TestStartup {
  entry?: InstallerNative.StartupResult["entry"];
  inspection: InstallerNative.StartupResult["inspection"];
  updates: Partial<InstallerNative.UpdateStatus>;
}
interface MockArgs {
  operationId?: string;
  screen?: InstallerNative.Screen;
  action?: InstallerNative.WindowAction;
}
type MockCommand = keyof InstallerNative.Commands;
type MockHandler = (command: MockCommand, args: MockArgs) => unknown;
type MockEvent = Partial<PointerEvent> & { key?: string };
type MockListener = (event?: MockEvent) => unknown;
interface MockRect {
  left: number;
  top: number;
  width: number;
  height: number;
  right?: number;
  bottom?: number;
}
interface MockImage {
  style: Record<string, string>;
}
interface MockNode {
  dataset: Record<string, string>;
  hidden: boolean;
  inert: boolean;
  listeners: Record<string, MockListener>;
  attributes: Record<string, string>;
  style: Record<string, string>;
  offsetWidth: number;
  textContent?: string;
  focused?: boolean;
  disabled?: boolean;
  pointerId?: number;
  querySelector(): MockImage;
  getBoundingClientRect(): MockRect;
  focus(): void;
  setPointerCapture(id: number): void;
  hasPointerCapture(id: number): boolean;
  releasePointerCapture(): void;
  addEventListener(name: string, callback: MockListener): void;
  setAttribute(name: string, value: string): void;
  removeAttribute(name: string): void;
}
class MockNodes extends Map<string, MockNode> {
  override get(key: string): MockNode {
    const node = super.get(key);
    assert.ok(node, `Missing mock node: ${key}`);
    return node;
  }
}
interface MockWindow {
  innerWidth: number;
  innerHeight: number;
  listeners: Record<string, MockListener>;
  addEventListener(name: string, callback: MockListener): void;
  matchMedia(): { matches: boolean };
  __TAURI_INTERNALS__?: {
    invoke(command: MockCommand, args?: MockArgs): Promise<unknown>;
  };
}
interface SetupOptions {
  demoInstall?: boolean;
  startupCheck?: () => TestStartup | Promise<TestStartup>;
}

const source = await readFile(
  new URL("../../ui/app.js", import.meta.url),
  "utf8",
);
const flush = () => new Promise((resolve) => setImmediate(resolve));

function setup(
  handler?: MockHandler,
  reducedMotion = true,
  previewProgress = false,
  options: SetupOptions = {},
) {
  const image: MockImage = { style: {} };
  const appImage: MockImage = { style: {} };
  const nodes = new MockNodes();
  for (const key of [
    ".installer",
    ".elevation-overlay",
    ".installation-flow",
    "elevate",
    "app-icon",
    "app-name",
    "folder-target",
    "installer-details",
    "progress-status",
    "download-progress",
    "install-progress",
    "download-fill",
    "install-fill",
    "transition-error",
    "maintenance",
    "maintenance-title",
    "maintenance-status",
    "maintenance-action",
    "launch-app",
    "elevation-error",
    "minimize",
    "close",
    "feGaussianBlur",
    "feDisplacementMap",
  ]) {
    nodes.set(key, {
      dataset: { locked: "true" },
      hidden: [
        "elevation-error",
        "installer-details",
        "transition-error",
      ].includes(key),
      inert: true,
      listeners: {},
      attributes: {},
      style: {},
      offsetWidth: 160,
      querySelector() {
        return key === "app-icon" ? appImage : image;
      },
      getBoundingClientRect() {
        const left = key === "folder-target" ? 640 : 160;
        return {
          left,
          right: left + 160,
          top: 160,
          bottom: 320,
          width: 160,
          height: 160,
        };
      },
      focus() {
        this.focused = true;
      },
      setPointerCapture(id) {
        this.pointerId = id;
      },
      hasPointerCapture(id) {
        return this.pointerId === id;
      },
      releasePointerCapture() {
        this.pointerId = undefined;
      },
      addEventListener(name, callback) {
        this.listeners[name] = callback;
      },
      setAttribute(name, value) {
        this.attributes[name] = value;
      },
      removeAttribute(name) {
        delete this.attributes[name];
      },
    });
  }
  const calls: [MockCommand, MockArgs?][] = [];
  nodes.get(".installer").dataset.previewProgress = String(previewProgress);
  nodes.get(".installer").dataset.stage = "selection";
  nodes.get(".installer").dataset.demoInstall = String(
    options.demoInstall ?? true,
  );
  const timers = new Map<number, () => unknown>();
  let timerId = 0;
  const frames = new Map<number, (time: number) => void>();
  let frameId = 0;
  let time = 0;
  const window: MockWindow = {
    innerWidth: 960,
    innerHeight: 480,
    listeners: {},
    addEventListener(name, callback) {
      this.listeners[name] = callback;
    },
    matchMedia: () => ({ matches: reducedMotion }),
  };
  if (handler)
    window.__TAURI_INTERNALS__ = {
      invoke: async (command: MockCommand, args?: MockArgs) => {
        calls.push([command, args]);
        if (command === "startup_check") {
          return options.startupCheck
            ? options.startupCheck()
            : {
                inspection: { platformSupported: true, webviewVersion: "154" },
                updates: {
                  operationId: "test-operation",
                  stage: "unconfigured",
                  configured: false,
                },
              };
        }
        return handler(command, args ?? {});
      },
    };
  vm.runInNewContext(source, {
    window,
    document: {
      querySelector: (key: string) =>
        nodes.get(key.startsWith("#") ? key.slice(1) : key),
      getElementById: (key: string) => nodes.get(key),
      documentElement: { style: { setProperty() {} } },
    },
    setInterval: (callback: () => unknown) => {
      timers.set(++timerId, callback);
      return timerId;
    },
    clearInterval: (id: number) => timers.delete(id),
    requestAnimationFrame: (callback: (time: number) => void) => {
      frames.set(++frameId, callback);
      return frameId;
    },
    cancelAnimationFrame: (id: number) => frames.delete(id),
  });
  return {
    nodes,
    calls,
    timers,
    image,
    appImage,
    window,
    frames,
    async advance(count: number, step = 16) {
      const scales = [];
      for (let i = 0; i < count; i++) {
        time += step;
        const pending = [...frames.values()];
        frames.clear();
        pending.forEach((callback) => callback(time));
        scales.push(
          Number(image.style.transform?.match(/scale\((.+)\)/)?.[1] ?? 1),
        );
        await flush();
      }
      return scales;
    },
    click: () => nodes.get("elevate").listeners.click(),
  };
}

test("preview entry starts directly on progress without requesting or claiming administrator approval", async () => {
  const app = setup(() => true, true, true);
  await flush();
  assert.equal(app.nodes.get(".installer").dataset.stage, "details");
  assert.equal(app.nodes.get(".installer").dataset.locked, "true");
  assert.equal(app.nodes.get(".installer").style.height, `${96}px`);
  assert.equal(
    app.nodes.get(".installer").style.top,
    `${(480 / (960 / 510) - 96) / 2}px`,
  );
  assert.equal(app.nodes.get(".installation-flow").hidden, true);
  assert.equal(app.nodes.get(".elevation-overlay").hidden, true);
  assert.equal(app.nodes.get("installer-details").inert, false);
  assert.equal(app.nodes.get("progress-status").textContent, "indiriliyor");
  assert.deepEqual(
    app.calls.map(([command]) => command),
    ["installer_screen"],
  );
  assert.equal(app.timers.size, 0);
  await app.advance(1, 0);
  await app.advance(1, 7000);
  assert.equal(
    app.nodes.get("progress-status").textContent,
    "Başarıyla tamamlandı",
  );
  assert.equal(app.frames.size, 0);
});

test("startup checks state without requesting UAC", async () => {
  const app = setup(() => false);
  await flush();
  assert.deepEqual(
    app.calls.map(([command]) => command),
    ["startup_check", "elevation_state"],
  );
  assert.equal(app.nodes.get(".installer").dataset.locked, "true");
});

test("normal installed entry reaches the shared repair screen after folder consent", async () => {
  const status: TestUpdate = {
    operationId: "existing-operation",
    stage: "available",
    installedVersion: "0.1.0",
    intent: "repair",
  };
  const app = setup(
    (command) => (command === "update_status" ? status : true),
    true,
    false,
    {
      demoInstall: false,
      startupCheck: () => ({
        inspection: { platformSupported: true, webviewVersion: "154" },
        updates: status,
      }),
    },
  );
  await flush();
  assert.equal(app.nodes.get(".installer").dataset.stage, "selection");
  await app.nodes.get("folder-target").listeners.click();
  assert.equal(app.nodes.get(".installer").dataset.screen, "repair");
  assert.equal(app.nodes.get("maintenance").hidden, false);
  assert.equal(
    app.calls.some(([command]) => command === "install_update"),
    false,
  );
});

test("uninstall confirmation reports native completion without starting installation", async () => {
  const app = setup(() => undefined, true, false, {
    demoInstall: false,
    startupCheck: () => ({
      entry: "uninstall",
      inspection: { platformSupported: true, webviewVersion: "154" },
      updates: {},
    }),
  });
  await flush();
  await app.nodes.get("maintenance-action").listeners.click();
  assert.equal(
    app.nodes.get("maintenance-status").textContent,
    "Uygulama başarıyla kaldırıldı.",
  );
  assert.equal(
    app.calls.some(
      ([command]) =>
        command === "install_update" || command === "request_elevation",
    ),
    false,
  );
});

test("repair entry checks first and does not mutate until confirmation", async () => {
  const status: TestUpdate = {
    operationId: "repair-operation",
    stage: "available",
    installedVersion: "0.1.0",
    intent: "repair",
  };
  const app = setup(
    (command) =>
      command === "install_update" ? { ...status, stage: "completed" } : status,
    true,
    false,
    {
      demoInstall: false,
      startupCheck: () => ({
        entry: "repair",
        inspection: { platformSupported: true, webviewVersion: "154" },
        updates: status,
      }),
    },
  );
  await flush();
  assert.equal(app.nodes.get("maintenance").hidden, false);
  assert.equal(
    app.calls.some(([command]) => command === "install_update"),
    false,
  );
  await app.nodes.get("maintenance-action").listeners.click();
  assert.equal(
    app.calls.filter(([command]) => command === "install_update").length,
    1,
  );
  assert.equal(app.nodes.get("launch-app").hidden, false);
});

test("healthy repair entry does not permit reinstalling intact files", async () => {
  const app = setup(() => true, true, false, {
    demoInstall: false,
    startupCheck: () => ({
      entry: "repair",
      inspection: { platformSupported: true, webviewVersion: "154" },
      updates: {
        stage: "no_release",
        installedVersion: "0.1.0",
        installedHealthy: true,
      },
    }),
  });
  await flush();
  assert.equal(app.nodes.get("maintenance-action").disabled, true);
  await app.nodes.get("maintenance-action").listeners.click();
  assert.equal(
    app.calls.some(([command]) => command === "install_update"),
    false,
  );
});

test("uninstall requires confirmation and never claims success on rejected UAC", async () => {
  const app = setup(
    (command) => {
      if (command === "uninstall_app") throw new Error("cancelled");
      return true;
    },
    true,
    false,
    {
      demoInstall: false,
      startupCheck: () => ({
        entry: "uninstall",
        inspection: { platformSupported: true, webviewVersion: "154" },
        updates: {},
      }),
    },
  );
  await flush();
  assert.equal(
    app.calls.some(([command]) => command === "uninstall_app"),
    false,
  );
  await app.nodes.get("maintenance-action").listeners.click();
  assert.equal(
    app.nodes.get("maintenance-status").textContent?.includes("tamamlanamadı"),
    true,
  );
  assert.equal(app.nodes.get("close").disabled, false);
});

test("shield stays disabled until startup checks finish, without automatic UAC", async () => {
  let finish!: (value: TestStartup) => void;
  const app = setup(() => false, true, false, {
    demoInstall: false,
    startupCheck: () =>
      new Promise((resolve) => {
        finish = resolve;
      }),
  });
  await app.click();
  assert.equal(app.nodes.get("elevate").disabled, true);
  assert.equal(
    app.calls.some(([command]) => command === "request_elevation"),
    false,
  );
  finish({
    inspection: { platformSupported: true, webviewVersion: "154" },
    updates: { operationId: "native-operation", stage: "unconfigured" },
  });
  await flush();
  assert.equal(app.nodes.get("elevate").disabled, false);
  assert.equal(app.nodes.get(".installer").dataset.locked, "true");
});

test("unsupported startup cannot request elevation or start an operation", async () => {
  const app = setup(() => false, true, false, {
    demoInstall: false,
    startupCheck: () => ({
      inspection: { platformSupported: false, webviewVersion: "154" },
      updates: {},
    }),
  });
  await flush();
  await app.click();
  assert.equal(app.nodes.get("elevate").disabled, true);
  assert.equal(app.nodes.get("elevation-error").hidden, false);
  assert.equal(
    app.calls.some(([command]) => command === "request_elevation"),
    false,
  );
});

test("normal native entry with no release source never starts the installation demo", async () => {
  const app = setup(() => true, true, false, { demoInstall: false });
  await flush();
  assert.equal(await app.nodes.get("folder-target").listeners.click(), false);
  await app.advance(2, 7000);
  assert.equal(app.nodes.get(".installer").dataset.stage, "selection");
  assert.equal(
    app.calls.some(([command]) => command === "install_update"),
    false,
  );
  assert.equal(app.frames.size, 0);
});

test("native preparation uses operation ID, ignores stale status and never claims installed", async () => {
  let finish!: (value: TestUpdate | TestStartup) => void;
  let status: TestUpdate = {
    operationId: "native-operation",
    stage: "preparing",
    downloadPercent: null,
    installPercent: null,
  };
  const app = setup(
    (command, args) => {
      if (command === "install_update") {
        assert.equal(args.operationId, "native-operation");
        return new Promise((resolve) => {
          finish = resolve;
        });
      }
      if (command === "update_status") return status;
      return true;
    },
    true,
    false,
    {
      demoInstall: false,
      startupCheck: () => ({
        inspection: { platformSupported: true, webviewVersion: "154" },
        updates: { operationId: "native-operation", stage: "available" },
      }),
    },
  );
  await flush();
  await app.nodes.get("folder-target").listeners.click();
  const poll = [...app.timers.values()].at(-1);
  assert.ok(poll);
  await poll();
  assert.equal(
    app.nodes.get("progress-status").textContent,
    "indiriliyor ve doğrulanıyor",
  );
  assert.equal(
    app.nodes.get("download-progress").attributes["aria-valuenow"],
    undefined,
  );
  status = {
    ...status,
    operationId: "stale-operation",
    stage: "completed",
    downloadPercent: 100,
    installPercent: 100,
  };
  await poll();
  assert.equal(app.nodes.get("launch-app").hidden, true);
  assert.equal(app.nodes.get(".installer").dataset.completed, "false");
  assert.equal(
    app.nodes.get("progress-status").textContent,
    "indiriliyor ve doğrulanıyor",
  );
  finish({
    operationId: "native-operation",
    stage: "prepared",
    downloadPercent: 100,
    installPercent: null,
  });
  await flush();
  await app.advance(2, 7000);
  assert.equal(app.nodes.get("progress-status").textContent, "Paket hazır");
  assert.equal(app.nodes.get("download-fill").style.width, "100%");
  assert.equal(app.nodes.get("install-fill").style.width, "0%");
  assert.equal(app.frames.size, 0);
});

function readAppTransform(app: ReturnType<typeof setup>) {
  const numbers = app.appImage.style.transform
    .match(/-?\d+(?:\.\d+)?(?:e[+-]?\d+)?/g)!
    .map(Number);
  return { x: numbers[0], y: numbers[1], angle: numbers[2], scale: numbers[3] };
}

test("real phases show measured progress and require a verified terminal result", async () => {
  let finish!: (value: TestUpdate | TestStartup) => void;
  let status: TestUpdate = {
    operationId: "native-operation",
    stage: "downloading",
    downloadPercent: 42,
    installPercent: null,
  };
  const app = setup(
    (command) => {
      if (command === "install_update")
        return new Promise((resolve) => {
          finish = resolve;
        });
      if (command === "update_status") return status;
      return true;
    },
    true,
    false,
    {
      demoInstall: false,
      startupCheck: () => ({
        inspection: { platformSupported: true, webviewVersion: "154" },
        updates: { operationId: "native-operation", stage: "available" },
      }),
    },
  );
  await flush();
  await app.nodes.get("folder-target").listeners.click();
  const poll = [...app.timers.values()].at(-1);
  assert.ok(poll);
  await poll();
  assert.equal(app.nodes.get("progress-status").textContent, "indiriliyor");
  assert.equal(app.nodes.get("download-fill").style.width, "42%");
  for (const [phase, label] of [
    ["verifying_package", "doğrulanıyor"],
    ["installing", "kuruluyor"],
    ["verifying_installation", "doğrulanıyor"],
  ]) {
    status = { ...status, stage: phase, downloadPercent: 100 };
    await poll();
    assert.equal(app.nodes.get("progress-status").textContent, label);
    assert.equal(
      app.nodes.get("install-progress").attributes["aria-valuenow"],
      undefined,
    );
  }
  await app.advance(2, 10000);
  assert.equal(app.nodes.get("progress-status").textContent, "doğrulanıyor");
  finish({ ...status, stage: "completed", installPercent: 100 });
  await flush();
  assert.equal(
    app.nodes.get("progress-status").textContent,
    "Başarıyla tamamlandı",
  );
  assert.equal(app.nodes.get("install-fill").style.width, "100%");
  assert.equal(app.timers.size, 0);
});

test("service failure cannot turn into timed demo success", async () => {
  const app = setup(
    (command) => {
      if (command === "install_update") throw new Error("installation_failed");
      return true;
    },
    true,
    false,
    {
      demoInstall: false,
      startupCheck: () => ({
        inspection: { platformSupported: true, webviewVersion: "154" },
        updates: { operationId: "native-operation", stage: "available" },
      }),
    },
  );
  await flush();
  await app.nodes.get("folder-target").listeners.click();
  await flush();
  await app.advance(2, 10000);
  assert.equal(
    app.nodes.get("progress-status").textContent,
    "Kurulum tamamlanamadı",
  );
  assert.notEqual(app.nodes.get("install-fill").style.width, "100%");
});

function pointer(x: number, y: number, timeStamp = 960) {
  return {
    button: 0,
    pointerId: 1,
    clientX: x,
    clientY: y,
    timeStamp,
    preventDefault() {},
  };
}

test("app icon is locked until approval and uses the same 115% hover spring", async () => {
  const app = setup(() => false, false);
  const button = app.nodes.get("app-icon");
  button.listeners.pointerdown(pointer(200, 150));
  assert.equal(button.pointerId, undefined);
  app.nodes.get(".installer").dataset.locked = "false";
  button.listeners.pointerenter();
  await app.advance(60);
  assert.equal(readAppTransform(app).scale, 1.15);
  button.listeners.pointerleave();
  await app.advance(3);
  button.listeners.pointerenter();
  button.listeners.pointerleave();
  await app.advance(60);
  assert.equal(readAppTransform(app).scale, 1);
  assert.equal(app.frames.size, 0);
});

test("drag keeps the picked point under the pointer across window scales and returns on capture loss", async () => {
  for (const zoom of [0.5, 0.75, 1, 1.5]) {
    const app = setup(() => true, false);
    await flush();
    const button = app.nodes.get("app-icon");
    button.getBoundingClientRect = () => ({
      left: 160 * zoom,
      top: 160 * zoom,
      width: 160 * zoom,
      height: 160 * zoom,
    });
    button.listeners.pointerenter();
    await app.advance(60);
    const startX = 180 * zoom;
    const startY = 160 * zoom;
    button.listeners.pointerdown(pointer(startX, startY));
    const [ax, ay] = app.appImage.style.transformOrigin
      .match(/-?\d+(?:\.\d+)?/g)!
      .map(Number);
    let transform = readAppTransform(app);
    assert.ok(Math.abs((160 + ax + transform.x) * zoom - startX) < 0.00001);
    assert.ok(Math.abs((160 + ay + transform.y) * zoom - startY) < 0.00001);
    const movedX = startX + 100 * zoom;
    const movedY = startY + 20 * zoom;
    button.listeners.pointermove(pointer(movedX, movedY, 976));
    await app.advance(4);
    transform = readAppTransform(app);
    assert.ok(Math.abs((160 + ax + transform.x) * zoom - movedX) < 0.00001);
    assert.ok(Math.abs((160 + ay + transform.y) * zoom - movedY) < 0.00001);
    assert.ok(
      Math.abs(transform.angle) > 0.1 && Math.abs(transform.angle) < 20,
    );
    button.listeners.lostpointercapture({ pointerId: 1 });
    await app.advance(120);
    assert.deepEqual(readAppTransform(app), { x: 0, y: 0, angle: 0, scale: 1 });
    assert.equal(button.dataset.dragging, "false");
    assert.equal(app.frames.size, 0);
  }
});

test("faster dragging produces more sway and reduced motion preserves dragging without rotation", async () => {
  const angles = [];
  for (const duration of [160, 16]) {
    const app = setup(() => true, false);
    await flush();
    const button = app.nodes.get("app-icon");
    button.listeners.pointerenter();
    await app.advance(60);
    button.listeners.pointerdown(pointer(240, 180));
    button.listeners.pointermove(pointer(256, 180, 960 + duration));
    await app.advance(1);
    angles.push(readAppTransform(app).angle);
    app.window.listeners.keydown({ key: "Escape" });
    await app.advance(120);
    assert.equal(app.frames.size, 0);
  }
  assert.ok(angles[1] > angles[0] * 5);
  assert.ok(angles[0] > 0); // Upper anchor: moving right swings clockwise.
  const app = setup(() => true);
  await flush();
  const button = app.nodes.get("app-icon");
  button.listeners.pointerdown(pointer(240, 180));
  button.listeners.pointermove(pointer(300, 200, 976));
  assert.equal(readAppTransform(app).angle, 0);
  assert.equal(button.dataset.dragging, "true");
  button.listeners.pointercancel({ pointerId: 1 });
  assert.deepEqual(readAppTransform(app), { x: 0, y: 0, angle: 0, scale: 1 });
});

test("app name uses the 400 ms blur fade, reverses smoothly and restores after drag", async () => {
  const app = setup(() => true, false);
  await flush();
  const button = app.nodes.get("app-icon");
  const name = app.nodes.get("app-name");
  button.listeners.pointerdown(pointer(240, 180, 0));
  await app.advance(13);
  assert.ok(Number(name.style.opacity) > 0 && Number(name.style.opacity) < 1);
  const partialOpacity = name.style.opacity;
  button.listeners.pointerup({ pointerId: 1 });
  assert.equal(name.style.opacity, partialOpacity);
  await app.advance(26);
  assert.equal(name.style.opacity, "1");
  assert.equal(name.style.filter, "blur(0px)");
  button.listeners.pointerdown(pointer(240, 180, 624));
  await app.advance(25); // First frame + 384 ms: not finished yet.
  assert.ok(Number(name.style.opacity) > 0);
  await app.advance(1);
  assert.equal(name.style.opacity, "0");
  assert.equal(name.style.filter, "blur(51px)");
  button.listeners.pointercancel({ pointerId: 1 });
  await app.advance(120);
  assert.equal(name.style.opacity, "1");
  assert.equal(name.style.filter, "blur(0px)");
  assert.equal(app.frames.size, 0);
});

test("app name fades with cubic ease-in and returns with cubic ease-out", async () => {
  const app = setup(() => true, false);
  await flush();
  const button = app.nodes.get("app-icon");
  const name = app.nodes.get("app-name");
  button.listeners.pointerdown(pointer(240, 240, 0));
  await app.advance(13); // First frame + 192 ms: t = 0.48.
  assert.ok(Math.abs(Number(name.style.opacity) - (1 - 0.48 ** 3)) < 1e-9);
  await app.advance(13);
  assert.equal(name.style.opacity, "0");
  button.listeners.pointercancel({ pointerId: 1 });
  await app.advance(13);
  assert.ok(Math.abs(Number(name.style.opacity) - (1 - 0.52 ** 3)) < 1e-9);
  await app.advance(60);
  assert.equal(name.style.opacity, "1");
  assert.equal(app.frames.size, 0);
});

test("folder highlight and drop share the enlarged target without per-frame layout reads", async () => {
  const app = setup(() => true, false);
  await flush();
  const button = app.nodes.get("app-icon");
  const folder = app.nodes.get("folder-target");
  let reads = 0;
  for (const node of [button, folder]) {
    const original = node.getBoundingClientRect.bind(node);
    node.getBoundingClientRect = () => {
      reads++;
      return original();
    };
  }
  button.listeners.pointerdown(pointer(240, 240, 0));
  const capturedReads = reads;
  button.listeners.pointermove(pointer(636, 240, 16));
  await app.advance(30);
  assert.equal(folder.dataset.dropActive, "true");
  assert.equal(reads, capturedReads);
  button.listeners.pointerup(pointer(636, 240, 496));
  assert.equal(app.nodes.get(".installer").dataset.stage, "absorbing");
  await app.advance(100);
  assert.equal(app.nodes.get(".installer").dataset.stage, "details");
});

test("context menus are suppressed in the browser UI too", () => {
  const app = setup();
  let prevented = false;
  app.window.listeners.contextmenu({
    preventDefault() {
      prevented = true;
    },
  });
  assert.equal(prevented, true);
});

test("folder drop runs absorption, contraction, then second-screen reveal in order", async () => {
  const app = setup(() => true, false);
  await flush();
  const button = app.nodes.get("app-icon");
  const surface = app.nodes.get(".installer");
  const flow = app.nodes.get(".installation-flow");
  const details = app.nodes.get("installer-details");
  button.listeners.pointerdown(pointer(240, 180, 0));
  button.listeners.pointermove(pointer(720, 240, 16));
  assert.equal(app.nodes.get("folder-target").dataset.dropActive, "true");
  assert.equal(details.hidden, true);
  assert.equal(surface.dataset.stage, "selection"); // Holding alone never starts navigation.
  button.listeners.pointerup(pointer(720, 240, 16));
  assert.equal(surface.dataset.stage, "absorbing");
  assert.equal(flow.inert, true);
  assert.equal(await app.nodes.get("folder-target").listeners.click(), false);
  await app.advance(19); // First frame + 288 ms: absorption still running.
  assert.equal(surface.dataset.stage, "absorbing");
  assert.ok(readAppTransform(app).scale > 0);
  await app.advance(1);
  assert.equal(app.appImage.style.opacity, "0");
  assert.equal(surface.dataset.stage, "contracting");
  await app.advance(10); // First contraction frame + 144 ms.
  assert.equal(surface.dataset.stage, "contracting");
  assert.ok(Number.parseFloat(surface.style.height) > 96);
  assert.ok(Number.parseFloat(surface.style.borderRadius) > 12.75);
  assert.ok(Number.parseFloat(surface.style.borderRadius) < 25.5);
  assert.equal(
    app.calls.filter(([command]) => command === "installer_screen").length,
    0,
  );
  await app.advance(1);
  assert.equal(surface.dataset.stage, "revealing");
  assert.equal(surface.style.borderRadius, "12.75px");
  assert.equal(flow.hidden, true);
  assert.equal(details.hidden, false);
  assert.equal(details.inert, true);
  assert.equal(
    app.calls.filter(([command]) => command === "installer_screen").length,
    1,
  );
  assert.deepEqual(JSON.parse(JSON.stringify(app.calls.at(-1))), [
    "installer_screen",
    { screen: "details" },
  ]);
  await app.advance(10); // First reveal frame + 144 ms.
  assert.equal(surface.dataset.stage, "revealing");
  assert.ok(Number(details.style.opacity) < 1);
  await app.advance(1);
  assert.equal(surface.dataset.stage, "details");
  assert.equal(details.style.opacity, "1");
  assert.equal(details.style.transform, "translateY(0px)");
  assert.equal(details.inert, false);
  assert.equal(details.focused, true);
  assert.equal(app.frames.size, 1); // Only the timed UI demo remains.
});

test("cancelled pointer over folder never advances, and drop elsewhere returns home", async () => {
  const app = setup(() => true, false);
  await flush();
  const button = app.nodes.get("app-icon");
  button.listeners.pointerdown(pointer(240, 180, 0));
  button.listeners.pointermove(pointer(720, 240, 16));
  button.listeners.pointercancel(pointer(720, 240, 16));
  await app.advance(120);
  assert.equal(app.nodes.get("installer-details").hidden, true);
  assert.equal(
    app.calls.filter(([command]) => command === "installer_screen").length,
    0,
  );
  assert.deepEqual(readAppTransform(app), { x: 0, y: 0, angle: 0, scale: 1 });
  button.listeners.pointerdown(pointer(240, 180, 1920));
  button.listeners.pointermove(pointer(500, 240, 1936));
  button.listeners.pointerup(pointer(500, 240, 1936));
  await app.advance(120);
  assert.deepEqual(readAppTransform(app), { x: 0, y: 0, angle: 0, scale: 1 });
  assert.equal(app.frames.size, 0);
});

test("drop uses the transformed icon center rather than the pointer at every window scale", async () => {
  for (const zoom of [0.5, 0.75, 1, 1.5]) {
    for (const [endX, accepted] of [
      [618, true],
      [770, false],
    ] as const) {
      const app = setup(() => true);
      await flush();
      const button = app.nodes.get("app-icon");
      button.getBoundingClientRect = () => ({
        left: 160 * zoom,
        top: 160 * zoom,
        width: 160 * zoom,
        height: 160 * zoom,
      });
      app.nodes.get("folder-target").getBoundingClientRect = () => ({
        left: 640 * zoom,
        right: 800 * zoom,
        top: 160 * zoom,
        bottom: 320 * zoom,
        width: 160 * zoom,
        height: 160 * zoom,
      });
      // Pick near the left edge. 618: pointer outside, center inside;
      // 770: pointer inside, center outside the right edge.
      button.listeners.pointerdown(pointer(180 * zoom, 240 * zoom, 0));
      button.listeners.pointermove(pointer(endX * zoom, 240 * zoom, 16));
      assert.equal(
        app.nodes.get("folder-target").dataset.dropActive,
        String(accepted),
      );
      button.listeners.pointerup(pointer(endX * zoom, 240 * zoom, 16));
      await flush();
      assert.equal(app.nodes.get("installer-details").hidden, !accepted);
      assert.equal(
        app.nodes.get(".installer").style.borderRadius,
        accepted ? "12.75px" : undefined,
      );
    }
  }
});

test("reduced motion and keyboard folder activation skip transitions but retain demo timing", async () => {
  const app = setup(() => true);
  await flush();
  assert.equal(await app.nodes.get("folder-target").listeners.click(), true);
  assert.equal(app.nodes.get(".installer").dataset.stage, "details");
  assert.equal(app.frames.size, 1);
  assert.equal(app.nodes.get("installer-details").focused, true);
});

test("CSS contraction keeps the full native viewport and survives duplicate resize events", async () => {
  const app = setup(() => true, false);
  await flush();
  const surface = app.nodes.get(".installer");
  const pending = app.nodes.get("folder-target").listeners.click();
  await app.advance(31);
  assert.equal(surface.dataset.stage, "revealing");
  assert.ok(Math.abs(parseFloat(surface.style.height) - 96) < 1e-9);
  assert.ok(
    Math.abs(parseFloat(surface.style.top) - (480 / (960 / 510) - 96) / 2) <
      1e-9,
  );
  assert.equal(app.window.innerWidth, 960);
  assert.equal(app.window.innerHeight, 480);
  app.window.listeners.resize();
  app.window.listeners.resize();
  await app.advance(11);
  assert.equal(await pending, true);
  assert.equal(surface.dataset.stage, "details");
  assert.ok(Math.abs(parseFloat(surface.style.height) - 96) < 1e-9);
  assert.ok(
    Math.abs(parseFloat(surface.style.top) - (480 / (960 / 510) - 96) / 2) <
      1e-9,
  );
  assert.deepEqual(
    app.calls
      .filter(([command]) => command === "installer_screen")
      .map(([, args]) => args?.screen),
    ["details"],
  );
  assert.equal(app.frames.size, 1);
});

test("the second screen waits only for the native input region, not a viewport resize", async () => {
  let reply!: (value: boolean) => void;
  const app = setup(
    (command) =>
      command === "installer_screen"
        ? new Promise((resolve) => {
            reply = resolve;
          })
        : true,
    false,
  );
  await flush();
  const pending = app.nodes.get("folder-target").listeners.click();
  await app.advance(31);
  app.window.listeners.resize();
  assert.equal(app.nodes.get(".installer").dataset.stage, "contracting");
  assert.equal(
    app.nodes.get(".installer").style.top,
    `${(480 / (960 / 510) - 96) / 2}px`,
  );
  reply(true);
  await flush();
  assert.equal(app.nodes.get(".installer").dataset.stage, "revealing");
  await app.advance(11);
  assert.equal(await pending, true);
  assert.equal(app.window.innerHeight, 480);
  assert.equal(app.frames.size, 1);
});

test("UI demo downloads for 3 seconds, verifies for 1, then installs for 3 without native work", async () => {
  const app = setup(() => true);
  await flush();
  assert.equal(app.nodes.get("progress-status").textContent, undefined);
  await app.nodes.get("folder-target").listeners.click();
  const status = app.nodes.get("progress-status");
  const download = app.nodes.get("download-fill");
  const install = app.nodes.get("install-fill");
  await app.advance(1, 0); // Start the demo clock only after the second screen opens.
  assert.equal(status.textContent, "indiriliyor");
  await app.advance(1, 1500);
  assert.equal(download.style.width, "50%");
  assert.equal(install.style.width, "0%");
  await app.advance(1, 1500);
  assert.equal(status.textContent, "doğrulanıyor");
  assert.equal(download.style.width, "100%");
  assert.equal(install.style.width, "0%");
  await app.advance(1, 999);
  assert.equal(status.textContent, "doğrulanıyor");
  assert.equal(install.style.width, "0%");
  await app.advance(1, 1);
  assert.equal(status.textContent, "kuruluyor");
  await app.advance(1, 1500);
  assert.equal(download.style.width, "100%");
  assert.equal(install.style.width, "50%");
  await app.advance(1, 1500);
  assert.equal(status.textContent, "Başarıyla tamamlandı");
  assert.equal(install.style.width, "100%");
  assert.equal(
    app.nodes.get("download-progress").attributes["aria-valuenow"],
    "100",
  );
  assert.equal(
    app.nodes.get("install-progress").attributes["aria-valuenow"],
    "100",
  );
  assert.equal(app.frames.size, 0);
  assert.deepEqual(
    app.calls.map(([command]) => command),
    ["startup_check", "elevation_state", "installer_screen"],
  );
});

test("helper loss cancels the UI demo and re-entry restarts it from zero", async () => {
  let approved = true;
  const app = setup(() => approved);
  await flush();
  await app.nodes.get("folder-target").listeners.click();
  await app.advance(1, 0);
  await app.advance(1, 5500);
  approved = false;
  await [...app.timers.values()][0]();
  assert.equal(app.nodes.get("installer-details").hidden, true);
  assert.equal(app.nodes.get("download-fill").style.width, "0%");
  assert.equal(app.nodes.get("install-fill").style.width, "0%");
  assert.equal(app.frames.size, 0);
  approved = true;
  await app.click();
  await app.nodes.get("folder-target").listeners.click();
  assert.equal(app.nodes.get("progress-status").textContent, "indiriliyor");
  await app.advance(1, 0);
  await app.advance(1, 1500);
  assert.equal(app.nodes.get("download-fill").style.width, "50%");
  app.window.listeners.beforeunload();
  assert.equal(app.frames.size, 0);
});

test("compact CSS placement stays centered across resolution changes", async () => {
  const app = setup(() => true);
  await flush();
  assert.equal(await app.nodes.get("folder-target").listeners.click(), true);
  for (const [width, height, panelHeight, panelTop] of [
    [340, 170, 64, 53],
    [510, 255, 96, 79.5],
    [765, 383, 144, 119.5],
    [1020, 510, 192, 159],
  ]) {
    app.window.innerWidth = width;
    app.window.innerHeight = height;
    app.window.listeners.resize();
    const surface = app.nodes.get(".installer");
    const zoom = width / 510;
    assert.ok(
      Math.abs(parseFloat(surface.style.height) * zoom - panelHeight) < 1e-9,
    );
    assert.ok(Math.abs(parseFloat(surface.style.top) * zoom - panelTop) < 1e-9);
    assert.equal(surface.style.borderRadius, "12.75px");
    assert.equal(surface.dataset.stage, "details");
  }
});

test("helper loss while applying the input region queues full-region restoration", async () => {
  let approved = true;
  let reply!: (value: boolean) => void;
  const app = setup((command, args) => {
    if (command === "elevation_state") return approved;
    if (command === "installer_screen" && args?.screen === "details")
      return new Promise((resolve) => {
        reply = resolve;
      });
    return true;
  }, false);
  await flush();
  const pending = app.nodes.get("folder-target").listeners.click();
  await app.advance(31);
  approved = false;
  await [...app.timers.values()][0]();
  assert.equal(app.nodes.get(".installer").dataset.stage, "selection");
  assert.equal(app.nodes.get(".installer").style.top, "");
  reply(true);
  assert.equal(await pending, false);
  await app.advance(120);
  assert.equal(app.nodes.get(".installer").dataset.locked, "true");
  assert.equal(app.nodes.get("installer-details").hidden, true);
  assert.deepEqual(
    app.calls
      .filter(([command]) => command === "installer_screen")
      .map(([, args]) => args?.screen),
    ["details", "selection"],
  );
  assert.equal(app.frames.size, 0);
});

test("layout failure restores the first screen for retry", async () => {
  const app = setup((command, args) => {
    if (command === "installer_screen" && args?.screen === "details")
      throw new Error("window_layout_failed");
    return true;
  });
  await flush();
  assert.equal(await app.nodes.get("folder-target").listeners.click(), false);
  await flush();
  assert.equal(app.nodes.get(".installer").dataset.stage, "selection");
  assert.equal(app.nodes.get(".installer").style.borderRadius, "");
  assert.equal(app.nodes.get(".installation-flow").hidden, false);
  assert.equal(app.nodes.get("installer-details").hidden, true);
  assert.equal(app.nodes.get("transition-error").hidden, false);
  assert.deepEqual(readAppTransform(app), { x: 0, y: 0, angle: 0, scale: 1 });
});

test("helper loss during a transition cancels navigation and restores the UAC screen", async () => {
  let approved = true;
  const app = setup(
    (command) => (command === "elevation_state" ? approved : true),
    false,
  );
  await flush();
  const pending = app.nodes.get("folder-target").listeners.click();
  await app.advance(5);
  approved = false;
  await [...app.timers.values()][0]();
  assert.equal(await pending, false);
  await app.advance(120);
  assert.equal(app.nodes.get(".installer").dataset.stage, "selection");
  assert.equal(app.nodes.get(".installer").dataset.locked, "true");
  assert.equal(app.nodes.get(".elevation-overlay").hidden, false);
  assert.equal(app.nodes.get("installer-details").hidden, true);
  assert.equal(
    app.calls.some(
      ([command, args]) =>
        command === "installer_screen" && args?.screen === "details",
    ),
    false,
  );
  assert.equal(app.frames.size, 0);
});

test("rapid hover reversal settles at 100% without duplicate animation loops", async () => {
  const app = setup(undefined, false);
  const shield = app.nodes.get("elevate");
  shield.listeners.pointerenter();
  await app.advance(60);
  assert.equal(app.image.style.transform, "scale(1.15)");
  shield.listeners.pointerleave();
  await app.advance(3);
  shield.listeners.pointerenter();
  shield.listeners.pointerleave();
  assert.equal(app.frames.size, 1);
  await app.advance(60);
  assert.equal(app.image.style.transform, "scale(1)");
  assert.equal(app.frames.size, 0);
});

test("press shrinks to 85% and returns to 100% before requesting native UAC", async () => {
  const app = setup(() => false, false);
  await flush();
  app.nodes.get("elevate").listeners.pointerenter();
  await app.advance(60);
  const pending = app.click();
  assert.equal(
    app.calls.filter(([command]) => command === "request_elevation").length,
    0,
  );
  const scales = await app.advance(120);
  await pending;
  assert.ok(Math.min(...scales) <= 0.85);
  assert.ok(Math.min(...scales) > 0.8);
  assert.equal(app.image.style.transform, "scale(1)");
  assert.equal(
    app.calls.filter(([command]) => command === "request_elevation").length,
    1,
  );
  assert.equal(app.frames.size, 0);
});

test("successful click unlocks only after native approval", async () => {
  let approve!: (value: boolean) => void;
  const app = setup((command) =>
    command === "elevation_state"
      ? false
      : new Promise((resolve) => {
          approve = resolve;
        }),
  );
  await flush();
  const pending = app.click();
  await flush();
  assert.equal(app.nodes.get(".elevation-overlay").hidden, false);
  assert.equal(app.nodes.get("elevate").disabled, true);
  await app.click();
  assert.equal(
    app.calls.filter(([command]) => command === "request_elevation").length,
    1,
  );
  approve(true);
  await pending;
  assert.equal(app.nodes.get(".installer").dataset.locked, "false");
  assert.equal(app.nodes.get(".elevation-overlay").hidden, true);
  assert.equal(app.nodes.get(".installation-flow").inert, false);
});

test("approval fades all effects together before unlocking, and helper loss restores them", async () => {
  let approved = false;
  const app = setup((command) => {
    if (command === "request_elevation") approved = true;
    return approved;
  }, false);
  await flush();
  const pending = app.click();
  for (
    let frame = 0;
    frame < 100 &&
    !app.calls.some(([command]) => command === "request_elevation");
    frame++
  ) {
    await app.advance(1);
  }
  assert.ok(app.calls.some(([command]) => command === "request_elevation"));
  await app.advance(13); // First reveal frame + 192 ms of the 400 ms transition.
  assert.equal(app.nodes.get(".installer").dataset.locked, "true");
  assert.equal(app.nodes.get(".elevation-overlay").hidden, false);
  assert.equal(app.nodes.get(".installation-flow").inert, true);
  const blur = Number(app.nodes.get("feGaussianBlur").attributes.stdDeviation);
  const displacement = Number(
    app.nodes.get("feDisplacementMap").attributes.scale,
  );
  const opacity = Number(app.nodes.get("elevate").style.opacity);
  assert.ok(blur > 0 && blur < 4.25);
  assert.ok(displacement > 0 && displacement < 5.3125);
  assert.ok(opacity > 0 && opacity < 1);
  await app.advance(12); // 384 ms: the content must still be locked.
  assert.equal(app.nodes.get(".elevation-overlay").hidden, false);
  await app.advance(1); // 400 ms: the reveal finishes.
  await pending;
  assert.equal(app.nodes.get(".elevation-overlay").hidden, true);
  assert.equal(app.nodes.get(".installation-flow").inert, false);
  assert.equal(app.nodes.get("feGaussianBlur").attributes.stdDeviation, "0");
  assert.equal(app.nodes.get("feDisplacementMap").attributes.scale, "0");
  assert.equal(app.nodes.get("elevate").style.filter, "blur(51px)");
  assert.equal(app.nodes.get("elevate").style.opacity, "0");
  assert.equal(app.frames.size, 0);
  approved = false;
  await [...app.timers.values()][0]();
  assert.equal(app.nodes.get(".elevation-overlay").hidden, false);
  assert.equal(app.nodes.get("feGaussianBlur").attributes.stdDeviation, "4.25");
  assert.equal(app.nodes.get("feDisplacementMap").attributes.scale, "5.3125");
  assert.equal(app.nodes.get("elevate").style.opacity, "");
});

for (const error of ["elevation_cancelled", "elevation_launch_failed"]) {
  test(`${error} keeps the shield available for retry`, async () => {
    const app = setup((command) => {
      if (command === "elevation_state") return false;
      throw error;
    });
    await flush();
    await app.click();
    assert.equal(app.nodes.get(".installer").dataset.locked, "true");
    assert.equal(app.nodes.get(".elevation-overlay").hidden, false);
    assert.equal(app.nodes.get("elevate").disabled, false);
    assert.equal(
      app.nodes.get("elevation-error").hidden,
      error === "elevation_cancelled",
    );
  });
}

for (const code of [
  "bootstrap_download_failed",
  "bootstrap_service_binary_differs",
  "service_ipc_failed",
  "service_ipc_timeout",
]) {
  test(`${code} shows a service failure without claiming UAC was denied`, async () => {
    const app = setup((command) => {
      if (command === "elevation_state") return false;
      throw code;
    });
    await flush();
    await app.click();
    assert.equal(app.nodes.get("elevation-error").hidden, false);
    assert.equal(
      app.nodes.get("elevation-error").textContent,
      `Kurulum servisi hazırlanamadı (${code}).`,
    );
    assert.equal(app.nodes.get("elevate").disabled, false);
    assert.equal(app.nodes.get(".installer").dataset.locked, "true");
  });
}

test("detailed IPC failure clears busy state and keeps installation locked", async () => {
  const app = setup((command) => {
    if (command === "elevation_state") return false;
    throw "service_ipc_failed: IPC OpenProcess failed (pid 123): Access denied (os error 5)";
  });
  await flush();
  await app.click();
  assert.equal(
    app.nodes.get("elevation-error").textContent,
    "Kurulum servisi hazırlanamadı (service_ipc_failed).",
  );
  assert.equal(app.nodes.get("elevate").disabled, false);
  assert.equal(app.nodes.get("elevate").attributes["aria-busy"], undefined);
  assert.equal(app.nodes.get(".installer").dataset.locked, "true");
});

test("lost elevated helper re-locks the screen", async () => {
  let alive = true;
  const app = setup(() => alive);
  await flush();
  assert.equal(app.nodes.get(".elevation-overlay").hidden, true);
  alive = false;
  await [...app.timers.values()][0]();
  assert.equal(app.nodes.get(".elevation-overlay").hidden, false);
  assert.equal(app.timers.size, 0);
});

test("browser preview cannot grant elevation", async () => {
  const app = setup();
  await app.click();
  assert.equal(app.nodes.get(".installer").dataset.locked, "true");
  assert.equal(app.nodes.get("elevation-error").hidden, false);
  assert.equal(app.calls.length, 0);
});
