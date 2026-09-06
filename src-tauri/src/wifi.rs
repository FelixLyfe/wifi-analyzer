use crate::command_text;
use chrono::Utc;
use serde::Serialize;
use std::collections::BTreeMap;
use std::process::Command;

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

#[cfg(target_os = "macos")]
use objc2_core_location::{CLAuthorizationStatus, CLLocationManager};
#[cfg(target_os = "macos")]
use objc2_core_wlan::{CWChannelBand, CWNetwork, CWSecurity, CWWiFiClient};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WifiNetwork {
    pub ssid: String,
    pub bssid: String,
    pub signal_dbm: i32,
    pub quality: u8,
    pub channel: u16,
    pub frequency_mhz: u16,
    pub band: String,
    pub security: String,
    pub is_open: bool,
    pub is_enterprise: bool,
    pub is_connected: bool,
}

impl WifiNetwork {
    fn with_band(mut self, band: Option<WifiBand>) -> Self {
        if let Some(band) = band {
            self.frequency_mhz = band.frequency(self.channel);
            self.band = band.label().to_string();
        }
        self
    }
}

#[derive(Debug, Clone, Copy)]
enum WifiBand {
    Ghz2,
    Ghz5,
    Ghz6,
}

impl WifiBand {
    fn label(self) -> &'static str {
        match self {
            Self::Ghz2 => "2.4GHz",
            Self::Ghz5 => "5GHz",
            Self::Ghz6 => "6GHz",
        }
    }

    fn frequency(self, channel: u16) -> u16 {
        match (self, channel) {
            (Self::Ghz2, 1..=13) => 2407 + channel * 5,
            (Self::Ghz2, 14) => 2484,
            (Self::Ghz5, 1..=177) => 5000 + channel * 5,
            (Self::Ghz6, 2) => 5935,
            (Self::Ghz6, 1..=233) => 5950 + channel * 5,
            _ => 0,
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct NetworkChannel {
    number: u16,
    band: Option<WifiBand>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConnectionIdentity {
    pub ssid: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bssid: Option<String>,
}

#[derive(Debug, Clone, Default)]
struct CurrentConnection {
    ssid: String,
    bssid: Option<String>,
    channel: Option<u16>,
    signal_dbm: Option<i32>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelDistribution {
    pub band: String,
    pub channel: u16,
    pub network_count: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanResult {
    pub scanned_at: String,
    pub source: String,
    pub networks: Vec<WifiNetwork>,
    pub channel_distribution: Vec<ChannelDistribution>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ScanIssueCode {
    #[allow(dead_code)]
    LocationPermissionRequired,
    LocationPermissionDenied,
    #[allow(dead_code)]
    LocationServicesDisabled,
    WifiDisabled,
    AdapterUnavailable,
    #[allow(dead_code)]
    UnsupportedPlatform,
    ScanFailed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ScanRecoveryAction {
    #[allow(dead_code)]
    RequestLocationPermission,
    OpenLocationSettings,
    OpenWifiSettings,
    Retry,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanError {
    pub code: ScanIssueCode,
    pub title: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recovery_action: Option<ScanRecoveryAction>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,
}

impl ScanError {
    fn new(
        code: ScanIssueCode,
        title: impl Into<String>,
        message: impl Into<String>,
        recovery_action: Option<ScanRecoveryAction>,
        details: Option<String>,
    ) -> Self {
        Self {
            code,
            title: title.into(),
            message: message.into(),
            recovery_action,
            details,
        }
    }

    fn scan_failed(details: impl Into<String>) -> Self {
        Self::new(
            ScanIssueCode::ScanFailed,
            "扫描失败",
            "系统没有返回可用的 WiFi 扫描结果，请稍后重试。",
            Some(ScanRecoveryAction::Retry),
            Some(details.into()),
        )
    }
}

pub fn scan() -> Result<ScanResult, ScanError> {
    #[cfg(target_os = "macos")]
    let (source, mut networks) = scan_macos()?;

    #[cfg(target_os = "windows")]
    let (source, mut networks) = {
        let (source, raw) = scan_raw().map_err(|error| classify_windows_scan_error(&error))?;
        let networks = parse_by_platform(&raw);
        if networks.is_empty() {
            if let Some(issue) = windows_scan_issue_from_text(&raw) {
                return Err(issue);
            }
        }
        (source, networks)
    };

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    return Err(ScanError::new(
        ScanIssueCode::UnsupportedPlatform,
        "当前系统暂不支持",
        "Poliwave 的真实 WiFi 扫描目前仅支持 macOS 和 Windows。",
        None,
        None,
    ));

    mark_current_connection(&mut networks);

    networks.sort_by(|a, b| {
        b.signal_dbm
            .cmp(&a.signal_dbm)
            .then_with(|| a.ssid.to_lowercase().cmp(&b.ssid.to_lowercase()))
    });

    let channel_distribution = build_channel_distribution(&networks);

    Ok(ScanResult {
        scanned_at: Utc::now().to_rfc3339(),
        source,
        networks,
        channel_distribution,
    })
}

#[cfg(target_os = "macos")]
pub fn request_location_authorization() {
    // The prompt is asynchronous, so retain the manager for the process lifetime.
    unsafe {
        let manager = CLLocationManager::new();
        manager.requestWhenInUseAuthorization();
        std::mem::forget(manager);
    }
}

#[cfg(target_os = "macos")]
fn scan_macos() -> Result<(String, Vec<WifiNetwork>), ScanError> {
    ensure_macos_scan_ready()?;

    let core_wlan_error = match scan_core_wlan() {
        Ok(networks) if has_displayable_ssid(&networks) => {
            return Ok(("CoreWLAN".to_string(), networks));
        }
        Ok(_) => "CoreWLAN 未返回可显示的 WiFi 名称，请允许应用访问定位服务。".to_string(),
        Err(error) => error,
    };

    let (source, raw) = scan_raw().map_err(|fallback_error| {
        ScanError::scan_failed(format!(
            "{core_wlan_error} 兼容扫描也失败：{fallback_error}"
        ))
    })?;
    let networks: Vec<_> = parse_by_platform(&raw)
        .into_iter()
        .filter(|network| !is_non_displayable_ssid(&network.ssid))
        .collect();

    if networks.is_empty() {
        Err(ScanError::scan_failed(core_wlan_error))
    } else {
        Ok((source, networks))
    }
}

#[cfg(target_os = "macos")]
fn ensure_macos_scan_ready() -> Result<(), ScanError> {
    unsafe {
        if !CLLocationManager::locationServicesEnabled_class() {
            return Err(ScanError::new(
                ScanIssueCode::LocationServicesDisabled,
                "定位服务已关闭",
                "macOS 需要定位服务才能向应用提供附近 WiFi 的真实名称。Poliwave 不会读取或保存您的位置。",
                Some(ScanRecoveryAction::OpenLocationSettings),
                None,
            ));
        }

        let manager = CLLocationManager::new();
        match manager.authorizationStatus() {
            CLAuthorizationStatus::NotDetermined => {
                return Err(ScanError::new(
                    ScanIssueCode::LocationPermissionRequired,
                    "需要定位权限",
                    "允许 Poliwave 使用定位服务后，macOS 才会返回附近 WiFi 的真实名称。",
                    Some(ScanRecoveryAction::RequestLocationPermission),
                    None,
                ));
            }
            CLAuthorizationStatus::Denied => {
                return Err(ScanError::new(
                    ScanIssueCode::LocationPermissionDenied,
                    "定位权限已被拒绝",
                    "请在系统设置的定位服务中允许 Poliwave，然后返回重新扫描。",
                    Some(ScanRecoveryAction::OpenLocationSettings),
                    None,
                ));
            }
            CLAuthorizationStatus::Restricted => {
                return Err(ScanError::new(
                    ScanIssueCode::LocationPermissionDenied,
                    "定位权限受到系统限制",
                    "当前系统策略不允许 Poliwave 使用定位服务，请检查家长控制或设备管理策略。",
                    Some(ScanRecoveryAction::OpenLocationSettings),
                    None,
                ));
            }
            CLAuthorizationStatus::AuthorizedAlways
            | CLAuthorizationStatus::AuthorizedWhenInUse => {}
            _ => {
                return Err(ScanError::new(
                    ScanIssueCode::LocationPermissionRequired,
                    "无法确认定位权限",
                    "请重新授权定位服务后再扫描。",
                    Some(ScanRecoveryAction::RequestLocationPermission),
                    None,
                ));
            }
        }

        let client = CWWiFiClient::sharedWiFiClient();
        let interface = client.interface().ok_or_else(|| {
            ScanError::new(
                ScanIssueCode::AdapterUnavailable,
                "未找到 WiFi 网卡",
                "系统没有提供可用的 WiFi 网卡，请检查硬件或系统网络配置。",
                Some(ScanRecoveryAction::Retry),
                None,
            )
        })?;
        if !interface.powerOn() {
            return Err(ScanError::new(
                ScanIssueCode::WifiDisabled,
                "WiFi 已关闭",
                "请先在系统设置中开启 WiFi，然后返回重新扫描。",
                Some(ScanRecoveryAction::OpenWifiSettings),
                None,
            ));
        }
    }

    Ok(())
}

#[cfg(target_os = "macos")]
fn scan_core_wlan() -> Result<Vec<WifiNetwork>, String> {
    unsafe {
        let client = CWWiFiClient::sharedWiFiClient();
        let interface = client
            .interface()
            .ok_or_else(|| "CoreWLAN 未找到 Wi-Fi 网卡。".to_string())?;
        let current_connection = interface.ssid().map(|value| CurrentConnection {
            ssid: value.to_string(),
            bssid: interface
                .bssid()
                .map(|value| value.to_string().to_ascii_lowercase())
                .filter(|value| is_mac_address(value)),
            channel: interface
                .wlanChannel()
                .map(|value| value.channelNumber().clamp(0, u16::MAX as isize) as u16),
            signal_dbm: Some(
                interface
                    .rssiValue()
                    .clamp(i32::MIN as isize, i32::MAX as isize) as i32,
            ),
        });
        let scanned = interface
            .scanForNetworksWithSSID_error(None)
            .map_err(|error| format!("CoreWLAN 扫描失败：{error}"))?;

        let mut networks = Vec::with_capacity(scanned.len());
        for network in &*scanned {
            let Some(ssid) = network.ssid().map(|value| value.to_string()) else {
                continue;
            };
            if is_non_displayable_ssid(&ssid) {
                continue;
            }

            let channel = network
                .wlanChannel()
                .map(|value| NetworkChannel {
                    number: value.channelNumber().clamp(0, u16::MAX as isize) as u16,
                    band: match value.channelBand() {
                        CWChannelBand::Band2GHz => Some(WifiBand::Ghz2),
                        CWChannelBand::Band5GHz => Some(WifiBand::Ghz5),
                        CWChannelBand::Band6GHz => Some(WifiBand::Ghz6),
                        _ => None,
                    },
                })
                .unwrap_or_default();
            let bssid = network
                .bssid()
                .map(|value| value.to_string().to_ascii_lowercase())
                .filter(|value| is_mac_address(value))
                .unwrap_or_else(|| synthetic_bssid(&ssid, channel.number, networks.len()));
            let parsed = make_network(
                ssid.clone(),
                bssid.clone(),
                network
                    .rssiValue()
                    .clamp(i32::MIN as isize, i32::MAX as isize) as i32,
                channel.number,
                core_wlan_security(&network),
                None,
            )
            .with_band(channel.band);
            networks.push(parsed);
        }

        mark_current_connection_from(&mut networks, current_connection.as_ref());

        Ok(networks)
    }
}

#[cfg(target_os = "macos")]
unsafe fn core_wlan_security(network: &CWNetwork) -> String {
    let candidates = [
        (CWSecurity::WPA3Enterprise, "WPA3 Enterprise"),
        (CWSecurity::WPA3Personal, "WPA3 Personal"),
        (CWSecurity::WPA3Transition, "WPA3/WPA2 Personal"),
        (CWSecurity::WPA2Enterprise, "WPA2 Enterprise"),
        (CWSecurity::WPA2Personal, "WPA2 Personal"),
        (CWSecurity::WPAEnterpriseMixed, "WPA/WPA2 Enterprise"),
        (CWSecurity::WPAPersonalMixed, "WPA/WPA2 Personal"),
        (CWSecurity::WPAEnterprise, "WPA Enterprise"),
        (CWSecurity::WPAPersonal, "WPA Personal"),
        (CWSecurity::Enterprise, "Enterprise"),
        (CWSecurity::Personal, "Personal"),
        (CWSecurity::DynamicWEP, "Dynamic WEP"),
        (CWSecurity::WEP, "WEP"),
        (CWSecurity::OWE, "OWE"),
        (CWSecurity::OWETransition, "OWE Transition"),
    ];

    candidates
        .into_iter()
        .find_map(|(security, label)| network.supportsSecurity(security).then_some(label))
        .unwrap_or_else(|| {
            if network.supportsSecurity(CWSecurity::None) {
                "Open"
            } else {
                "Unknown"
            }
        })
        .to_string()
}

#[cfg(target_os = "macos")]
fn scan_raw() -> Result<(String, String), String> {
    let airport =
        "/System/Library/PrivateFrameworks/Apple80211.framework/Versions/Current/Resources/airport";
    if let Ok(raw) = run_command(airport, &["-s"]) {
        let networks = parse_airport(&raw);
        if !networks.is_empty() && has_displayable_ssid(&networks) {
            return Ok(("airport -s".to_string(), raw));
        }
    }

    run_command("system_profiler", &["SPAirPortDataType"])
        .map(|raw| ("system_profiler SPAirPortDataType".to_string(), raw))
}

#[cfg(target_os = "windows")]
fn scan_raw() -> Result<(String, String), String> {
    let bssid_result = run_command("netsh", &["wlan", "show", "networks", "mode=bssid"]);
    if let Ok(raw) = bssid_result.as_ref() {
        return Ok((
            "netsh wlan show networks mode=bssid".to_string(),
            raw.clone(),
        ));
    }

    let basic_result = run_command("netsh", &["wlan", "show", "networks"]);
    if let Ok(raw) = basic_result.as_ref() {
        return Ok(("netsh wlan show networks".to_string(), raw.clone()));
    }

    let interfaces_result = run_command("netsh", &["wlan", "show", "interfaces"]);
    if let Ok(raw) = interfaces_result.as_ref() {
        return Ok(("netsh wlan show interfaces".to_string(), raw.clone()));
    }

    Err(format!(
        "{}; fallback netsh wlan show networks failed: {}; fallback netsh wlan show interfaces failed: {}",
        bssid_result.err().unwrap_or_else(|| "unknown netsh failure".to_string()),
        basic_result.err().unwrap_or_else(|| "unknown netsh failure".to_string()),
        interfaces_result
            .err()
            .unwrap_or_else(|| "unknown netsh failure".to_string())
    ))
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn classify_windows_scan_error(details: &str) -> ScanError {
    windows_scan_issue_from_text(details).unwrap_or_else(|| ScanError::scan_failed(details))
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn windows_scan_issue_from_text(raw: &str) -> Option<ScanError> {
    let normalized = raw.to_ascii_lowercase();

    if normalized.contains("no wireless interface")
        || raw.contains("没有无线接口")
        || raw.contains("未找到无线接口")
        || normalized.contains("wlansvc service is not running")
        || normalized.contains("wireless autoconfig service")
        || normalized.contains("wlansvc) is not running")
    {
        return Some(ScanError::new(
            ScanIssueCode::AdapterUnavailable,
            "未找到可用的 WiFi 网卡",
            "Windows 没有提供可用的无线接口，请检查网卡、驱动或 WLAN AutoConfig 服务。",
            Some(ScanRecoveryAction::Retry),
            Some(raw.trim().to_string()),
        ));
    }

    if normalized.contains("software off")
        || normalized.contains("radio is off")
        || raw.contains("软件关闭")
        || raw.contains("无线电已关闭")
    {
        return Some(ScanError::new(
            ScanIssueCode::WifiDisabled,
            "WiFi 已关闭",
            "请先在 Windows WiFi 设置中开启无线网络，然后返回重新扫描。",
            Some(ScanRecoveryAction::OpenWifiSettings),
            Some(raw.trim().to_string()),
        ));
    }

    if normalized.contains("location permission")
        || normalized.contains("access is denied")
        || raw.contains("定位权限")
        || raw.contains("位置权限")
        || raw.contains("访问被拒绝")
        || raw.contains("拒绝访问")
    {
        return Some(ScanError::new(
            ScanIssueCode::LocationPermissionDenied,
            "需要 Windows 定位权限",
            "Windows 需要定位权限才能扫描附近 WiFi，请在隐私设置中允许定位访问。",
            Some(ScanRecoveryAction::OpenLocationSettings),
            Some(raw.trim().to_string()),
        ));
    }

    None
}

fn run_command(program: &str, args: &[&str]) -> Result<String, String> {
    let mut command = Command::new(program);
    command.args(args);
    #[cfg(target_os = "windows")]
    command.creation_flags(0x0800_0000);

    let output = command
        .output()
        .map_err(|err| format!("Failed to run {program}: {err}"))?;

    if !output.status.success() {
        let stdout = command_text::decode(&output.stdout).trim().to_string();
        let stderr = command_text::decode(&output.stderr).trim().to_string();
        return Err(if stderr.is_empty() {
            if stdout.is_empty() {
                format!("{program} exited with status {}", output.status)
            } else {
                stdout
            }
        } else {
            stderr
        });
    }

    Ok(command_text::decode(&output.stdout))
}

#[cfg(target_os = "macos")]
fn parse_by_platform(raw: &str) -> Vec<WifiNetwork> {
    if raw.contains("Current Network Information:") || raw.contains("Other Local Wi-Fi Networks:") {
        parse_system_profiler_airport(raw)
    } else {
        parse_airport(raw)
    }
}

#[cfg(target_os = "windows")]
fn parse_by_platform(raw: &str) -> Vec<WifiNetwork> {
    parse_windows_netsh(raw)
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn parse_by_platform(_raw: &str) -> Vec<WifiNetwork> {
    Vec::new()
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn parse_airport(raw: &str) -> Vec<WifiNetwork> {
    raw.lines()
        .skip(1)
        .filter_map(|line| {
            let tokens: Vec<&str> = line.split_whitespace().collect();
            let bssid_index = tokens.iter().position(|token| is_mac_address(token))?;
            if bssid_index == 0 || tokens.len() <= bssid_index + 2 {
                return None;
            }

            let ssid = tokens[..bssid_index].join(" ");
            let bssid = tokens[bssid_index].to_lowercase();
            let signal_dbm = tokens.get(bssid_index + 1)?.parse::<i32>().ok()?;
            let channel = parse_channel(tokens.get(bssid_index + 2)?)?;
            let security = tokens
                .get(bssid_index + 6..)
                .map(|items| items.join(" "))
                .unwrap_or_else(|| "Unknown".to_string());

            Some(make_network(
                ssid, bssid, signal_dbm, channel, security, None,
            ))
        })
        .collect()
}

#[cfg(target_os = "macos")]
#[derive(Debug, Default)]
struct SystemProfilerNetwork {
    ssid: String,
    channel: u16,
    band: Option<WifiBand>,
    signal_dbm: Option<i32>,
    security: String,
    is_connected: bool,
}

#[cfg(target_os = "macos")]
fn parse_system_profiler_airport(raw: &str) -> Vec<WifiNetwork> {
    let mut networks = Vec::new();
    let mut in_network_section = false;
    let mut section_is_connected = false;
    let mut current: Option<SystemProfilerNetwork> = None;

    for line in raw.lines() {
        let trimmed = line.trim();

        if trimmed == "Current Network Information:" || trimmed == "Other Local Wi-Fi Networks:" {
            push_system_profiler_network(&mut networks, current.take());
            in_network_section = true;
            section_is_connected = trimmed == "Current Network Information:";
            continue;
        }

        if !in_network_section || trimmed.is_empty() {
            continue;
        }

        if is_system_profiler_section_boundary(trimmed) {
            push_system_profiler_network(&mut networks, current.take());
            in_network_section = false;
            continue;
        }

        if trimmed.ends_with(':') && value_after_colon(trimmed).unwrap_or_default().is_empty() {
            push_system_profiler_network(&mut networks, current.take());
            current = Some(SystemProfilerNetwork {
                ssid: trimmed.trim_end_matches(':').to_string(),
                security: "Unknown".to_string(),
                is_connected: section_is_connected,
                ..Default::default()
            });
            continue;
        }

        let Some(network) = current.as_mut() else {
            continue;
        };

        if let Some(value) = trimmed.strip_prefix("Channel:") {
            network.channel = parse_channel(value.trim()).unwrap_or(0);
            network.band = parse_channel_band(value);
        } else if let Some(value) = trimmed.strip_prefix("Security:") {
            network.security = value.trim().to_string();
        } else if let Some(value) = trimmed.strip_prefix("Signal / Noise:") {
            network.signal_dbm = parse_signal_dbm(value);
        }
    }

    push_system_profiler_network(&mut networks, current);
    networks
}

#[cfg(target_os = "macos")]
fn push_system_profiler_network(
    networks: &mut Vec<WifiNetwork>,
    network: Option<SystemProfilerNetwork>,
) {
    let Some(network) = network else {
        return;
    };

    if network.ssid.is_empty() || network.channel == 0 {
        return;
    }

    let signal_dbm = network.signal_dbm.unwrap_or(-82);
    let bssid = synthetic_bssid(&network.ssid, network.channel, networks.len());

    let mut parsed = make_network(
        network.ssid,
        bssid,
        signal_dbm,
        network.channel,
        network.security,
        None,
    )
    .with_band(network.band);
    parsed.is_connected = network.is_connected;
    networks.push(parsed);
}

#[cfg(target_os = "macos")]
fn is_system_profiler_section_boundary(trimmed: &str) -> bool {
    matches!(
        trimmed,
        "Interfaces:" | "Software Versions:" | "Supported Channels:"
    )
}

// 以下各平台解析函数在所有平台编译，以便单元测试跨平台覆盖；
// 仅在未使用的平台上豁免 dead_code。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn parse_windows_netsh(raw: &str) -> Vec<WifiNetwork> {
    if looks_like_netsh_interfaces(raw) {
        let interfaces = parse_netsh_interfaces(raw);
        if !interfaces.is_empty() {
            return interfaces;
        }
    }

    let bssid_networks = parse_netsh(raw);
    if !bssid_networks.is_empty() {
        return bssid_networks;
    }

    parse_netsh_ssid_only(raw)
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn looks_like_netsh_interfaces(raw: &str) -> bool {
    raw.lines().any(|line| {
        let trimmed = line.trim();
        (trimmed.starts_with("State") || trimmed.starts_with("状态")) && trimmed.contains(':')
    })
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn parse_netsh(raw: &str) -> Vec<WifiNetwork> {
    let mut networks = Vec::new();
    let mut current_ssid = String::new();
    let mut current_security = String::from("Unknown");
    let mut current_bssid = String::new();
    let mut current_quality: Option<u8> = None;
    let mut current_channel = NetworkChannel::default();

    for line in raw.lines() {
        let trimmed = line.trim();

        if trimmed.starts_with("SSID ") && trimmed.contains(':') {
            push_netsh_network(
                &mut networks,
                &current_ssid,
                &current_security,
                &current_bssid,
                current_quality,
                current_channel,
            );
            current_bssid.clear();
            current_quality = None;
            current_channel = NetworkChannel::default();
            current_ssid = value_after_colon(trimmed).unwrap_or_default().to_string();
        } else if trimmed.starts_with("Authentication") || trimmed.starts_with("身份验证") {
            current_security = value_after_colon(trimmed).unwrap_or("Unknown").to_string();
        } else if trimmed.starts_with("BSSID ") && trimmed.contains(':') {
            push_netsh_network(
                &mut networks,
                &current_ssid,
                &current_security,
                &current_bssid,
                current_quality,
                current_channel,
            );
            current_bssid = value_after_colon(trimmed)
                .unwrap_or_default()
                .to_lowercase();
            current_quality = None;
            current_channel = NetworkChannel::default();
        } else if trimmed.starts_with("Signal") || trimmed.starts_with("信号") {
            current_quality = value_after_colon(trimmed)
                .and_then(|value| value.trim_end_matches('%').trim().parse::<u8>().ok());
        } else if trimmed.starts_with("Channel")
            || trimmed.starts_with("频道")
            || trimmed.starts_with("信道")
        {
            current_channel.number = value_after_colon(trimmed)
                .and_then(parse_channel)
                .unwrap_or(0);
        } else if is_netsh_band_field(trimmed) {
            current_channel.band = value_after_colon(trimmed).and_then(parse_channel_band);
        }
    }

    push_netsh_network(
        &mut networks,
        &current_ssid,
        &current_security,
        &current_bssid,
        current_quality,
        current_channel,
    );

    networks
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn parse_netsh_interfaces(raw: &str) -> Vec<WifiNetwork> {
    let mut networks = Vec::new();
    let mut current_ssid = String::new();
    let mut current_security = String::from("Unknown");
    let mut current_bssid = String::new();
    let mut current_quality: Option<u8> = None;
    let mut current_channel = NetworkChannel::default();
    let mut is_connected = false;

    for line in raw.lines() {
        let trimmed = line.trim();

        if (trimmed.starts_with("Name") || trimmed.starts_with("名称")) && trimmed.contains(':') {
            push_netsh_interface_network(
                &mut networks,
                &current_ssid,
                &current_security,
                &current_bssid,
                current_quality,
                current_channel,
                is_connected,
            );
            current_ssid.clear();
            current_security = String::from("Unknown");
            current_bssid.clear();
            current_quality = None;
            current_channel = NetworkChannel::default();
            is_connected = false;
        } else if trimmed.starts_with("State") || trimmed.starts_with("状态") {
            is_connected = value_after_colon(trimmed)
                .map(is_connected_netsh_state)
                .unwrap_or(false);
        } else if trimmed.starts_with("SSID") && !trimmed.starts_with("BSSID") {
            current_ssid = value_after_colon(trimmed).unwrap_or_default().to_string();
        } else if trimmed.starts_with("Authentication") || trimmed.starts_with("身份验证") {
            current_security = value_after_colon(trimmed).unwrap_or("Unknown").to_string();
        } else if trimmed.starts_with("BSSID") {
            current_bssid = value_after_colon(trimmed)
                .unwrap_or_default()
                .to_lowercase();
        } else if trimmed.starts_with("Signal") || trimmed.starts_with("信号") {
            current_quality = value_after_colon(trimmed)
                .and_then(|value| value.trim_end_matches('%').trim().parse::<u8>().ok());
        } else if trimmed.starts_with("Channel")
            || trimmed.starts_with("频道")
            || trimmed.starts_with("信道")
        {
            current_channel.number = value_after_colon(trimmed)
                .and_then(parse_channel)
                .unwrap_or(0);
        } else if is_netsh_band_field(trimmed) {
            current_channel.band = value_after_colon(trimmed).and_then(parse_channel_band);
        }
    }

    push_netsh_interface_network(
        &mut networks,
        &current_ssid,
        &current_security,
        &current_bssid,
        current_quality,
        current_channel,
        is_connected,
    );

    networks
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn push_netsh_interface_network(
    networks: &mut Vec<WifiNetwork>,
    ssid: &str,
    security: &str,
    bssid: &str,
    quality: Option<u8>,
    channel: NetworkChannel,
    is_connected: bool,
) {
    if !is_connected {
        return;
    }

    let before = networks.len();
    push_netsh_network(networks, ssid, security, bssid, quality, channel);
    if networks.len() > before {
        if let Some(network) = networks.last_mut() {
            network.is_connected = true;
        }
    }
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn parse_netsh_ssid_only(raw: &str) -> Vec<WifiNetwork> {
    let mut networks = Vec::new();
    let mut current_ssid = String::new();
    let mut current_security = String::from("Unknown");
    let mut current_has_bssid = false;
    let mut salt = 0usize;

    for line in raw.lines() {
        let trimmed = line.trim();

        if trimmed.starts_with("SSID ") && trimmed.contains(':') {
            push_netsh_ssid_only_network(
                &mut networks,
                &current_ssid,
                &current_security,
                current_has_bssid,
                salt,
            );
            salt += 1;
            current_ssid = value_after_colon(trimmed).unwrap_or_default().to_string();
            current_security = String::from("Unknown");
            current_has_bssid = false;
        } else if trimmed.starts_with("BSSID ") && trimmed.contains(':') {
            current_has_bssid = true;
        } else if trimmed.starts_with("Authentication") || trimmed.starts_with("身份验证") {
            current_security = value_after_colon(trimmed).unwrap_or("Unknown").to_string();
        }
    }

    push_netsh_ssid_only_network(
        &mut networks,
        &current_ssid,
        &current_security,
        current_has_bssid,
        salt,
    );

    networks
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn push_netsh_ssid_only_network(
    networks: &mut Vec<WifiNetwork>,
    ssid: &str,
    security: &str,
    has_bssid: bool,
    salt: usize,
) {
    if ssid.is_empty() || has_bssid {
        return;
    }

    networks.push(make_network(
        ssid.to_string(),
        synthetic_bssid(ssid, 0, salt),
        -100,
        0,
        security.to_string(),
        Some(0),
    ));
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn is_connected_netsh_state(value: &str) -> bool {
    let normalized = value.trim().to_ascii_lowercase();
    normalized == "connected" || value.trim() == "已连接"
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn push_netsh_network(
    networks: &mut Vec<WifiNetwork>,
    ssid: &str,
    security: &str,
    bssid: &str,
    quality: Option<u8>,
    channel: NetworkChannel,
) {
    if ssid.is_empty() || !is_mac_address(bssid) {
        return;
    }

    let quality = quality.unwrap_or(0);
    let signal_dbm = quality_to_dbm(quality);

    networks.push(
        make_network(
            ssid.to_string(),
            bssid.to_string(),
            signal_dbm,
            channel.number,
            security.to_string(),
            Some(quality),
        )
        .with_band(channel.band),
    );
}

fn build_channel_distribution(networks: &[WifiNetwork]) -> Vec<ChannelDistribution> {
    let mut groups: BTreeMap<(String, u16), usize> = BTreeMap::new();

    for network in networks {
        if network.channel == 0 {
            continue;
        }
        *groups
            .entry((network.band.clone(), network.channel))
            .or_default() += 1;
    }

    groups
        .into_iter()
        .map(|((band, channel), network_count)| ChannelDistribution {
            band,
            channel,
            network_count,
        })
        .collect()
}

fn make_network(
    ssid: String,
    bssid: String,
    signal_dbm: i32,
    channel: u16,
    security: String,
    quality: Option<u8>,
) -> WifiNetwork {
    let frequency_mhz = channel_to_frequency(channel);
    let band = band_from_frequency(frequency_mhz);
    let security = if security.is_empty() {
        "Unknown".to_string()
    } else {
        security
    };

    WifiNetwork {
        ssid: if ssid.is_empty() {
            "<hidden>".to_string()
        } else {
            ssid
        },
        bssid,
        signal_dbm,
        quality: quality.unwrap_or_else(|| dbm_to_quality(signal_dbm)),
        channel,
        frequency_mhz,
        band,
        is_open: is_open_security(&security),
        is_enterprise: is_enterprise_security(&security),
        security,
        is_connected: false,
    }
}

fn mark_current_connection(networks: &mut [WifiNetwork]) {
    if let Some(first_connected) = networks.iter().position(|network| network.is_connected) {
        for (index, network) in networks.iter_mut().enumerate() {
            network.is_connected = index == first_connected;
        }
        return;
    }

    let current = current_connection();
    mark_current_connection_from(networks, current.as_ref());
}

fn mark_current_connection_from(networks: &mut [WifiNetwork], current: Option<&CurrentConnection>) {
    for network in networks.iter_mut() {
        network.is_connected = false;
    }

    let Some(current) = current else {
        return;
    };

    if let Some(current_bssid) = current.bssid.as_deref() {
        if let Some(network) = networks
            .iter_mut()
            .find(|network| network.bssid.eq_ignore_ascii_case(current_bssid))
        {
            network.is_connected = true;
        }
        // A known BSSID is authoritative even when its AP was missed by this scan.
        return;
    }

    let ssid_matches: Vec<usize> = networks
        .iter()
        .enumerate()
        .filter_map(|(index, network)| (network.ssid == current.ssid).then_some(index))
        .collect();
    let channel_matches: Vec<usize> = current
        .channel
        .map(|channel| {
            ssid_matches
                .iter()
                .copied()
                .filter(|index| networks[*index].channel == channel)
                .collect()
        })
        .unwrap_or_default();
    let candidates = if channel_matches.is_empty() {
        &ssid_matches
    } else {
        &channel_matches
    };

    let selected = current
        .signal_dbm
        .and_then(|signal_dbm| {
            candidates
                .iter()
                .copied()
                .min_by_key(|index| networks[*index].signal_dbm.abs_diff(signal_dbm))
        })
        .or_else(|| (candidates.len() == 1).then(|| candidates[0]));

    if let Some(index) = selected {
        networks[index].is_connected = true;
    }
}

pub fn current_connection_identity() -> Option<ConnectionIdentity> {
    current_connection().map(|connection| ConnectionIdentity {
        ssid: connection.ssid,
        bssid: connection.bssid,
    })
}

#[cfg(target_os = "macos")]
fn current_connection() -> Option<CurrentConnection> {
    unsafe {
        let client = CWWiFiClient::sharedWiFiClient();
        if let Some(interface) = client.interface() {
            if let Some(ssid) = interface.ssid() {
                return Some(CurrentConnection {
                    ssid: ssid.to_string(),
                    bssid: interface
                        .bssid()
                        .map(|value| value.to_string().to_ascii_lowercase())
                        .filter(|value| is_mac_address(value)),
                    channel: interface
                        .wlanChannel()
                        .map(|value| value.channelNumber().clamp(0, u16::MAX as isize) as u16),
                    signal_dbm: Some(
                        interface
                            .rssiValue()
                            .clamp(i32::MIN as isize, i32::MAX as isize)
                            as i32,
                    ),
                });
            }
        }
    }

    let device = macos_wifi_device()?;
    let raw = run_command("networksetup", &["-getairportnetwork", &device]).ok()?;
    parse_networksetup_current_ssid(&raw).map(|ssid| CurrentConnection {
        ssid,
        bssid: None,
        channel: None,
        signal_dbm: None,
    })
}

#[cfg(target_os = "windows")]
fn current_connection() -> Option<CurrentConnection> {
    let raw = run_command("netsh", &["wlan", "show", "interfaces"]).ok()?;
    parse_netsh_current_connection(&raw)
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn current_connection() -> Option<CurrentConnection> {
    None
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn parse_netsh_current_connection(raw: &str) -> Option<CurrentConnection> {
    let mut connected = false;
    let mut current = CurrentConnection::default();

    for line in raw.lines() {
        let trimmed = line.trim();

        if (trimmed.starts_with("Name") || trimmed.starts_with("名称")) && trimmed.contains(':') {
            if connected && !current.ssid.is_empty() {
                return Some(current);
            }
            connected = false;
            current = CurrentConnection::default();
        } else if trimmed.starts_with("State") || trimmed.starts_with("状态") {
            connected = value_after_colon(trimmed)
                .map(is_connected_netsh_state)
                .unwrap_or(false);
        } else if trimmed.starts_with("SSID") && !trimmed.starts_with("BSSID") {
            current.ssid = value_after_colon(trimmed).unwrap_or_default().to_string();
        } else if trimmed.starts_with("BSSID") {
            current.bssid = value_after_colon(trimmed)
                .filter(|value| is_mac_address(value))
                .map(|value| value.to_ascii_lowercase());
        } else if trimmed.starts_with("Channel")
            || trimmed.starts_with("信道")
            || trimmed.starts_with("频道")
        {
            current.channel = value_after_colon(trimmed).and_then(parse_channel);
        } else if trimmed.starts_with("Signal") || trimmed.starts_with("信号") {
            current.signal_dbm = value_after_colon(trimmed)
                .and_then(|value| value.trim_end_matches('%').trim().parse::<u8>().ok())
                .map(quality_to_dbm);
        }
    }

    (connected && !current.ssid.is_empty()).then_some(current)
}

#[cfg(target_os = "macos")]
fn macos_wifi_device() -> Option<String> {
    let raw = run_command("networksetup", &["-listallhardwareports"]).ok()?;
    parse_macos_wifi_device(&raw)
}

#[cfg(target_os = "macos")]
fn parse_macos_wifi_device(raw: &str) -> Option<String> {
    let mut in_wifi_port = false;

    for line in raw.lines() {
        let trimmed = line.trim();

        if let Some(port) = trimmed.strip_prefix("Hardware Port:") {
            let port = port.trim();
            in_wifi_port = port == "Wi-Fi" || port == "AirPort";
            continue;
        }

        if in_wifi_port {
            if let Some(device) = trimmed.strip_prefix("Device:") {
                let device = device.trim();
                if !device.is_empty() {
                    return Some(device.to_string());
                }
            }
        }
    }

    None
}

#[cfg(target_os = "macos")]
fn parse_networksetup_current_ssid(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.contains("not associated") {
        return None;
    }

    trimmed
        .split_once(": ")
        .map(|(_, ssid)| ssid.trim().to_string())
        .filter(|ssid| !ssid.is_empty())
}

fn value_after_colon(line: &str) -> Option<&str> {
    line.split_once(':').map(|(_, value)| value.trim())
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn has_displayable_ssid(networks: &[WifiNetwork]) -> bool {
    networks
        .iter()
        .any(|network| !is_non_displayable_ssid(&network.ssid))
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn is_non_displayable_ssid(ssid: &str) -> bool {
    let normalized = ssid.trim().to_ascii_lowercase();
    normalized.is_empty() || normalized == "<hidden>" || normalized == "<redacted>"
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn parse_signal_dbm(value: &str) -> Option<i32> {
    value
        .split_whitespace()
        .find_map(|part| part.parse::<i32>().ok())
}

fn parse_channel(value: &str) -> Option<u16> {
    let digits: String = value.chars().take_while(|ch| ch.is_ascii_digit()).collect();
    digits.parse::<u16>().ok()
}

fn parse_channel_band(value: &str) -> Option<WifiBand> {
    value.split(['(', ')', ',']).find_map(|part| {
        let normalized: String = part.chars().filter(|ch| !ch.is_whitespace()).collect();
        match normalized.to_ascii_lowercase().as_str() {
            "2ghz" | "2.4ghz" => Some(WifiBand::Ghz2),
            "5ghz" => Some(WifiBand::Ghz5),
            "6ghz" => Some(WifiBand::Ghz6),
            _ => None,
        }
    })
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn is_netsh_band_field(line: &str) -> bool {
    ["Band", "频段", "频带", "波段"]
        .iter()
        .any(|prefix| line.starts_with(prefix))
}

fn is_mac_address(value: &str) -> bool {
    let clean = value.trim();
    let parts: Vec<&str> = clean.split(':').collect();
    parts.len() == 6
        && parts
            .iter()
            .all(|part| part.len() == 2 && part.chars().all(|ch| ch.is_ascii_hexdigit()))
}

fn is_open_security(security: &str) -> bool {
    let normalized = security.to_ascii_lowercase();
    normalized == "--"
        || normalized.contains("open")
        || normalized.contains("none")
        || normalized.contains("无")
}

fn is_enterprise_security(security: &str) -> bool {
    let normalized = security.to_ascii_lowercase();
    normalized.contains("enterprise") || normalized.contains("802.1x")
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn synthetic_bssid(ssid: &str, channel: u16, salt: usize) -> String {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in ssid
        .bytes()
        .chain(channel.to_be_bytes())
        .chain((salt as u64).to_be_bytes())
    {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }

    format!(
        "02:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        (hash >> 32) & 0xff,
        (hash >> 24) & 0xff,
        (hash >> 16) & 0xff,
        (hash >> 8) & 0xff,
        hash & 0xff
    )
}

fn channel_to_frequency(channel: u16) -> u16 {
    match channel {
        // 1..=13 与 6GHz 信道号重叠，缺少频段上下文时优先按 2.4GHz 解释
        1..=13 => 2407 + channel * 5,
        14 => 2484,
        32..=177 => 5000 + channel * 5,
        15..=31 | 178..=233 => 5950 + channel * 5,
        _ => 0,
    }
}

fn band_from_frequency(frequency_mhz: u16) -> String {
    match frequency_mhz {
        2400..=2500 => "2.4GHz".to_string(),
        4900..=5925 => "5GHz".to_string(),
        5926..=7125 => "6GHz".to_string(),
        _ => "Unknown".to_string(),
    }
}

fn dbm_to_quality(dbm: i32) -> u8 {
    (((dbm + 100) * 2).clamp(0, 100)) as u8
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn quality_to_dbm(quality: u8) -> i32 {
    (i32::from(quality) / 2) - 100
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_airport_rows_with_spaces_in_ssid() {
        let raw = "                            SSID BSSID             RSSI CHANNEL HT CC SECURITY (auth/unicast/group)\n\
                   Office Main aa:bb:cc:dd:ee:ff -48  149     Y  US WPA2(PSK/AES/AES)\n\
                         IoT Net 11:22:33:44:55:66 -79  6       Y  US WPA(PSK/TKIP/TKIP)\n";

        let rows = parse_airport(raw);

        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].ssid, "Office Main");
        assert_eq!(rows[0].band, "5GHz");
        assert_eq!(rows[1].channel, 6);
    }

    #[test]
    fn treats_redacted_airport_rows_as_not_displayable() {
        let raw = "                            SSID BSSID             RSSI CHANNEL HT CC SECURITY (auth/unicast/group)\n\
                       <redacted> aa:bb:cc:dd:ee:ff -48  149     Y  US WPA2(PSK/AES/AES)\n\
                         <hidden> 11:22:33:44:55:66 -79  6       Y  US WPA(PSK/TKIP/TKIP)\n";

        let rows = parse_airport(raw);

        assert_eq!(rows.len(), 2);
        assert!(!has_displayable_ssid(&rows));
    }

    #[test]
    fn parses_current_connection_from_netsh_interfaces() {
        let raw = r#"Name                   : Wi-Fi
State                  : connected
SSID                   : Studio-5G
BSSID                  : aa:bb:cc:dd:ee:ff
Channel                : 149
Signal                 : 86%
"#;

        let current = parse_netsh_current_connection(raw).expect("connected WiFi");
        assert_eq!(current.ssid, "Studio-5G");
        assert_eq!(current.bssid.as_deref(), Some("aa:bb:cc:dd:ee:ff"));
        assert_eq!(current.channel, Some(149));
        assert_eq!(current.signal_dbm, Some(-57));
    }

    #[test]
    fn preserves_connected_interface_when_another_adapter_is_disconnected() {
        let connected = "Name : Wi-Fi\nState : connected\nSSID : Studio\nBSSID : aa:bb:cc:dd:ee:ff\nSignal : 86%\nChannel : 149\n";
        let disconnected = "Name : Wi-Fi 2\nState : disconnected\n";
        for raw in [
            format!("{connected}\n{disconnected}"),
            format!("{disconnected}\n{connected}"),
        ] {
            let current = parse_netsh_current_connection(&raw).expect("the active adapter");
            assert_eq!(current.ssid, "Studio");
            assert_eq!(current.bssid.as_deref(), Some("aa:bb:cc:dd:ee:ff"));
        }
    }

    #[test]
    fn keeps_explicit_6ghz_band_for_overlapping_channel_numbers() {
        for (channel, frequency) in [(5, 5975), (37, 6135), (149, 6695)] {
            let raw = format!("SSID 1 : Lab-6E\nAuthentication : WPA3-Personal\nBSSID 1 : aa:bb:cc:dd:ee:ff\nSignal : 86%\nBand : 6 GHz\nChannel : {channel}\n");
            let rows = parse_windows_netsh(&raw);
            assert_eq!(rows[0].band, "6GHz");
            assert_eq!(rows[0].frequency_mhz, frequency);
            assert_eq!(build_channel_distribution(&rows)[0].band, "6GHz");
        }
    }

    #[test]
    fn parses_6ghz_band_in_connected_interface_fallback() {
        let raw = "Name : Wi-Fi\nState : connected\nSSID : Lab-6E\nBSSID : aa:bb:cc:dd:ee:ff\nSignal : 86%\nBand : 6 GHz\nChannel : 37\n";
        let rows = parse_windows_netsh(raw);
        assert_eq!(rows[0].band, "6GHz");
        assert_eq!(rows[0].frequency_mhz, 6135);
        assert!(rows[0].is_connected);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn preserves_system_profiler_band_context() {
        let raw = "Current Network Information:\n  Lab-6E:\n    Channel: 37 (6GHz, 80MHz)\n    Security: WPA3 Personal\n    Signal / Noise: -48 dBm / -95 dBm\n";
        let rows = parse_system_profiler_airport(raw);
        assert_eq!(rows[0].band, "6GHz");
        assert_eq!(rows[0].frequency_mhz, 6135);
    }

    #[test]
    fn does_not_substitute_a_different_bssid_when_the_known_ap_is_missing() {
        let mut networks = vec![make_network(
            "Studio".to_string(),
            "00:00:00:00:00:02".to_string(),
            -43,
            149,
            "WPA2".to_string(),
            None,
        )];
        let current = CurrentConnection {
            ssid: "Studio".to_string(),
            bssid: Some("00:00:00:00:00:01".to_string()),
            channel: Some(149),
            signal_dbm: Some(-43),
        };
        mark_current_connection_from(&mut networks, Some(&current));
        assert!(!networks[0].is_connected);
    }

    #[test]
    fn classifies_windows_scan_recovery_failures() {
        let permission = classify_windows_scan_error(
            "Access is denied. Location permission is required to access WLAN data.",
        );
        assert_eq!(permission.code, ScanIssueCode::LocationPermissionDenied);
        assert_eq!(
            permission.recovery_action,
            Some(ScanRecoveryAction::OpenLocationSettings)
        );

        let wifi_off = windows_scan_issue_from_text("Radio status : Hardware On Software Off")
            .expect("WiFi disabled issue");
        assert_eq!(wifi_off.code, ScanIssueCode::WifiDisabled);

        let adapter = windows_scan_issue_from_text("There is no wireless interface on the system.")
            .expect("adapter issue");
        assert_eq!(adapter.code, ScanIssueCode::AdapterUnavailable);
    }

    #[test]
    fn parses_windows_bssid_scan_rows() {
        let raw = r#"Interface name : Wi-Fi
There are 1 networks currently visible.

SSID 1 : Studio-5G
    Network type            : Infrastructure
    Authentication          : WPA2-Personal
    Encryption              : CCMP
    BSSID 1                 : aa:bb:cc:dd:ee:ff
         Signal             : 86%
         Radio type         : 802.11ac
         Channel            : 149
"#;

        let rows = parse_windows_netsh(raw);

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].ssid, "Studio-5G");
        assert_eq!(rows[0].bssid, "aa:bb:cc:dd:ee:ff");
        assert_eq!(rows[0].quality, 86);
        assert_eq!(rows[0].channel, 149);
        assert_eq!(rows[0].band, "5GHz");
        assert!(!rows[0].is_connected);
    }

    #[test]
    fn parses_windows_interfaces_as_connected_fallback() {
        let raw = r#"Name                   : Wi-Fi
Description            : Wireless Adapter
GUID                   : 00000000-0000-0000-0000-000000000000
Physical address       : 11:22:33:44:55:66
State                  : connected
SSID                   : Studio-5G
BSSID                  : aa:bb:cc:dd:ee:ff
Network type           : Infrastructure
Radio type             : 802.11ac
Authentication         : WPA2-Personal
Cipher                 : CCMP
Channel                : 149
Signal                 : 86%
"#;

        let rows = parse_windows_netsh(raw);

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].ssid, "Studio-5G");
        assert_eq!(rows[0].bssid, "aa:bb:cc:dd:ee:ff");
        assert_eq!(rows[0].quality, 86);
        assert_eq!(rows[0].channel, 149);
        assert!(rows[0].is_connected);
    }

    #[test]
    fn parses_windows_ssid_only_scan_rows_when_bssid_mode_is_unavailable() {
        let raw = r#"Interface name : Wi-Fi
There are 2 networks currently visible.

SSID 1 : Studio-5G
    Network type            : Infrastructure
    Authentication          : WPA2-Personal
    Encryption              : CCMP

SSID 2 : Cafe Guest
    Network type            : Infrastructure
    Authentication          : Open
    Encryption              : None
"#;

        let rows = parse_windows_netsh(raw);

        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].ssid, "Studio-5G");
        assert_eq!(rows[0].quality, 0);
        assert_eq!(rows[0].channel, 0);
        assert!(is_mac_address(&rows[0].bssid));
        assert_eq!(rows[1].ssid, "Cafe Guest");
        assert!(rows[1].is_open);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn parses_system_profiler_wifi_sections_when_airport_scan_is_empty() {
        let raw = r#"Wi-Fi:

      Interfaces:
        en1:
          Status: Connected
          Current Network Information:
            ZhaoPin-Employee:
              PHY Mode: 802.11ac
              Channel: 52 (5GHz, 20MHz)
              Network Type: Infrastructure
              Security: WPA2 Enterprise
              Signal / Noise: -63 dBm / -101 dBm
          Other Local Wi-Fi Networks:
            ZhaoPin-Guest:
              PHY Mode: 802.11b/g/n
              Channel: 11 (2GHz, 20MHz)
              Network Type: Infrastructure
              Security: None
            ZhaoPin-Mgmt:
              PHY Mode: 802.11a/n/ac
              Channel: 36 (5GHz, 20MHz)
              Network Type: Infrastructure
              Security: WPA2 Enterprise
"#;

        let rows = parse_system_profiler_airport(raw);

        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].ssid, "ZhaoPin-Employee");
        assert_eq!(rows[0].signal_dbm, -63);
        assert_eq!(rows[0].band, "5GHz");
        assert!(rows[0].is_connected);
        assert_eq!(rows[1].ssid, "ZhaoPin-Guest");
        assert_eq!(rows[1].band, "2.4GHz");
        assert!(!rows[1].is_connected);
        assert!(is_mac_address(&rows[1].bssid));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn parses_macos_wifi_device_from_networksetup_hardware_ports() {
        let raw = r#"Hardware Port: Ethernet
Device: en0
Ethernet Address: d0:11:e5:0b:ef:20

Hardware Port: Wi-Fi
Device: en1
Ethernet Address: d0:11:e5:03:28:84
"#;

        assert_eq!(parse_macos_wifi_device(raw).as_deref(), Some("en1"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn parses_current_ssid_from_networksetup_output() {
        assert_eq!(
            parse_networksetup_current_ssid("Current Wi-Fi Network: Studio-5G\n").as_deref(),
            Some("Studio-5G")
        );
        assert_eq!(
            parse_networksetup_current_ssid("You are not associated with an AirPort network.\n"),
            None
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "uses the host WiFi adapter and can be slow"]
    fn live_macos_scan_returns_networks() {
        let result = scan().expect("live macOS scan should not fail");

        assert!(
            !result.networks.is_empty(),
            "expected at least one WiFi network from {}",
            result.source
        );
    }

    #[test]
    fn marks_open_and_enterprise_security_flags() {
        let open = make_network(
            "Cafe".to_string(),
            "00:00:00:00:00:01".to_string(),
            -60,
            6,
            "Open".to_string(),
            None,
        );
        assert!(open.is_open);
        assert!(!open.is_enterprise);

        let enterprise = make_network(
            "Corp".to_string(),
            "00:00:00:00:00:02".to_string(),
            -60,
            36,
            "WPA2 Enterprise".to_string(),
            None,
        );
        assert!(!enterprise.is_open);
        assert!(enterprise.is_enterprise);

        let dot1x = make_network(
            "Corp2".to_string(),
            "00:00:00:00:00:03".to_string(),
            -60,
            36,
            "WPA2 802.1X".to_string(),
            None,
        );
        assert!(dot1x.is_enterprise);

        let psk = make_network(
            "Home".to_string(),
            "00:00:00:00:00:04".to_string(),
            -60,
            149,
            "WPA2(PSK/AES/AES)".to_string(),
            None,
        );
        assert!(!psk.is_open);
        assert!(!psk.is_enterprise);

        let dash = make_network(
            "FreeWifi".to_string(),
            "00:00:00:00:00:05".to_string(),
            -60,
            1,
            "--".to_string(),
            None,
        );
        assert!(dash.is_open);
    }

    #[test]
    fn marks_only_the_exact_connected_bssid() {
        let mut networks = vec![
            make_network(
                "Studio".to_string(),
                "00:00:00:00:00:01".to_string(),
                -43,
                149,
                "WPA2".to_string(),
                None,
            ),
            make_network(
                "Studio".to_string(),
                "00:00:00:00:00:02".to_string(),
                -55,
                149,
                "WPA2".to_string(),
                None,
            ),
        ];
        let current = CurrentConnection {
            ssid: "Studio".to_string(),
            bssid: Some("00:00:00:00:00:02".to_string()),
            channel: Some(149),
            signal_dbm: Some(-55),
        };

        mark_current_connection_from(&mut networks, Some(&current));

        assert!(!networks[0].is_connected);
        assert!(networks[1].is_connected);
    }

    #[test]
    fn uses_channel_and_signal_when_the_current_bssid_is_unavailable() {
        let mut networks = vec![
            make_network(
                "Studio".to_string(),
                "00:00:00:00:00:01".to_string(),
                -43,
                36,
                "WPA2".to_string(),
                None,
            ),
            make_network(
                "Studio".to_string(),
                "00:00:00:00:00:02".to_string(),
                -58,
                149,
                "WPA2".to_string(),
                None,
            ),
        ];
        let current = CurrentConnection {
            ssid: "Studio".to_string(),
            bssid: None,
            channel: Some(149),
            signal_dbm: Some(-57),
        };

        mark_current_connection_from(&mut networks, Some(&current));

        assert!(!networks[0].is_connected);
        assert!(networks[1].is_connected);
    }

    #[test]
    fn counts_scanned_bssids_per_channel_without_inferring_load() {
        let networks = vec![
            make_network(
                "Studio".to_string(),
                "00:00:00:00:00:01".to_string(),
                -43,
                149,
                "WPA2".to_string(),
                None,
            ),
            make_network(
                "Guest".to_string(),
                "00:00:00:00:00:02".to_string(),
                -55,
                149,
                "WPA2".to_string(),
                None,
            ),
        ];
        let distribution = build_channel_distribution(&networks);

        assert_eq!(distribution.len(), 1);
        assert_eq!(distribution[0].band, "5GHz");
        assert_eq!(distribution[0].channel, 149);
        assert_eq!(distribution[0].network_count, 2);
    }
}
