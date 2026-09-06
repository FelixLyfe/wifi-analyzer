import {
  Activity,
  CircleCheck,
  CircleX,
  createIcons,
  Globe2,
  MapPin,
  Radar,
  RefreshCw,
  Router,
  Settings,
  ShieldAlert,
  Signal,
  Wifi,
  WifiOff,
  Wrench,
} from "lucide";
import { buildCurveSvg } from "./chart";
import { buildConnectionStatus } from "./connection";
import {
  clamp,
  escapeAttr,
  escapeHtml,
  formatSourceLabel,
  formatTime,
  signalClass,
} from "./format";
import { getCurrentNetwork, getSelectedNetwork, type AppState } from "./state";
import type {
  Band,
  ChannelDistribution,
  ConnectionDiagnosticReport,
  DiagnosticCheckId,
  ScanIssue,
  ScanIssueCode,
  ScanRecoveryAction,
  WifiNetwork,
} from "./types";

export interface RenderHandlers {
  onSelectNetwork(bssid: string | undefined): void;
  onOpenWifiSettings(): void;
  onRetryScan(): void;
  onRecoveryAction(action: ScanRecoveryAction): void;
}

export function mountShell(root: HTMLElement): void {
  root.innerHTML = `
    <main class="shell">
      <header class="command-bar">
        <div class="app-lockup">
          <span class="app-icon" aria-hidden="true"><img src="/icon.png" alt="" /></span>
          <div class="app-title">
            <h1>Poliwave</h1>
            <p id="scanActivity" class="scan-activity" role="status" aria-live="polite">
              <span class="activity-dot" aria-hidden="true"></span>
              <span>准备扫描</span>
            </p>
          </div>
        </div>

        <section class="status-grid" aria-label="扫描摘要">
          <article class="metric metric-strong">
            <span>发现网络</span>
            <strong id="networkCount">0</strong>
          </article>
          <article class="metric">
            <span>当前信号</span>
            <strong id="currentSignal">--</strong>
          </article>
          <article class="metric">
            <span>当前频段</span>
            <strong id="currentBand">--</strong>
          </article>
          <article class="metric source-metric">
            <span>数据源</span>
            <strong id="scanSource">待扫描</strong>
          </article>
        </section>

        <div class="actions">
          <button id="scanBtn" class="button primary" type="button" aria-label="立即刷新 WiFi 扫描结果">
            <i data-lucide="radar"></i>
            <span>立即刷新</span>
          </button>
          <label class="toggle">
            <input id="autoScan" type="checkbox" role="switch" />
            <span class="toggle-track" aria-hidden="true"><span></span></span>
            <span class="toggle-label">每 5 秒</span>
          </label>
        </div>
      </header>

      <section class="workspace">
        <aside class="panel network-panel">
          <div class="panel-head">
            <div>
              <p class="panel-label">按信号强度排序</p>
              <h2>周围 WiFi</h2>
            </div>
            <span id="scanTime" class="stamp">--</span>
          </div>
          <div id="networkList" class="network-list empty-state" role="listbox" aria-label="周围 WiFi">点击扫描开始分析</div>
        </aside>

        <section class="analysis">
          <section class="panel curve-panel">
            <div class="panel-head">
              <div>
                <h2 id="curveTitle">RSSI 曲线</h2>
              </div>
              <span id="curveMeta" class="stamp">选择一个网络</span>
            </div>
            <div id="rssiCurve" class="curve empty-state">扫描后选择网络查看曲线</div>
            <div id="selectedDetail" class="selected-detail"></div>
          </section>

          <section class="insight-grid">
            <div class="panel connection-panel">
              <div class="panel-head">
                <div>
                  <p class="panel-label">逐层检查 WiFi、网关、DNS 与互联网</p>
                  <h2>连接诊断</h2>
                </div>
                <button id="diagnoseBtn" class="panel-action" type="button">
                  <i data-lucide="activity"></i>
                  <span>一键诊断</span>
                </button>
              </div>
              <div id="connectionStatus" class="connection-status-list empty-state">等待扫描结果</div>
            </div>

            <div class="panel distribution-panel">
              <div class="panel-head">
                <div>
                  <p class="panel-label">扫描数量，不代表实际信道负载</p>
                  <h2>周边网络分布</h2>
                </div>
                <span class="stamp">扫描估算</span>
              </div>
              <div id="channelDistribution" class="channel-chart empty-state">暂无分布数据</div>
            </div>
          </section>
        </section>
      </section>
    </main>
  `;
}

export function render(state: AppState, handlers: RenderHandlers): void {
  syncAutoScanInput(mustGet<HTMLInputElement>("autoScan"), state.autoScan);

  const scanBtn = mustGet<HTMLButtonElement>("scanBtn");
  scanBtn.disabled = state.busy;
  scanBtn.setAttribute("aria-busy", String(state.busy));
  scanBtn.classList.toggle("loading", state.busy);
  scanBtn.querySelector("span")!.textContent = state.busy ? "扫描中" : "立即刷新";

  const diagnoseBtn = mustGet<HTMLButtonElement>("diagnoseBtn");
  diagnoseBtn.disabled = state.diagnosticBusy;
  diagnoseBtn.setAttribute("aria-busy", String(state.diagnosticBusy));
  diagnoseBtn.classList.toggle("loading", state.diagnosticBusy);
  diagnoseBtn.querySelector("span")!.textContent = state.diagnosticBusy
    ? "诊断中"
    : state.diagnostics
      ? "重新诊断"
      : "一键诊断";

  const scanActivity = mustGet<HTMLElement>("scanActivity");
  scanActivity.className = `scan-activity ${state.busy ? "scanning" : state.scanIssue ? "error" : state.autoScan ? "active" : "paused"}`;
  scanActivity.querySelector("span:last-child")!.textContent = state.busy
    ? "正在扫描周围网络"
    : state.scanIssue
      ? "上次扫描失败"
      : state.autoScan
        ? "自动刷新已开启"
        : "自动刷新已暂停";

  const scan = state.scan;
  const selected = getSelectedNetwork(state);
  const current = getCurrentNetwork(state);

  setText("networkCount", state.scanIssue ? "--" : scan ? String(scan.networks.length) : "0");
  setText("currentSignal", state.scanIssue ? "未知" : current ? `${current.signalDbm} dBm` : scan ? "未连接" : "--");
  setText("currentBand", state.scanIssue ? "未知" : current?.band ?? (scan ? "未连接" : "--"));
  const sourceLabel = state.scanIssue ? "扫描失败" : scan ? formatSourceLabel(scan.source) : "待扫描";
  setText("scanSource", sourceLabel);
  mustGet<HTMLElement>("scanSource").title = state.scanIssue?.title ?? scan?.source ?? sourceLabel;
  setText("scanTime", scan ? `${state.scanIssue ? "上次成功 " : ""}${formatTime(scan.scannedAt)}` : "--");

  renderNetworks(state, scan?.networks ?? [], handlers);
  renderConnectionStatus(state, current, handlers);
  renderChannelDistribution(state, scan?.channelDistribution ?? [], current);
  renderCurve(state, selected);
  renderSelectedDetail(selected, Boolean(state.scanIssue));

  createIcons({
    icons: {
      Activity,
      CircleCheck,
      CircleX,
      Globe2,
      MapPin,
      Radar,
      RefreshCw,
      Router,
      Settings,
      ShieldAlert,
      Signal,
      Wifi,
      WifiOff,
      Wrench,
    },
  });
}

export function syncAutoScanInput(input: Pick<HTMLInputElement, "checked">, autoScan: boolean): void {
  input.checked = autoScan;
}

function renderNetworks(state: AppState, networks: WifiNetwork[], handlers: RenderHandlers): void {
  const list = mustGet<HTMLDivElement>("networkList");
  const activeElement = document.activeElement;
  const focusedBssid =
    activeElement instanceof HTMLButtonElement && activeElement.classList.contains("network-row")
      ? activeElement.dataset.bssid
      : undefined;

  if (state.scanIssue) {
    renderRecoveryGuide(list, state, handlers);
    return;
  }

  if (!networks.length) {
    list.className = "network-list empty-state";
    list.textContent = state.busy ? "正在读取无线网卡数据" : "未发现 WiFi";
    return;
  }

  list.className = "network-list";
  list.innerHTML = networks
    .map((network) => {
      const selected = network.bssid === state.selectedBssid;
      const connected = network.isConnected;
      return `
        <button class="network-row ${selected ? "selected" : ""} ${connected ? "connected" : ""}" type="button" role="option" data-bssid="${escapeAttr(network.bssid)}" aria-selected="${selected}" aria-label="查看 ${escapeAttr(network.ssid)} 的信号详情，信号 ${network.signalDbm} dBm" tabindex="${selected ? "0" : "-1"}">
          <span class="signal-mark ${signalClass(network.signalDbm)}"></span>
          <span class="network-main">
            <span class="network-title">
              <strong>${escapeHtml(network.ssid)}</strong>
              ${connected ? '<span class="connected-badge">当前网络</span>' : ""}
            </span>
            <small>${escapeHtml(network.bssid)} | CH ${network.channel || "--"} | ${network.frequencyMhz || "--"} MHz</small>
          </span>
          <span class="network-side">
            <b>${network.signalDbm} dBm</b>
            <small>${network.band}</small>
          </span>
          <span class="quality-bar" aria-hidden="true"><span style="width:${clamp(network.quality, 0, 100)}%"></span></span>
        </button>
      `;
    })
    .join("");

  const buttons = Array.from(list.querySelectorAll<HTMLButtonElement>(".network-row"));
  buttons.forEach((button, index) => {
    button.addEventListener("click", () => {
      handlers.onSelectNetwork(button.dataset.bssid);
    });
    button.addEventListener("keydown", (event) => {
      const targetIndex = getKeyboardTargetIndex(event.key, index, buttons.length);
      if (targetIndex === undefined) {
        return;
      }

      event.preventDefault();
      const target = buttons[targetIndex];
      target.focus({ preventScroll: true });
      target.scrollIntoView({ block: "nearest" });
      handlers.onSelectNetwork(target.dataset.bssid);
    });
  });

  if (focusedBssid) {
    buttons.find((button) => button.dataset.bssid === focusedBssid)?.focus({ preventScroll: true });
  }
}

function renderRecoveryGuide(
  root: HTMLDivElement,
  state: AppState,
  handlers: RenderHandlers,
): void {
  const issue = state.scanIssue!;
  const steps = recoverySteps(issue.code);
  const action = issue.recoveryAction;
  const actionLabel = action ? recoveryActionLabel(action) : undefined;
  const recoveryBusy = Boolean(state.recoveryBusy);
  const canRetry = issue.code !== "unsupportedPlatform";

  root.className = "network-list recovery-state";
  root.innerHTML = `
    <section class="recovery-guide" role="alert" aria-labelledby="recoveryTitle">
      <div class="recovery-heading">
        <span class="recovery-icon"><i data-lucide="wrench"></i></span>
        <div>
          <p class="panel-label">扫描恢复向导</p>
          <h3 id="recoveryTitle">${escapeHtml(issue.title)}</h3>
        </div>
      </div>
      <p class="recovery-message">${escapeHtml(issue.message)}</p>
      <ol class="recovery-steps">
        ${steps.map((step) => `<li>${escapeHtml(step)}</li>`).join("")}
      </ol>
      <div class="recovery-actions">
        ${
          action && actionLabel
            ? `<button class="status-action recovery-primary" type="button" data-recovery-action="${action}" ${recoveryBusy ? "disabled" : ""}>
                <i data-lucide="${recoveryActionIcon(action)}"></i>
                <span>${state.recoveryBusy === action ? "处理中" : actionLabel}</span>
              </button>`
            : ""
        }
        ${
          canRetry && action !== "retry"
            ? `<button class="status-action recovery-secondary" type="button" data-action="retry-scan" ${recoveryBusy || state.busy ? "disabled" : ""}>
                <i data-lucide="refresh-cw"></i>
                <span>${state.busy ? "扫描中" : "重新扫描"}</span>
              </button>`
            : ""
        }
      </div>
      ${state.settingsError ? `<p class="status-error" role="alert">${escapeHtml(state.settingsError)}</p>` : ""}
      ${
        issue.details
          ? `<details class="recovery-details"><summary>查看技术详情</summary><p>${escapeHtml(issue.details)}</p></details>`
          : ""
      }
    </section>
  `;

  root
    .querySelector<HTMLButtonElement>("[data-recovery-action]")
    ?.addEventListener("click", (event) => {
      const button = event.currentTarget as HTMLButtonElement;
      const selectedAction = button.dataset.recoveryAction as ScanRecoveryAction;
      if (selectedAction === "retry") {
        handlers.onRetryScan();
      } else {
        handlers.onRecoveryAction(selectedAction);
      }
    });
  root
    .querySelector<HTMLButtonElement>('[data-action="retry-scan"]')
    ?.addEventListener("click", handlers.onRetryScan);
}

export function recoverySteps(code: ScanIssueCode): string[] {
  const guides: Record<ScanIssueCode, string[]> = {
    locationPermissionRequired: [
      "点击“请求定位权限”。",
      "在系统弹窗中选择允许。",
      "返回 Poliwave 后重新扫描。",
    ],
    locationPermissionDenied: [
      "打开系统定位设置。",
      "允许 Poliwave 使用定位服务。",
      "返回 Poliwave 后重新扫描。",
    ],
    locationServicesDisabled: [
      "打开系统定位设置并开启定位服务。",
      "确认 Poliwave 的定位权限已开启。",
      "返回 Poliwave 后重新扫描。",
    ],
    wifiDisabled: [
      "打开系统 WiFi 设置。",
      "开启 WiFi 并等待无线网卡就绪。",
      "返回 Poliwave 后重新扫描。",
    ],
    adapterUnavailable: [
      "确认设备具有可用的 WiFi 网卡。",
      "检查网卡驱动或系统 WLAN 服务。",
      "恢复后重新扫描。",
    ],
    unsupportedPlatform: ["请在受支持的 macOS 或 Windows 设备上运行 Poliwave。"],
    scanFailed: [
      "确认 WiFi 已开启且系统权限正常。",
      "等待几秒后重新扫描。",
      "若仍失败，可展开技术详情定位系统原因。",
    ],
  };
  return guides[code];
}

export function recoveryActionLabel(action: ScanRecoveryAction): string {
  const labels: Record<ScanRecoveryAction, string> = {
    requestLocationPermission: "请求定位权限",
    openLocationSettings: "打开定位设置",
    openWifiSettings: "打开 WiFi 设置",
    retry: "重新扫描",
  };
  return labels[action];
}

function recoveryActionIcon(action: ScanRecoveryAction): string {
  const icons: Record<ScanRecoveryAction, string> = {
    requestLocationPermission: "map-pin",
    openLocationSettings: "settings",
    openWifiSettings: "wifi",
    retry: "refresh-cw",
  };
  return icons[action];
}

function renderConnectionStatus(
  state: AppState,
  current: WifiNetwork | undefined,
  handlers: RenderHandlers,
): void {
  const root = mustGet<HTMLDivElement>("connectionStatus");

  if (!state.scan && !state.scanIssue && !state.diagnostics && !state.diagnosticBusy && !state.diagnosticError) {
    root.className = "connection-status-list empty-state";
    root.textContent = state.busy ? "正在读取当前连接" : "扫描 WiFi，或直接运行一键诊断";
    return;
  }

  const items: ReturnType<typeof buildConnectionStatus> = state.scanIssue
    ? [{ tone: "notice", icon: "wifi-off", title: "当前连接状态未知", detail: "扫描失败，无法确认当前 WiFi 的信号和安全状态。请重新扫描。" }]
    : state.scan ? buildConnectionStatus(current) : [];
  root.className = "connection-status-list";
  root.innerHTML = `${items
    .map(
      (item) => `
        <article class="connection-status-item ${item.tone}">
          <i data-lucide="${item.icon}"></i>
          <div>
            <strong>${escapeHtml(item.title)}</strong>
            <p>${escapeHtml(item.detail)}</p>
            ${
              item.canOpenWifiSettings
                ? '<button class="status-action" type="button" data-action="open-wifi-settings">打开 WiFi 设置</button>'
                : ""
            }
          </div>
        </article>
      `,
    )
    .join("")}${
      state.settingsError && !state.scanIssue
        ? `<p class="status-error" role="alert">${escapeHtml(state.settingsError)}</p>`
        : ""
    }`;

  if (state.diagnosticBusy) {
    root.innerHTML += `
      <div class="diagnostic-progress" role="status" aria-live="polite">
        <i data-lucide="activity"></i>
        <div>
          <strong>正在逐层诊断</strong>
          <p>检查 WiFi、默认网关、DNS 和互联网连接，通常需要几秒。</p>
        </div>
      </div>
    `;
  } else {
    if (state.diagnostics) {
      root.innerHTML += renderDiagnosticReport(state.diagnostics, state.diagnosticStale);
    }
    if (state.diagnosticError) {
      root.innerHTML += `<p class="status-error" role="alert">诊断未完成：${escapeHtml(state.diagnosticError)}</p>`;
    }
  }

  root.querySelector<HTMLButtonElement>('[data-action="open-wifi-settings"]')?.addEventListener("click", () => {
    handlers.onOpenWifiSettings();
  });
}

function renderDiagnosticReport(
  report: ConnectionDiagnosticReport,
  stale: boolean,
): string {
  return `
    <section class="diagnostic-summary ${stale ? "stale" : report.overall}" aria-label="${stale ? "上次诊断" : "诊断结论"}">
      <i data-lucide="${stale ? "shield-alert" : report.overall === "healthy" ? "circle-check" : report.overall === "offline" ? "circle-x" : "shield-alert"}"></i>
      <div>
        <strong>${stale ? "上次诊断已过期" : diagnosticOverallLabel(report.overall)}</strong>
        <p>${stale ? "连接状态已变化或无法确认，以下为历史结果，请重新诊断。" : escapeHtml(report.summary)}</p>
      </div>
      <time datetime="${escapeAttr(report.checkedAt)}">${formatTime(report.checkedAt)}</time>
    </section>
    <div class="diagnostic-checks" aria-label="${stale ? "上次检查明细" : "本次检查明细"}">
      ${report.checks
        .map(
          (check) => `
            <article class="diagnostic-check ${stale ? "stale" : check.status}">
              <i data-lucide="${diagnosticCheckIcon(check.id)}"></i>
              <div>
                <strong>${escapeHtml(check.title)}</strong>
                <p>${escapeHtml(check.detail)}</p>
              </div>
            </article>
          `,
        )
        .join("")}
    </div>
  `;
}

function diagnosticOverallLabel(overall: ConnectionDiagnosticReport["overall"]): string {
  if (overall === "healthy") {
    return "连接正常";
  }
  if (overall === "degraded") {
    return "发现需要关注的项目";
  }
  return "连接不可用";
}

function diagnosticCheckIcon(id: DiagnosticCheckId): string {
  const icons: Record<DiagnosticCheckId, string> = {
    wifi: "wifi",
    gateway: "router",
    dns: "globe-2",
    internet: "activity",
  };
  return icons[id];
}

function renderChannelDistribution(
  state: AppState,
  distribution: ChannelDistribution[],
  current: WifiNetwork | undefined,
): void {
  const root = mustGet<HTMLDivElement>("channelDistribution");

  if (state.scanIssue) {
    root.className = "channel-chart empty-state";
    root.textContent = "扫描失败，周边分布暂不可用";
    return;
  }

  if (!distribution.length) {
    root.className = "channel-chart empty-state";
    root.textContent = state.busy ? "正在整理周边网络" : "暂无分布数据";
    return;
  }

  const bands: Band[] = ["2.4GHz", "5GHz", "6GHz"];
  const maxCount = Math.max(...distribution.map((item) => item.networkCount), 1);
  root.className = "channel-chart";
  root.innerHTML = bands
    .map((band) => {
      const items = distribution
        .filter((item) => item.band === band)
        .sort((a, b) => a.channel - b.channel);
      if (!items.length) {
        return "";
      }

      return `
        <div class="band-row">
          <div class="band-label">${band}</div>
          <div class="channel-bars">
            ${items
              .map((item) => {
                const isCurrent = current?.band === item.band && current.channel === item.channel;
                const height = Math.max(18, Math.round((item.networkCount / maxCount) * 100));
                return `
                  <div class="channel-item ${isCurrent ? "current" : ""}" title="CH ${item.channel}，本次扫描到 ${item.networkCount} 个 WiFi" aria-label="信道 ${item.channel}，本次扫描到 ${item.networkCount} 个 WiFi${isCurrent ? "，当前连接所在信道" : ""}">
                    <div class="channel-value">
                      <b>${item.networkCount}</b>
                      <div class="bar" style="height:${height}%"></div>
                    </div>
                    <span>${item.channel}</span>
                  </div>
                `;
              })
              .join("")}
          </div>
        </div>
      `;
    })
    .join("");
}

function renderCurve(state: AppState, network?: WifiNetwork): void {
  const root = mustGet<HTMLDivElement>("rssiCurve");
  const curveTitle = mustGet<HTMLHeadingElement>("curveTitle");
  const curveMeta = mustGet<HTMLSpanElement>("curveMeta");

  if (!network) {
    root.className = "curve empty-state";
    root.textContent = state.busy ? "等待扫描样本" : "扫描后选择网络查看曲线";
    curveTitle.textContent = "RSSI 曲线";
    curveMeta.textContent = "选择一个网络";
    return;
  }

  const points = state.history.get(network.bssid) ?? [{ time: Date.now(), dbm: network.signalDbm }];
  curveTitle.textContent = `${network.ssid} RSSI`;
  curveMeta.textContent = `${state.scanIssue ? `历史样本 · ${formatTime(state.scan!.scannedAt)} | ` : ""}${network.band} | CH ${network.channel || "--"}`;
  root.className = "curve";
  root.innerHTML = buildCurveSvg(points);
}

function renderSelectedDetail(network: WifiNetwork | undefined, stale: boolean): void {
  const root = mustGet<HTMLDivElement>("selectedDetail");
  const previousBssid = root.dataset.bssid;

  if (!network) {
    root.innerHTML = "";
    delete root.dataset.bssid;
    return;
  }

  root.innerHTML = `
    <div><span>SSID</span><strong>${escapeHtml(network.ssid)}</strong></div>
    <div><span>BSSID</span><strong>${escapeHtml(network.bssid)}</strong></div>
    <div><span>频段</span><strong>${network.band}</strong></div>
    <div><span>安全</span><strong>${escapeHtml(network.security)}</strong></div>
    <div><span>信号质量</span><strong>${network.quality}%</strong></div>
    <div><span>系统状态</span><strong>${stale ? "历史记录" : network.isConnected ? "当前使用" : "周边网络"}</strong></div>
  `;
  root.dataset.bssid = network.bssid;

  if (previousBssid && previousBssid !== network.bssid && !window.matchMedia("(prefers-reduced-motion: reduce)").matches) {
    root.getAnimations().forEach((animation) => animation.cancel());
    root.animate(
      [
        { opacity: 0.68, transform: "translateX(6px)" },
        { opacity: 1, transform: "translateX(0)" },
      ],
      { duration: 240, easing: "cubic-bezier(0.2, 0.8, 0.2, 1)" },
    );
  }
}

export function getKeyboardTargetIndex(key: string, currentIndex: number, itemCount: number): number | undefined {
  if (key === "ArrowDown") {
    return Math.min(currentIndex + 1, itemCount - 1);
  }
  if (key === "ArrowUp") {
    return Math.max(currentIndex - 1, 0);
  }
  if (key === "Home") {
    return 0;
  }
  if (key === "End") {
    return itemCount - 1;
  }
  return undefined;
}

function setText(id: string, value: string): void {
  mustGet<HTMLElement>(id).textContent = value;
}

function mustGet<T extends HTMLElement>(id: string): T {
  const element = document.getElementById(id);
  if (!element) {
    throw new Error(`Missing #${id}`);
  }
  return element as T;
}
