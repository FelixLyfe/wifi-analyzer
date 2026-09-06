// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { diagnoseConnection, fetchScan, WifiScanError } from "./ipc";
import type { ConnectionDiagnosticReport, ScanResult, WifiNetwork } from "./types";

vi.mock("./ipc", async (importOriginal) => ({
  ...(await importOriginal<typeof import("./ipc")>()),
  fetchScan: vi.fn(),
  diagnoseConnection: vi.fn(),
}));

const networkA: WifiNetwork = {
  ssid: "Network-A", bssid: "00:00:00:00:00:01", signalDbm: -48, quality: 100,
  channel: 149, frequencyMhz: 5745, band: "5GHz", security: "WPA3",
  isOpen: false, isEnterprise: false, isConnected: true,
};
const networkB: WifiNetwork = {
  ...networkA, ssid: "Network-B", bssid: "00:00:00:00:00:02",
  signalDbm: -85, quality: 30, security: "Open", isOpen: true,
};
const wifiOff = new WifiScanError({
  code: "wifiDisabled", title: "WiFi 已关闭", message: "请开启 WiFi。", recoveryAction: "openWifiSettings",
});

function scan(network = networkA): ScanResult {
  return {
    scannedAt: new Date().toISOString(), source: "test", networks: [network],
    channelDistribution: [{ band: network.band, channel: network.channel, networkCount: 1 }],
  };
}

function report(network = networkA): ConnectionDiagnosticReport {
  const value = {
    checkedAt: new Date().toISOString(), overall: "healthy" as const,
    summary: "WiFi、DNS 与互联网连接均正常。",
    connection: { ssid: network.ssid, bssid: network.bssid },
    checks: [{ id: "wifi" as const, status: "pass" as const, title: "WiFi 已连接", detail: `当前连接：${network.ssid}` }],
  };
  return value;
}

function text(id: string): string {
  return document.getElementById(id)!.textContent!;
}

async function click(id: string): Promise<void> {
  document.getElementById(id)!.click();
  await vi.advanceTimersByTimeAsync(0);
}

beforeEach(async () => {
  vi.resetModules();
  vi.useFakeTimers();
  vi.mocked(fetchScan).mockReset().mockResolvedValue(scan());
  vi.mocked(diagnoseConnection).mockReset().mockResolvedValue(report());
  document.body.innerHTML = '<div id="app"></div>';
  vi.spyOn(window, "matchMedia").mockReturnValue({ matches: true } as MediaQueryList);
  await import("./main");
  await vi.advanceTimersByTimeAsync(0);
});

afterEach(() => {
  vi.clearAllTimers();
  vi.useRealTimers();
  vi.restoreAllMocks();
  document.body.innerHTML = "";
});

describe("scan and diagnostic lifecycle", () => {
  it("invalidates current metrics on scan failure while retaining labeled history through a retry", async () => {
    vi.mocked(fetchScan).mockRejectedValueOnce(wifiOff);
    await click("scanBtn");
    expect(text("networkList")).toContain("WiFi 已关闭");
    expect(text("currentSignal")).toBe("未知");
    expect(text("currentBand")).toBe("未知");
    expect(text("connectionStatus")).not.toContain("信号很强");
    expect(text("selectedDetail")).not.toContain("当前使用");
    expect(text("curveMeta")).toContain("历史样本");
    expect(document.querySelector("#rssiCurve svg")).not.toBeNull();

    let finish!: (value: ScanResult) => void;
    vi.mocked(fetchScan).mockImplementationOnce(() => new Promise((resolve) => { finish = resolve; }));
    await click("scanBtn");
    expect(text("currentSignal")).toBe("未知");
    finish(scan());
    await vi.advanceTimersByTimeAsync(0);
    expect(text("currentSignal")).toBe("-48 dBm");
    expect(text("curveMeta")).not.toContain("历史样本");
  });

  it("expires the previous report after switching networks and keeps current risk notices visible", async () => {
    await click("diagnoseBtn");
    vi.mocked(fetchScan).mockResolvedValue(scan(networkB));
    await click("scanBtn");
    expect(text("connectionStatus")).toContain("已过期");
    expect(text("connectionStatus")).toContain("当前网络安全性较低");
    expect(text("connectionStatus")).toContain("信号较弱");
    expect(document.querySelector(".diagnostic-summary")!.textContent).not.toContain("连接正常");
    expect(diagnoseConnection).toHaveBeenCalledTimes(1);

    vi.mocked(diagnoseConnection).mockResolvedValue(report(networkB));
    await click("diagnoseBtn");
    expect(text("connectionStatus")).not.toContain("已过期");
    expect(text("connectionStatus")).toContain("当前网络安全性较低");
  });

  it("expires a report when the BSSID changes even if the SSID stays the same", async () => {
    await click("diagnoseBtn");
    vi.mocked(fetchScan).mockResolvedValue(scan({ ...networkB, ssid: networkA.ssid }));
    await click("scanBtn");
    expect(text("connectionStatus")).toContain("已过期");
  });

  it("does not relabel a late diagnostic response as current after a network change", async () => {
    let finish!: (value: ConnectionDiagnosticReport) => void;
    vi.mocked(diagnoseConnection).mockImplementationOnce(() => new Promise((resolve) => { finish = resolve; }));
    await click("diagnoseBtn");
    vi.mocked(fetchScan).mockResolvedValue(scan(networkB));
    await click("scanBtn");
    finish(report());
    await vi.advanceTimersByTimeAsync(0);
    expect(text("connectionStatus")).toContain("已过期");
    expect(text("connectionStatus")).toContain("信号较弱");
  });

  it("keeps a matching report current across signal-only scans", async () => {
    await click("diagnoseBtn");
    vi.mocked(fetchScan).mockResolvedValue(scan({ ...networkA, signalDbm: -85 }));
    await click("scanBtn");
    expect(text("connectionStatus")).not.toContain("已过期");
    expect(text("connectionStatus")).toContain("连接正常");
    expect(text("connectionStatus")).toContain("信号较弱");
  });

  it("does not revive a previous report after failure and recovery on the same network", async () => {
    await click("diagnoseBtn");
    vi.mocked(fetchScan).mockRejectedValueOnce(wifiOff);
    await click("scanBtn");
    expect(text("connectionStatus")).toContain("已过期");
    await click("scanBtn");
    expect(text("connectionStatus")).toContain("已过期");
  });

  it("keeps a failed recheck from presenting the previous report as current", async () => {
    await click("diagnoseBtn");
    vi.mocked(diagnoseConnection).mockRejectedValueOnce(new Error("probe failed"));
    await click("diagnoseBtn");
    expect(text("connectionStatus")).toContain("诊断未完成");
    expect(text("connectionStatus")).toContain("已过期");
  });

  it("expires an in-flight report even if the connection changes away and back", async () => {
    let finish!: (value: ConnectionDiagnosticReport) => void;
    vi.mocked(diagnoseConnection).mockImplementationOnce(() => new Promise((resolve) => { finish = resolve; }));
    await click("diagnoseBtn");
    vi.mocked(fetchScan).mockResolvedValueOnce(scan(networkB));
    await click("scanBtn");
    await click("scanBtn");
    finish(report());
    await vi.advanceTimersByTimeAsync(0);
    expect(text("connectionStatus")).toContain("已过期");
  });
});
