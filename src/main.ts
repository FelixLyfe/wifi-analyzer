import {
  diagnoseConnection,
  fetchScan,
  normalizeScanIssue,
  openLocationSettings,
  openWifiSettings,
  requestLocationPermission,
  WifiScanError,
} from "./ipc";
import { mountShell, render, type RenderHandlers } from "./render";
import { createInitialState, diagnosticMatchesScan, ingestHistory } from "./state";
import type { ScanRecoveryAction } from "./types";
import "./styles.css";

const state = createInitialState();
let autoScanTimer: number | undefined;

const app = document.querySelector<HTMLDivElement>("#app");

if (!app) {
  throw new Error("Missing #app root");
}

mountShell(app);

const handlers: RenderHandlers = {
  onSelectNetwork(bssid) {
    state.selectedBssid = bssid;
    rerender();
  },
  onOpenWifiSettings() {
    state.settingsError = undefined;
    rerender();
    void openWifiSettings().catch((error) => {
      state.settingsError = error instanceof Error ? error.message : String(error);
      rerender();
    });
  },
  onRetryScan() {
    void runScan();
  },
  onRecoveryAction(action) {
    void runRecoveryAction(action);
  },
};

const scanBtn = document.getElementById("scanBtn") as HTMLButtonElement;
const autoScanInput = document.getElementById("autoScan") as HTMLInputElement;
const diagnoseBtn = document.getElementById("diagnoseBtn") as HTMLButtonElement;

scanBtn.addEventListener("click", () => void runScan());
diagnoseBtn.addEventListener("click", () => void runDiagnostics());
autoScanInput.addEventListener("change", () => {
  state.autoScan = autoScanInput.checked;
  setupAutoScan();
  rerender();
});
rerender();
void runScan();
setupAutoScan();

function rerender(): void {
  render(state, handlers);
}

async function runScan(): Promise<void> {
  if (state.busy) {
    return;
  }

  state.busy = true;
  state.settingsError = undefined;
  rerender();

  try {
    const scan = await fetchScan();
    const previous = state.scan?.networks.find((network) => network.isConnected);
    const current = scan.networks.find((network) => network.isConnected);
    if (state.scan && previous?.bssid !== current?.bssid) {
      state.connectionRevision += 1;
      state.diagnosticStale = true;
    }
    state.scan = scan;
    state.scanIssue = undefined;
    if (state.diagnostics && !diagnosticMatchesScan(state.diagnostics, scan)) {
      state.diagnosticStale = true;
    }
    ingestHistory(state, scan);

    const selectionStillExists = scan.networks.some((network) => network.bssid === state.selectedBssid);
    if ((!state.selectedBssid || !selectionStillExists) && scan.networks[0]) {
      state.selectedBssid = current?.bssid ?? scan.networks[0].bssid;
    }
  } catch (error) {
    state.scanIssue = error instanceof WifiScanError ? error.issue : normalizeScanIssue(error);
    state.connectionRevision += 1;
    state.diagnosticStale = true;
    state.autoScan = false;
    setupAutoScan();
  } finally {
    state.busy = false;
    rerender();
  }
}

async function runDiagnostics(): Promise<void> {
  if (state.diagnosticBusy) {
    return;
  }

  state.diagnosticBusy = true;
  state.diagnosticError = undefined;
  const connectionRevision = state.connectionRevision;
  rerender();

  try {
    state.diagnostics = await diagnoseConnection();
    state.diagnosticStale =
      connectionRevision !== state.connectionRevision ||
      (!state.scanIssue && !diagnosticMatchesScan(state.diagnostics, state.scan));
  } catch (error) {
    state.diagnosticError = error instanceof Error ? error.message : String(error);
    state.diagnosticStale = true;
  } finally {
    state.diagnosticBusy = false;
    rerender();
  }
}

async function runRecoveryAction(action: ScanRecoveryAction): Promise<void> {
  if (action === "retry") {
    await runScan();
    return;
  }
  if (state.recoveryBusy) {
    return;
  }

  state.recoveryBusy = action;
  state.settingsError = undefined;
  rerender();

  try {
    if (action === "requestLocationPermission") {
      await requestLocationPermission();
    } else if (action === "openLocationSettings") {
      await openLocationSettings();
    } else {
      await openWifiSettings();
    }
  } catch (error) {
    state.settingsError = error instanceof Error ? error.message : String(error);
  } finally {
    state.recoveryBusy = undefined;
    rerender();
  }
}

function setupAutoScan(): void {
  if (autoScanTimer !== undefined) {
    window.clearInterval(autoScanTimer);
  }
  autoScanTimer = state.autoScan ? window.setInterval(() => void runScan(), 5000) : undefined;
}
