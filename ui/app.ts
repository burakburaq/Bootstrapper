function requiredElement<T extends Element>(
  selector: string,
  root: ParentNode = document,
): T {
  const element = root.querySelector<T>(selector);
  if (!element) throw new Error(`Missing installer element: ${selector}`);
  return element;
}

interface MotionState {
  x: number;
  y: number;
  angle: number;
  scale: number;
}
interface DragState {
  pointerId: number;
  anchorX: number;
  anchorY: number;
  startX: number;
  startY: number;
  offsetX: number;
  offsetY: number;
  x: number;
  y: number;
  time: number;
  sway: number;
  zoom: number;
  geometry: { rect: DOMRect; bounds: DOMRect; zoom: number };
}
const invoke = window.__TAURI_INTERNALS__?.invoke;
const installer = requiredElement<HTMLElement>(".installer");
// Temporary UI preview entry; this never grants native administrator privileges.
const progressPreview = installer.dataset.previewProgress === "true";
// Only an explicit preview/test entry may simulate an installation.
const demoInstall =
  progressPreview || !invoke || installer.dataset.demoInstall === "true";
const compactHeight = 96;
const overlay = requiredElement<HTMLElement>(".elevation-overlay");
const flow = requiredElement<HTMLElement>(".installation-flow");
const folder = requiredElement<HTMLElement>("#folder-target");
const details = requiredElement<HTMLElement>("#installer-details");
const maintenance = requiredElement<HTMLElement>("#maintenance");
const maintenanceTitle = requiredElement<HTMLElement>("#maintenance-title");
const maintenanceStatus = requiredElement<HTMLElement>("#maintenance-status");
const maintenanceAction = requiredElement<HTMLButtonElement>(
  "#maintenance-action",
);
const launchButton = requiredElement<HTMLButtonElement>("#launch-app");
let maintenanceMode: "repair" | "uninstall" | undefined;
let maintenanceBusy = false;
const progressStatus = requiredElement<HTMLElement>("#progress-status");
const downloadProgress = requiredElement<HTMLElement>("#download-progress");
const installProgress = requiredElement<HTMLElement>("#install-progress");
const downloadFill = requiredElement<HTMLElement>("#download-fill");
const installFill = requiredElement<HTMLElement>("#install-fill");
const transitionError = requiredElement<HTMLElement>("#transition-error");
const shield = requiredElement<HTMLButtonElement>("#elevate");
const elevationError = requiredElement<HTMLElement>("#elevation-error");
const backgroundBlur =
  requiredElement<SVGFEGaussianBlurElement>("feGaussianBlur");
const backgroundDisplacement =
  requiredElement<SVGFEDisplacementMapElement>("feDisplacementMap");
let elevationPending = false;
let elevationPoll: number | undefined;
let revealFrame: number | undefined;
let stage: InstallerNative.Stage = "selection";
let transitionFrame: number | undefined;
let transitionComplete: ((completed: boolean) => void) | undefined;
let transitionGeneration = 0;
let nativeRegionPending = false;
let layoutQueue: Promise<void> = Promise.resolve();
let progressFrame: number | undefined;
let nativeProgressPoll: number | undefined;
let nativeProgressGeneration = 0;
let startupPending = Boolean(invoke) && !progressPreview;
let nativeUpdate: InstallerNative.UpdateStatus | undefined;
const shieldImage = requiredElement<HTMLImageElement>("img", shield);
let springScale = 1;
let springVelocity = 0;
let springTarget = 1;
let springFrame: number | undefined;
let springLastTime: number | undefined;
let springComplete: (() => void) | undefined;

function springTick(time: number) {
  let remaining = Math.min(
    (time - (springLastTime ?? time - 16)) / 1000,
    0.032,
  );
  springLastTime = time;
  // Small integration steps keep the spring stable at different refresh rates.
  while (remaining > 0) {
    const dt = Math.min(remaining, 1 / 120);
    springVelocity +=
      (380 * (springTarget - springScale) - 26 * springVelocity) * dt;
    springScale += springVelocity * dt;
    remaining -= dt;
  }
  shieldImage.style.transform = `scale(${springScale})`;
  if (
    Math.abs(springTarget - springScale) < 0.0005 &&
    Math.abs(springVelocity) < 0.005
  ) {
    springScale = springTarget;
    springVelocity = 0;
    shieldImage.style.transform = `scale(${springTarget})`;
    springFrame = undefined;
    springLastTime = undefined;
    springComplete?.();
    springComplete = undefined;
  } else {
    springFrame = requestAnimationFrame(springTick);
  }
}

function springTo(target: number): Promise<void> {
  springComplete?.();
  springTarget = target;
  if (window.matchMedia("(prefers-reduced-motion: reduce)").matches) {
    if (springFrame !== undefined) cancelAnimationFrame(springFrame);
    springFrame = undefined;
    springLastTime = undefined;
    springVelocity = 0;
    springScale = target;
    shieldImage.style.transform = `scale(${target})`;
    return Promise.resolve();
  }
  return new Promise<void>((resolve) => {
    springComplete = resolve;
    if (springFrame === undefined)
      springFrame = requestAnimationFrame(springTick);
  });
}

shield.addEventListener("pointerenter", () => {
  if (!elevationPending) void springTo(1.15);
});
shield.addEventListener("pointerleave", () => {
  if (!elevationPending) void springTo(1);
});

function animateStep(
  duration: number,
  paint: (progress: number) => void,
): Promise<boolean> {
  if (window.matchMedia("(prefers-reduced-motion: reduce)").matches) {
    paint(1);
    return Promise.resolve(true);
  }
  return new Promise<boolean>((resolve) => {
    transitionComplete = resolve;
    let started: number | undefined;
    function tick(time: number) {
      started ??= time;
      const elapsed = Math.min((time - started) / duration, 1);
      paint(1 - (1 - elapsed) ** 3);
      if (elapsed < 1) transitionFrame = requestAnimationFrame(tick);
      else {
        transitionFrame = transitionComplete = undefined;
        resolve(true);
      }
    }
    transitionFrame = requestAnimationFrame(tick);
  });
}

function setStage(value: InstallerNative.Stage) {
  stage = value;
  installer.dataset.stage = value;
  if (value === "selection") installer.dataset.screen = "confirmation";
  else if (value === "details")
    installer.dataset.screen = maintenanceMode ?? "download";
  flow.inert = value !== "selection" || installer.dataset.locked === "true";
}

function nativeScreen(screen: InstallerNative.Screen) {
  const operation = layoutQueue
    .catch(() => {})
    .then(() => (invoke ? invoke("installer_screen", { screen }) : undefined));
  layoutQueue = operation;
  return operation;
}

function paintCompactSurface() {
  const initialHeight = window.innerHeight / (window.innerWidth / 510);
  installer.style.height = `${compactHeight}px`;
  installer.style.top = `${(initialHeight - compactHeight) / 2}px`;
  installer.style.borderRadius = "12.75px";
}

function paintDemoProgress(elapsed: number) {
  const download = Math.min(elapsed / 3000, 1);
  const installation = Math.max(0, Math.min((elapsed - 4000) / 3000, 1));
  const status =
    elapsed < 3000
      ? "indiriliyor"
      : elapsed < 4000
        ? "doğrulanıyor"
        : elapsed < 7000
          ? "kuruluyor"
          : "Başarıyla tamamlandı";
  if (progressStatus.textContent !== status)
    progressStatus.textContent = status;
  downloadFill.style.width = `${download * 100}%`;
  installFill.style.width = `${installation * 100}%`;
  downloadProgress.setAttribute(
    "aria-valuenow",
    String(Math.round(download * 100)),
  );
  installProgress.setAttribute(
    "aria-valuenow",
    String(Math.round(installation * 100)),
  );
}

function stopDemoProgress() {
  if (progressFrame !== undefined) cancelAnimationFrame(progressFrame);
  progressFrame = undefined;
}

function paintNativeUpdate(update: InstallerNative.UpdateStatus) {
  if (!update || update.operationId !== nativeUpdate?.operationId) return;
  launchButton.hidden = !["completed", "no_op"].includes(update.stage);
  installer.dataset.completed = String(!launchButton.hidden);
  nativeUpdate = update;
  const labels: Record<string, string> = {
    checking: "kontrol ediliyor",
    downloading: "indiriliyor",
    verifying_package: "doğrulanıyor",
    preflight: "kontrol ediliyor",
    awaiting_authorization: "kurulum izni alınıyor",
    staging: "kuruluyor",
    verifying_staging: "doğrulanıyor",
    closing_applications: "Uygulamayı kapatman bekleniyor",
    installing: "kuruluyor",
    verifying_installation: "doğrulanıyor",
    completed: "Başarıyla tamamlandı",
    no_op: "Başarıyla tamamlandı",
    interrupted: "Kurulum yarım kaldı",
    preparing: "indiriliyor ve doğrulanıyor",
    prepared: "Paket hazır",
    failed: "Kurulum tamamlanamadı",
  };
  progressStatus.textContent =
    labels[update.stage] ?? "Kurulum bağlantısı bekleniyor";
  if (maintenanceMode === "repair" && !maintenance.hidden) {
    const percent = update.installPercent ?? update.downloadPercent;
    maintenanceStatus.textContent = ["completed", "no_op"].includes(
      update.stage,
    )
      ? "Onarım başarıyla tamamlandı."
      : `${labels[update.stage] ?? "Kontrol ediliyor"}${typeof percent === "number" ? ` · %${percent}` : ""}`;
  }
  for (const [track, fill, percent] of [
    [downloadProgress, downloadFill, update.downloadPercent],
    [installProgress, installFill, update.installPercent],
  ] as const) {
    const measured = typeof percent === "number" && Number.isFinite(percent);
    const clampedPercent = measured ? Math.min(100, Math.max(0, percent)) : 0;
    fill.style.width = `${clampedPercent}%`;
    if (measured) track.setAttribute("aria-valuenow", String(clampedPercent));
    else track.removeAttribute("aria-valuenow");
    track.setAttribute(
      "aria-busy",
      String(
        !measured &&
          (track === downloadProgress
            ? ["preparing", "downloading", "verifying_package"].includes(
                update.stage,
              )
            : [
                "preflight",
                "awaiting_authorization",
                "staging",
                "verifying_staging",
                "closing_applications",
                "installing",
                "verifying_installation",
              ].includes(update.stage)),
      ),
    );
  }
}

async function prepareNativeUpdate(generation: number) {
  if (!invoke || !nativeUpdate) return;
  clearInterval(nativeProgressPoll);
  const pollingGeneration = ++nativeProgressGeneration;
  nativeProgressPoll = setInterval(async () => {
    try {
      const status = await invoke("update_status");
      if (
        pollingGeneration === nativeProgressGeneration &&
        generation === transitionGeneration &&
        stage === "details"
      )
        paintNativeUpdate(status);
    } catch {
      // Poll failure is not a successful installation or a reason to run NSIS.
    }
  }, 250);
  try {
    const result = await invoke("install_update", {
      operationId: nativeUpdate.operationId,
    });
    if (generation === transitionGeneration && stage === "details")
      paintNativeUpdate(result);
  } catch {
    if (generation === transitionGeneration && stage === "details")
      progressStatus.textContent = "Kurulum tamamlanamadı";
    if (maintenanceMode === "repair" && !maintenance.hidden)
      maintenanceStatus.textContent = "Onarım tamamlanamadı.";
  } finally {
    nativeProgressGeneration++;
    clearInterval(nativeProgressPoll);
    nativeProgressPoll = undefined;
  }
}

function startDemoProgress() {
  stopDemoProgress();
  clearInterval(nativeProgressPoll);
  nativeProgressPoll = undefined;
  paintDemoProgress(0);
  let started: number | undefined;
  function tick(time: number) {
    started ??= time;
    const elapsed = Math.min(time - started, 7000);
    paintDemoProgress(elapsed);
    if (elapsed < 7000) progressFrame = requestAnimationFrame(tick);
    else progressFrame = undefined;
  }
  // UI-only demo: no download, verification, file writes or installer command.
  progressFrame = requestAnimationFrame(tick);
}

async function openProgressPreview() {
  const generation = transitionGeneration;
  paintCompactSurface();
  setStage("details");
  flow.hidden = overlay.hidden = true;
  details.hidden = details.inert = false;
  details.style.opacity = "1";
  details.style.transform = "";
  try {
    await nativeScreen("details");
    if (generation !== transitionGeneration) return;
    details.focus({ preventScroll: true });
    startDemoProgress();
  } catch {
    if (generation !== transitionGeneration) return;
    transitionError.textContent =
      "Önizleme ekranı hazırlanamadı. Yeniden açmayı dene.";
    transitionError.hidden = false;
  }
}

function resetInstallationScreen() {
  maintenanceMode = undefined;
  maintenance.hidden = maintenance.inert = true;
  launchButton.hidden = true;
  installer.dataset.completed = "false";
  const needsRegionReset = stage !== "selection";
  transitionGeneration++;
  stopDemoProgress();
  clearInterval(nativeProgressPoll);
  nativeProgressPoll = undefined;
  paintDemoProgress(0);
  downloadProgress.removeAttribute("aria-busy");
  installProgress.removeAttribute("aria-busy");
  if (transitionFrame !== undefined) cancelAnimationFrame(transitionFrame);
  transitionFrame = undefined;
  transitionComplete?.(false);
  transitionComplete = undefined;
  nativeRegionPending = false;
  setStage("selection");
  installer.style.height = installer.style.top = "";
  installer.style.borderRadius = "";
  flow.hidden = false;
  flow.style.opacity = "1";
  details.hidden = details.inert = true;
  details.style.opacity = "0";
  details.style.transform = "";
  folder.dataset.dropActive = "false";
  appIconMotion.reset();
  if (needsRegionReset && invoke)
    void nativeScreen("selection").catch(() => {});
}

async function goToDetails() {
  if (stage !== "selection" || installer.dataset.locked !== "false")
    return false;
  if (
    !demoInstall &&
    nativeUpdate?.stage !== "available" &&
    !(nativeUpdate?.installedVersion && nativeUpdate.stage === "no_release")
  ) {
    transitionError.textContent =
      nativeUpdate?.stage === "no_release"
        ? "Kurulabilir bir sürüm henüz yayımlanmamış."
        : nativeUpdate?.stage === "blocked"
          ? "Bekleyen kurulum işlemi veya kurulum yapılandırması kontrol edilmeli."
          : "Kurulum kaynağı ve servis bağlantısı hazırlanamadı.";
    transitionError.hidden = false;
    appIconMotion.reset();
    return false;
  }
  const generation = ++transitionGeneration;
  transitionError.hidden = true;
  setStage("absorbing");
  folder.dataset.dropActive = "false";
  try {
    if (!(await appIconMotion.absorb()) || generation !== transitionGeneration)
      return false;
    setStage("contracting");
    // Keep the rendering scale fixed while the transparent top/bottom margins grow.
    const scale = window.innerWidth / 510;
    const initialHeight = window.innerHeight / scale;
    const finalHeight = compactHeight;
    const completed = await animateStep(150, (progress) => {
      const height = initialHeight + (finalHeight - initialHeight) * progress;
      installer.style.height = `${height}px`;
      installer.style.top = `${(initialHeight - height) / 2}px`;
      installer.style.borderRadius = `${25.5 - 12.75 * progress}px`;
      flow.style.opacity = String(1 - progress);
    });
    if (!completed || generation !== transitionGeneration) return false;
    // The native window and WebView stay full-size; only their active region changes.
    nativeRegionPending = true;
    await nativeScreen("details");
    if (generation !== transitionGeneration) return false;
    nativeRegionPending = false;
    flow.hidden = true;
    details.hidden = false;
    details.style.opacity = "0";
    details.style.transform = "translateY(4.25px)";
    setStage("revealing");
    if (
      !(await animateStep(150, (progress) => {
        details.style.opacity = String(progress);
        details.style.transform = `translateY(${4.25 * (1 - progress)}px)`;
      })) ||
      generation !== transitionGeneration
    )
      return false;
    setStage("details");
    details.inert = false;
    details.focus({ preventScroll: true });
    if (nativeUpdate?.installedVersion) {
      await openMaintenance("repair");
      return true;
    }
    if (demoInstall) startDemoProgress();
    else {
      // The service owns an accepted installation even if the UAC worker exits.
      clearInterval(elevationPoll);
      progressStatus.textContent = "Paket hazırlanıyor";
      void prepareNativeUpdate(generation);
    }
    return true;
  } catch {
    if (generation === transitionGeneration) {
      resetInstallationScreen();
      transitionError.textContent = "Ekran geçişi tamamlanamadı. Tekrar dene.";
      transitionError.hidden = false;
    }
    return false;
  }
}
folder.addEventListener("click", goToDetails);

async function openMaintenance(mode: "repair" | "uninstall") {
  maintenanceMode = mode;
  clearInterval(elevationPoll);
  flow.hidden = overlay.hidden = details.hidden = true;
  details.inert = true;
  launchButton.hidden = true;
  paintCompactSurface();
  setStage("details");
  await nativeScreen("details");
  maintenance.hidden = maintenance.inert = false;
  maintenanceTitle.textContent =
    mode === "repair" ? "BaykuşLig · Onarım" : "BaykuşLig · Kaldır";
  if (mode === "uninstall") {
    maintenanceStatus.textContent =
      "Uygulama kaldırılsın mı? Kullanıcı verileri korunur.";
    maintenanceAction.textContent = "Kaldır";
    maintenanceAction.disabled = false;
  } else {
    const available =
      nativeUpdate?.stage === "available" &&
      Boolean(nativeUpdate.installedVersion);
    const healthy =
      nativeUpdate?.installedHealthy === true &&
      nativeUpdate.intent !== "update" &&
      ["available", "no_release"].includes(nativeUpdate.stage);
    launchButton.hidden = !healthy;
    maintenanceStatus.textContent = healthy
      ? "Uygulama dosyaları sağlam. Onarım gerekmiyor."
      : available
        ? "Kontrol tamamlandı. Dosyalar onarılabilir veya güncellenebilir."
        : "Onarım kontrolü tamamlanamadı. Uygulama ve servis durumunu kontrol et.";
    maintenanceAction.textContent =
      nativeUpdate?.intent === "update" ? "Onar ve güncelle" : "Onar";
    maintenanceAction.disabled = !available || healthy;
  }
  maintenance.focus({ preventScroll: true });
}

maintenanceAction.addEventListener("click", async () => {
  if (maintenanceBusy || maintenanceAction.disabled || !invoke) return;
  maintenanceBusy = true;
  maintenanceAction.disabled = true;
  requiredElement<HTMLButtonElement>("#close").disabled = true;
  try {
    if (maintenanceMode === "uninstall") {
      maintenanceStatus.textContent =
        "Kaldırma izni bekleniyor / kaldırılıyor…";
      await invoke("uninstall_app");
      maintenanceStatus.textContent = "Uygulama başarıyla kaldırıldı.";
      maintenanceAction.hidden = true;
    } else {
      maintenanceStatus.textContent = "Onarım hazırlanıyor…";
      maintenanceAction.hidden = true;
      await prepareNativeUpdate(transitionGeneration);
    }
  } catch {
    maintenanceStatus.textContent =
      "Kaldırma tamamlanamadı veya yönetici izni iptal edildi.";
    maintenanceAction.disabled = false;
  } finally {
    maintenanceBusy = false;
    requiredElement<HTMLButtonElement>("#close").disabled = false;
  }
});

launchButton.addEventListener("click", async () => {
  if (!invoke) return;
  try {
    await invoke("launch_app");
  } catch {
    progressStatus.textContent = "Uygulama başlatılamadı.";
  }
});

function createAppIconMotion() {
  const button = requiredElement<HTMLButtonElement>("#app-icon");
  const image = requiredElement<HTMLImageElement>("img", button);
  const name = requiredElement<HTMLElement>("#app-name");
  const halfSize = button.offsetWidth / 2;
  const motionKeys = ["x", "y", "angle", "scale"] as const;
  const state: MotionState = { x: 0, y: 0, angle: 0, scale: 1 };
  const target = { ...state };
  const velocity = { x: 0, y: 0, angle: 0, scale: 0 };
  let frame: number | undefined;
  let lastTime: number | undefined;
  let drag: DragState | undefined;
  let nameFrame: number | undefined;
  let nameProgress = 0;
  let nameTarget = 0;

  function fadeName(hidden: boolean) {
    const to = hidden ? 1 : 0;
    if (nameTarget === to && nameFrame !== undefined) return;
    nameTarget = to;
    if (nameFrame !== undefined) cancelAnimationFrame(nameFrame);
    const from = nameProgress;
    function paintName(progress: number) {
      nameProgress = progress;
      name.style.filter = `blur(${51 * progress}px)`;
      name.style.opacity = String(1 - progress);
    }
    if (
      from === to ||
      window.matchMedia("(prefers-reduced-motion: reduce)").matches
    ) {
      nameFrame = undefined;
      paintName(to);
      return;
    }
    let started: number | undefined;
    function tickName(time: number) {
      started ??= time;
      const elapsed = Math.min((time - started) / 400, 1);
      const eased = hidden ? elapsed ** 3 : 1 - (1 - elapsed) ** 3;
      paintName(from + (to - from) * eased);
      if (elapsed < 1) nameFrame = requestAnimationFrame(tickName);
      else nameFrame = undefined;
    }
    nameFrame = requestAnimationFrame(tickName);
  }

  function paint() {
    image.style.transform = `translate(${state.x}px, ${state.y}px) rotate(${state.angle}deg) scale(${state.scale})`;
    if (drag) folder.dataset.dropActive = String(isOverFolder());
  }
  function isOverFolder() {
    if (!drag) return false;
    const { rect, bounds, zoom } = drag.geometry;
    const radians = (state.angle * Math.PI) / 180;
    const anchorX = drag?.anchorX ?? halfSize;
    const anchorY = drag?.anchorY ?? halfSize;
    const cx = (halfSize - anchorX) * state.scale;
    const cy = (halfSize - anchorY) * state.scale;
    // Transform the icon center around the picked point, including sway/scale.
    const centerX =
      rect.left +
      (anchorX + state.x + Math.cos(radians) * cx - Math.sin(radians) * cy) *
        zoom;
    const centerY =
      rect.top +
      (anchorY + state.y + Math.sin(radians) * cx + Math.cos(radians) * cy) *
        zoom;
    // Match the final 108% folder highlight, with a stable area during its tween.
    const paddingX = bounds.width * 0.04;
    const paddingY = bounds.height * 0.04;
    return (
      centerX >= bounds.left - paddingX &&
      centerX <= bounds.right + paddingX &&
      centerY >= bounds.top - paddingY &&
      centerY <= bounds.bottom + paddingY
    );
  }
  function tick(time: number) {
    let remaining = Math.min((time - (lastTime ?? time - 16)) / 1000, 0.032);
    lastTime = time;
    if (drag) {
      // The torque fades when the pointer stops; the icon settles at its anchor.
      target.angle =
        drag.sway * Math.exp(-Math.max(0, time - drag.time - 32) / 120);
    }
    while (remaining > 0) {
      const dt = Math.min(remaining, 1 / 120);
      for (const key of motionKeys) {
        velocity[key] +=
          (380 * (target[key] - state[key]) - 26 * velocity[key]) * dt;
        state[key] += velocity[key] * dt;
      }
      remaining -= dt;
    }
    paint();
    const settled = motionKeys.every(
      (key) =>
        Math.abs(target[key] - state[key]) < 0.0005 &&
        Math.abs(velocity[key]) < 0.005,
    );
    if (settled && (!drag || Math.abs(target.angle) < 0.0005)) {
      Object.assign(state, target);
      for (const key of motionKeys) velocity[key] = 0;
      paint();
      frame = lastTime = undefined;
    } else {
      frame = requestAnimationFrame(tick);
    }
  }
  function animate() {
    if (
      motionKeys.every(
        (key) => state[key] === target[key] && velocity[key] === 0,
      )
    ) {
      paint();
      return;
    }
    if (window.matchMedia("(prefers-reduced-motion: reduce)").matches) {
      if (frame !== undefined) cancelAnimationFrame(frame);
      frame = lastTime = undefined;
      Object.assign(state, target);
      for (const key of motionKeys) velocity[key] = 0;
      paint();
    } else if (frame === undefined) {
      frame = requestAnimationFrame(tick);
    }
  }
  function finish(accepted = false) {
    if (!drag) return;
    const { anchorX, anchorY, pointerId } = drag;
    const radians = (state.angle * Math.PI) / 180;
    const cx = (halfSize - anchorX) * state.scale;
    const cy = (halfSize - anchorY) * state.scale;
    // Move the pivot back to the center without changing the visible position.
    state.x +=
      anchorX - halfSize + Math.cos(radians) * cx - Math.sin(radians) * cy;
    state.y +=
      anchorY - halfSize + Math.sin(radians) * cx + Math.cos(radians) * cy;
    image.style.transformOrigin = "center";
    drag = undefined;
    button.dataset.dragging = "false";
    folder.dataset.dropActive = "false";
    fadeName(accepted);
    if (button.hasPointerCapture(pointerId))
      button.releasePointerCapture(pointerId);
    if (accepted) void goToDetails();
    else {
      Object.assign(target, { x: 0, y: 0, angle: 0, scale: 1 });
      animate();
    }
  }
  button.addEventListener("pointerenter", () => {
    if (
      !drag &&
      stage === "selection" &&
      installer.dataset.locked === "false"
    ) {
      target.scale = 1.15;
      animate();
    }
  });
  button.addEventListener("pointerleave", () => {
    if (!drag && stage === "selection") {
      target.scale = 1;
      animate();
    }
  });
  button.addEventListener("pointerdown", (event) => {
    if (
      event.button !== 0 ||
      drag ||
      stage !== "selection" ||
      installer.dataset.locked !== "false"
    )
      return;
    event.preventDefault();
    const rect = button.getBoundingClientRect();
    const zoom = rect.width / (halfSize * 2);
    const px = (event.clientX - rect.left) / zoom - halfSize - state.x;
    const py = (event.clientY - rect.top) / zoom - halfSize - state.y;
    const radians = (state.angle * Math.PI) / 180;
    const cos = Math.cos(radians);
    const sin = Math.sin(radians);
    const anchorX = halfSize + (cos * px + sin * py) / state.scale;
    const anchorY = halfSize + (-sin * px + cos * py) / state.scale;
    // A different transform origin must not make the enlarged icon jump.
    const ax = anchorX - halfSize;
    const ay = anchorY - halfSize;
    state.x += state.scale * (cos * ax - sin * ay) - ax;
    state.y += state.scale * (sin * ax + cos * ay) - ay;
    velocity.x = velocity.y = 0;
    image.style.transformOrigin = `${anchorX}px ${anchorY}px`;
    Object.assign(target, { x: state.x, y: state.y, scale: 1.15 });
    drag = {
      pointerId: event.pointerId,
      anchorX,
      anchorY,
      startX: event.clientX,
      startY: event.clientY,
      offsetX: state.x,
      offsetY: state.y,
      x: event.clientX,
      y: event.clientY,
      time: event.timeStamp,
      sway: 0,
      zoom,
      geometry: { rect, bounds: folder.getBoundingClientRect(), zoom },
    };
    button.dataset.dragging = "true";
    fadeName(true);
    button.setPointerCapture(event.pointerId);
    paint();
    animate();
  });
  button.addEventListener("pointermove", (event) => {
    if (!drag || event.pointerId !== drag.pointerId) return;
    const dt = Math.max((event.timeStamp - drag.time) / 1000, 1 / 240);
    const vx = (event.clientX - drag.x) / drag.zoom / dt;
    const vy = (event.clientY - drag.y) / drag.zoom / dt;
    const torque =
      (vx * (halfSize - drag.anchorY) - vy * (halfSize - drag.anchorX)) /
      halfSize;
    drag.sway = Math.max(-18, Math.min(18, torque * ((0.012 * 32) / 17)));
    drag.x = event.clientX;
    drag.y = event.clientY;
    drag.time = event.timeStamp;
    // Translation follows the pointer exactly; only rotation uses the spring.
    state.x = target.x =
      drag.offsetX + (event.clientX - drag.startX) / drag.zoom;
    state.y = target.y =
      drag.offsetY + (event.clientY - drag.startY) / drag.zoom;
    velocity.x = velocity.y = 0;
    target.angle = window.matchMedia("(prefers-reduced-motion: reduce)").matches
      ? 0
      : drag.sway;
    paint();
    animate();
  });
  for (const name of [
    "pointerup",
    "pointercancel",
    "lostpointercapture",
  ] as const) {
    button.addEventListener(name, (event) => {
      if (drag?.pointerId !== event.pointerId) return;
      if (
        name === "pointerup" &&
        Number.isFinite(event.clientX) &&
        Number.isFinite(event.clientY)
      ) {
        state.x = target.x =
          drag.offsetX + (event.clientX - drag.startX) / drag.zoom;
        state.y = target.y =
          drag.offsetY + (event.clientY - drag.startY) / drag.zoom;
      }
      const accepted = name === "pointerup" && isOverFolder();
      finish(accepted);
    });
  }
  window.addEventListener("keydown", (event) => {
    if (event.key === "Escape") finish();
  });
  return {
    absorb() {
      if (frame !== undefined) cancelAnimationFrame(frame);
      frame = lastTime = undefined;
      fadeName(true);
      const from = { ...state };
      const rect = button.getBoundingClientRect();
      const destination = folder.getBoundingClientRect();
      const zoom = rect.width / (halfSize * 2);
      const x =
        (destination.left +
          destination.width / 2 -
          rect.left -
          rect.width / 2) /
        zoom;
      const y =
        (destination.top +
          destination.height / 2 -
          rect.top -
          rect.height / 2) /
        zoom;
      return animateStep(300, (progress) => {
        state.x = from.x + (x - from.x) * progress;
        state.y = from.y + (y - from.y) * progress;
        state.scale = from.scale * (1 - progress);
        state.angle = from.angle * (1 - progress);
        image.style.opacity = String(1 - progress);
        paint();
      });
    },
    reset() {
      finish();
      image.style.opacity = "1";
      fadeName(false);
      Object.assign(target, { x: 0, y: 0, angle: 0, scale: 1 });
      animate();
    },
    dispose() {
      if (frame !== undefined) cancelAnimationFrame(frame);
      if (nameFrame !== undefined) cancelAnimationFrame(nameFrame);
    },
  };
}
const appIconMotion = createAppIconMotion();
window.addEventListener("blur", () => {
  if (stage === "selection") appIconMotion.reset();
});

function paintReveal(progress: number) {
  const remaining = 1 - progress;
  backgroundBlur.setAttribute("stdDeviation", String(4.25 * remaining));
  backgroundDisplacement.setAttribute("scale", String(5.3125 * remaining));
  overlay.style.backgroundColor = `rgb(0 0 0 / ${0.2 * remaining})`;
  shield.style.filter = `blur(${51 * progress}px)`;
  shield.style.opacity = String(remaining);
}

function revealInstallation(): Promise<void> {
  if (window.matchMedia("(prefers-reduced-motion: reduce)").matches) {
    paintReveal(1);
    return Promise.resolve();
  }
  return new Promise<void>((resolve) => {
    let started: number | undefined;
    function tick(time: number) {
      started ??= time;
      const elapsed = Math.min((time - started) / 400, 1);
      // Cubic ease-out: all effects start together, then slow to their endpoints.
      paintReveal(1 - (1 - elapsed) ** 3);
      if (elapsed < 1) {
        revealFrame = requestAnimationFrame(tick);
      } else {
        revealFrame = undefined;
        resolve();
      }
    }
    revealFrame = requestAnimationFrame(tick);
  });
}

function setElevated(approved: boolean) {
  if (!approved) {
    resetInstallationScreen();
    paintReveal(0);
    shield.style.filter = "";
    shield.style.opacity = "";
  }
  installer.dataset.locked = String(!approved);
  overlay.hidden = approved;
  flow.inert = !approved || stage !== "selection";
  clearInterval(elevationPoll);
  if (approved && invoke) {
    elevationPoll = setInterval(async () => {
      try {
        if (!(await invoke("elevation_state"))) setElevated(false);
      } catch {
        setElevated(false);
      }
    }, 2000);
  }
}

shield.addEventListener("click", async () => {
  if (elevationPending || startupPending || shield.disabled) return;
  elevationError.hidden = true;
  elevationPending = true;
  shield.disabled = true;
  shield.setAttribute("aria-busy", "true");
  try {
    // Finish the 85% → 100% press before the native UAC dialog covers the UI.
    await springTo(0.85);
    await springTo(1);
    if (!invoke) {
      elevationError.textContent =
        "Yönetici izni için Windows installer’ını aç.";
      elevationError.hidden = false;
      return;
    }
    if (!(await invoke("request_elevation")))
      throw new Error("elevation_failed");
    if (!demoInstall) nativeUpdate = await invoke("update_status");
    await revealInstallation();
    setElevated(true);
  } catch (error) {
    setElevated(false);
    if (String(error).includes("elevation_cancelled")) return;
    const failure = String(error).match(
      /\b(?:bootstrap_[a-z_]+|service_[a-z_]+|update_config_invalid)\b/,
    )?.[0];
    elevationError.textContent = failure
      ? `Kurulum servisi hazırlanamadı (${failure}).`
      : "Yönetici izni alınamadı. Tekrar denemek için kalkana tıkla.";
    elevationError.hidden = false;
  } finally {
    elevationPending = false;
    shield.disabled = startupPending;
    shield.removeAttribute("aria-busy");
  }
});

if (progressPreview) {
  void openProgressPreview();
} else if (invoke) {
  shield.disabled = true;
  shield.setAttribute("aria-busy", "true");
  // Existing native/platform and update-client checks; no startup UAC or install.
  const generation = transitionGeneration;
  void (async () => {
    try {
      const result = await invoke("startup_check");
      if (generation !== transitionGeneration) return;
      nativeUpdate = result.updates;
      if (result.entry === "repair" || result.entry === "uninstall") {
        startupPending = false;
        await openMaintenance(result.entry);
        return;
      }
      if (
        !result.inspection.platformSupported ||
        !result.inspection.webviewVersion
      ) {
        elevationError.textContent =
          "Windows veya WebView2 desteği doğrulanamadı.";
        elevationError.hidden = false;
        return;
      }
      startupPending = false;
      shield.disabled = false;
      if (await invoke("elevation_state")) {
        if (generation === transitionGeneration && !elevationPending)
          setElevated(true);
      }
    } catch {
      if (generation !== transitionGeneration) return;
      elevationError.textContent = "Başlangıç kontrolü tamamlanamadı.";
      elevationError.hidden = false;
    } finally {
      shield.removeAttribute("aria-busy");
    }
  })();
}
window.addEventListener("beforeunload", () => {
  transitionGeneration++;
  stopDemoProgress();
  clearInterval(nativeProgressPoll);
  if (transitionFrame !== undefined) cancelAnimationFrame(transitionFrame);
  transitionComplete?.(false);
  appIconMotion.dispose();
  clearInterval(elevationPoll);
  if (springFrame !== undefined) cancelAnimationFrame(springFrame);
  if (revealFrame !== undefined) cancelAnimationFrame(revealFrame);
});

function updateScale() {
  document.documentElement.style.setProperty(
    "--installer-scale",
    String(window.innerWidth / 510),
  );
}
updateScale();
window.addEventListener("resize", () => {
  updateScale();
  if (nativeRegionPending || stage === "revealing" || stage === "details")
    paintCompactSurface();
  else if (stage === "selection") appIconMotion.reset();
  else resetInstallationScreen();
});
window.addEventListener("contextmenu", (event) => event.preventDefault());

for (const action of ["minimize", "close"] as const) {
  requiredElement<HTMLButtonElement>(`#${action}`).addEventListener(
    "click",
    async () => {
      if (!invoke) return;
      try {
        await invoke("window_control", { action });
      } catch {
        // Window control failures do not start any installation operation.
      }
    },
  );
}
installer.addEventListener("mousedown", async (event) => {
  if (
    event.button !== 0 ||
    (event.target instanceof Element && event.target.closest("button")) ||
    !invoke
  )
    return;
  try {
    await invoke("window_control", { action: "drag" });
  } catch {
    // The window manager may decline dragging.
  }
});
