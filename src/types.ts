export type Band = "2.4GHz" | "5GHz" | "6GHz" | "Unknown";

export interface WifiNetwork {
  ssid: string;
  bssid: string;
  signalDbm: number;
  quality: number;
  channel: number;
  frequencyMhz: number;
  band: Band;
  security: string;
  isOpen: boolean;
  isEnterprise: boolean;
  isConnected: boolean;
}

export interface ChannelDistribution {
  band: Band;
  channel: number;
  networkCount: number;
}

export interface ScanResult {
  scannedAt: string;
  source: string;
  networks: WifiNetwork[];
  channelDistribution: ChannelDistribution[];
}

export interface HistoryPoint {
  time: number;
  dbm: number;
}

export type ScanIssueCode =
  | "locationPermissionRequired"
  | "locationPermissionDenied"
  | "locationServicesDisabled"
  | "wifiDisabled"
  | "adapterUnavailable"
  | "unsupportedPlatform"
  | "scanFailed";

export type ScanRecoveryAction =
  | "requestLocationPermission"
  | "openLocationSettings"
  | "openWifiSettings"
  | "retry";

export interface ScanIssue {
  code: ScanIssueCode;
  title: string;
  message: string;
  recoveryAction?: ScanRecoveryAction;
  details?: string;
}

export type DiagnosticOverall = "healthy" | "degraded" | "offline";
export type DiagnosticStatus = "pass" | "warning" | "fail";
export type DiagnosticCheckId = "wifi" | "gateway" | "dns" | "internet";

export interface DiagnosticCheck {
  id: DiagnosticCheckId;
  status: DiagnosticStatus;
  title: string;
  detail: string;
  latencyMs?: number;
  packetLossPercent?: number;
}

export interface ConnectionDiagnosticReport {
  checkedAt: string;
  overall: DiagnosticOverall;
  summary: string;
  checks: DiagnosticCheck[];
  connection?: { ssid: string; bssid?: string };
}
