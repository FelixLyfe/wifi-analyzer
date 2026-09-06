import { clamp } from "./format";
import type {
  Band,
  ChannelDistribution,
  ConnectionDiagnosticReport,
  ScanResult,
  WifiNetwork,
} from "./types";

export function demoScan(): ScanResult {
  const now = new Date().toISOString();
  const jitter = () => Math.round((Math.random() - 0.5) * 8);
  const networks: WifiNetwork[] = [
    makeDemo("Studio-5G", "8c:85:90:42:11:01", -49 + jitter(), 149, "WPA3", "5GHz"),
    makeDemo("Studio-IoT", "8c:85:90:42:11:02", -61 + jitter(), 6, "WPA2", "2.4GHz"),
    makeDemo("Neighbor-Living", "42:31:aa:09:c1:33", -72 + jitter(), 6, "WPA2", "2.4GHz"),
    makeDemo("CafeMesh", "00:25:9c:aa:78:2d", -67 + jitter(), 44, "WPA2", "5GHz"),
    makeDemo("Printer Setup", "c0:ff:ee:00:19:91", -82 + jitter(), 11, "Open", "2.4GHz"),
    makeDemo("Office-Guest", "28:ef:01:dd:22:91", -58 + jitter(), 36, "WPA2 Enterprise", "5GHz"),
  ]
    .map((network) => ({ ...network, isConnected: network.ssid === "Studio-5G" }))
    .sort((a, b) => b.signalDbm - a.signalDbm);

  const channelDistribution = buildDemoChannelDistribution(networks);

  return {
    scannedAt: now,
    source: "Browser demo data",
    networks,
    channelDistribution,
  };
}

export function demoDiagnostics(): ConnectionDiagnosticReport {
  return {
    checkedAt: new Date().toISOString(),
    overall: "healthy",
    summary: "WiFi、DNS 与互联网连接均正常。",
    connection: { ssid: "Studio-5G", bssid: "8c:85:90:42:11:01" },
    checks: [
      {
        id: "wifi",
        status: "pass",
        title: "WiFi 已连接",
        detail: "当前连接：Studio-5G",
      },
      {
        id: "gateway",
        status: "pass",
        title: "本地网关可达",
        detail: "192.168.1.1：平均延迟 2 ms，丢包 0%。",
        latencyMs: 2,
        packetLossPercent: 0,
      },
      {
        id: "dns",
        status: "pass",
        title: "DNS 解析正常",
        detail: "example.com 解析成功，耗时 18 ms。",
        latencyMs: 18,
      },
      {
        id: "internet",
        status: "pass",
        title: "互联网可达",
        detail: "公共网络检测端点：平均延迟 32 ms，丢包 0%。",
        latencyMs: 32,
        packetLossPercent: 0,
      },
    ],
  };
}

function makeDemo(
  ssid: string,
  bssid: string,
  signalDbm: number,
  channel: number,
  security: string,
  band: Band,
): WifiNetwork {
  return {
    ssid,
    bssid,
    signalDbm,
    quality: clamp((signalDbm + 100) * 2, 0, 100),
    channel,
    frequencyMhz: channel <= 14 ? 2407 + channel * 5 : 5000 + channel * 5,
    band,
    security,
    // demo 数据为静态样例，安全标记直接由样例文案得出；真实判定逻辑在后端。
    isOpen: security === "Open",
    isEnterprise: security.includes("Enterprise"),
    isConnected: false,
  };
}

function buildDemoChannelDistribution(networks: WifiNetwork[]): ChannelDistribution[] {
  const groups = new Map<string, WifiNetwork[]>();
  for (const network of networks) {
    const key = `${network.band}:${network.channel}`;
    groups.set(key, [...(groups.get(key) ?? []), network]);
  }

  return [...groups.entries()].map(([key, items]) => {
    const [band, channel] = key.split(":");
    return {
      band: band as Band,
      channel: Number(channel),
      networkCount: items.length,
    };
  });
}
