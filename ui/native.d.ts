// Browser bridge contracts mirror the native commands, not runtime validation.
declare namespace InstallerNative {
  type Screen = "selection" | "details";
  type WindowAction = "minimize" | "close" | "drag";
  type Stage =
    "selection" | "absorbing" | "contracting" | "revealing" | "details";

  interface UpdateStatus {
    installedHealthy?: boolean;
    installedVersion?: string | null;
    operationId: string;
    stage: string;
    downloadPercent: number | null;
    installPercent: number | null;
    bytes: number | null;
    totalBytes: number | null;
    intent: "install" | "update" | "repair" | null;
    version: string | null;
    releaseId: string | null;
    configured: boolean;
    serviceReady: boolean;
    error: string | null;
    preflight: unknown;
    capabilities: {
      check: boolean;
      prepare: boolean;
      install: boolean;
      repair: boolean;
      byteProgress: boolean;
      installedFileVerification: boolean;
    };
  }

  interface StartupResult {
    entry?: "selection" | "repair" | "uninstall";
    inspection: {
      platformSupported: boolean;
      webviewVersion: string | null;
    };
    updates: UpdateStatus;
  }

  interface Commands {
    uninstall_app: void;
    launch_app: void;
    startup_check: StartupResult;
    update_status: UpdateStatus;
    install_update: UpdateStatus;
    elevation_state: boolean;
    request_elevation: boolean;
    installer_screen: void;
    window_control: void;
  }

  interface Invoke {
    (command: "uninstall_app" | "launch_app"): Promise<void>;
    (command: "startup_check"): Promise<StartupResult>;
    (command: "update_status"): Promise<UpdateStatus>;
    (
      command: "install_update",
      args: { operationId: string },
    ): Promise<UpdateStatus>;
    (command: "elevation_state" | "request_elevation"): Promise<boolean>;
    (command: "installer_screen", args: { screen: Screen }): Promise<void>;
    (command: "window_control", args: { action: WindowAction }): Promise<void>;
  }
}

interface Window {
  __TAURI_INTERNALS__?: { invoke: InstallerNative.Invoke };
}
