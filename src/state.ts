import type {
  ConnectionDiagnosticReport,
  HistoryPoint,
  ScanIssue,
  ScanRecoveryAction,
  ScanResult,
  WifiNetwork,
} from "./types";

export const HISTORY_LIMIT = 36;

export interface AppState {
  scan?: ScanResult;
  selectedBssid?: string;
  history: Map<string, HistoryPoint[]>;
  autoScan: boolean;
  busy: boolean;
  scanIssue?: ScanIssue;
  settingsError?: string;
  recoveryBusy?: ScanRecoveryAction;
  diagnostics?: ConnectionDiagnosticReport;
  diagnosticBusy: boolean;
  diagnosticError?: string;
  diagnosticStale: boolean;
  connectionRevision: number;
}

export function createInitialState(): AppState {
  return {
    history: new Map(),
    autoScan: true,
    busy: false,
    diagnosticBusy: false,
    diagnosticStale: false,
    connectionRevision: 0,
  };
}

export function ingestHistory(state: AppState, scan: ScanResult): void {
  const now = Date.parse(scan.scannedAt);

  for (const network of scan.networks) {
    const points = state.history.get(network.bssid) ?? [];
    points.push({ time: now, dbm: network.signalDbm });
    state.history.set(network.bssid, points.slice(-HISTORY_LIMIT));
  }
}

export function getSelectedNetwork(state: AppState): WifiNetwork | undefined {
  const networks = state.scan?.networks ?? [];
  return networks.find((network) => network.bssid === state.selectedBssid) ?? networks[0];
}

export function getCurrentNetwork(state: AppState): WifiNetwork | undefined {
  if (state.scanIssue) {
    return undefined;
  }
  return state.scan?.networks.find((network) => network.isConnected);
}

export function diagnosticMatchesScan(report: ConnectionDiagnosticReport, scan?: ScanResult): boolean {
  if (!scan) {
    return true;
  }
  const current = scan.networks.find((network) => network.isConnected);
  if (!report.connection || !current) {
    return !report.connection && !current;
  }
  if (report.connection.bssid) {
    return report.connection.bssid.toLowerCase() === current.bssid.toLowerCase();
  }
  return report.connection.ssid === current.ssid;
}
