use crate::{command_text, wifi::ConnectionIdentity};
use chrono::Utc;
use serde::Serialize;
use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream, ToSocketAddrs},
    process::{Command, Output},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const PROBE_TIMEOUT: Duration = Duration::from_millis(1_500);
const DNS_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum DiagnosticOverall {
    Healthy,
    Degraded,
    Offline,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum DiagnosticStatus {
    Pass,
    Warning,
    Fail,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum DiagnosticCheckId {
    Wifi,
    Gateway,
    Dns,
    Internet,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticCheck {
    pub id: DiagnosticCheckId,
    pub status: DiagnosticStatus,
    pub title: String,
    pub detail: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub packet_loss_percent: Option<u8>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionDiagnosticReport {
    pub checked_at: String,
    pub overall: DiagnosticOverall,
    pub summary: String,
    pub checks: Vec<DiagnosticCheck>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connection: Option<ConnectionIdentity>,
}

#[derive(Debug, Clone, Copy, Default)]
struct PingMetrics {
    reachable: bool,
    latency_ms: Option<u32>,
    packet_loss_percent: Option<u8>,
}

pub fn run(connection: Option<ConnectionIdentity>) -> ConnectionDiagnosticReport {
    let wifi = wifi_check(connection.as_ref().map(|value| value.ssid.as_str()));
    let gateway_probe = thread::spawn(gateway_check);
    let dns_probe = thread::spawn(dns_check);
    let internet_probe = thread::spawn(internet_check);
    let gateway = gateway_probe
        .join()
        .unwrap_or_else(|_| internal_probe_failure(DiagnosticCheckId::Gateway, "网关检查意外中断"));
    let dns = dns_probe
        .join()
        .unwrap_or_else(|_| internal_probe_failure(DiagnosticCheckId::Dns, "DNS 检查意外中断"));
    let internet = internet_probe.join().unwrap_or_else(|_| {
        internal_probe_failure(DiagnosticCheckId::Internet, "互联网检查意外中断")
    });
    let (overall, summary) = summarize(&wifi, &gateway, &dns, &internet);

    ConnectionDiagnosticReport {
        checked_at: Utc::now().to_rfc3339(),
        overall,
        summary,
        checks: vec![wifi, gateway, dns, internet],
        connection,
    }
}

fn internal_probe_failure(id: DiagnosticCheckId, title: &str) -> DiagnosticCheck {
    DiagnosticCheck {
        id,
        status: DiagnosticStatus::Fail,
        title: title.to_string(),
        detail: "请重新运行诊断；如果问题持续出现，请重启 Poliwave。".to_string(),
        latency_ms: None,
        packet_loss_percent: None,
    }
}

fn wifi_check(ssid: Option<&str>) -> DiagnosticCheck {
    match ssid {
        Some(ssid) => DiagnosticCheck {
            id: DiagnosticCheckId::Wifi,
            status: DiagnosticStatus::Pass,
            title: "WiFi 已连接".to_string(),
            detail: format!("当前连接：{ssid}"),
            latency_ms: None,
            packet_loss_percent: None,
        },
        None => DiagnosticCheck {
            id: DiagnosticCheckId::Wifi,
            status: DiagnosticStatus::Fail,
            title: "未检测到 WiFi 连接".to_string(),
            detail: "请先连接 WiFi；如果设备正在使用有线网络，其他检查仍会继续。".to_string(),
            latency_ms: None,
            packet_loss_percent: None,
        },
    }
}

fn gateway_check() -> DiagnosticCheck {
    let Some(gateway) = default_gateway() else {
        return DiagnosticCheck {
            id: DiagnosticCheckId::Gateway,
            status: DiagnosticStatus::Warning,
            title: "未识别默认网关".to_string(),
            detail: "VPN 或特殊路由配置可能隐藏网关；请结合 DNS 和互联网检查判断。".to_string(),
            latency_ms: None,
            packet_loss_percent: None,
        };
    };

    match ping_target(&gateway) {
        Ok(metrics) if metrics.reachable => {
            let loss = metrics.packet_loss_percent.unwrap_or(0);
            let status = if loss == 0 {
                DiagnosticStatus::Pass
            } else {
                DiagnosticStatus::Warning
            };
            let title = if loss == 0 {
                "本地网关可达"
            } else {
                "本地网关存在丢包"
            };
            DiagnosticCheck {
                id: DiagnosticCheckId::Gateway,
                status,
                title: title.to_string(),
                detail: format_probe_detail(&gateway, metrics),
                latency_ms: metrics.latency_ms,
                packet_loss_percent: metrics.packet_loss_percent,
            }
        }
        Ok(metrics) => DiagnosticCheck {
            id: DiagnosticCheckId::Gateway,
            status: DiagnosticStatus::Warning,
            title: "网关未响应 Ping".to_string(),
            detail: format!(
                "已找到网关 {gateway}，但没有收到 ICMP 响应；部分路由器会禁用 Ping，请结合互联网检查判断。"
            ),
            latency_ms: metrics.latency_ms,
            packet_loss_percent: metrics.packet_loss_percent,
        },
        Err(error) => DiagnosticCheck {
            id: DiagnosticCheckId::Gateway,
            status: DiagnosticStatus::Warning,
            title: "无法检测本地网关".to_string(),
            detail: format!("网关 {gateway} 的 Ping 检查未能执行：{error}"),
            latency_ms: None,
            packet_loss_percent: None,
        },
    }
}

fn dns_check() -> DiagnosticCheck {
    let started = Instant::now();
    match resolve_with_timeout("example.com", DNS_TIMEOUT) {
        Ok(addresses) if !addresses.is_empty() => {
            let latency_ms = elapsed_ms(started);
            DiagnosticCheck {
                id: DiagnosticCheckId::Dns,
                status: DiagnosticStatus::Pass,
                title: "DNS 解析正常".to_string(),
                detail: format!("example.com 解析成功，耗时 {latency_ms} ms。"),
                latency_ms: Some(latency_ms),
                packet_loss_percent: None,
            }
        }
        Ok(_) => DiagnosticCheck {
            id: DiagnosticCheckId::Dns,
            status: DiagnosticStatus::Fail,
            title: "DNS 未返回地址".to_string(),
            detail: "请检查系统 DNS、路由器 DNS 或 VPN 配置。".to_string(),
            latency_ms: None,
            packet_loss_percent: None,
        },
        Err(error) => DiagnosticCheck {
            id: DiagnosticCheckId::Dns,
            status: DiagnosticStatus::Fail,
            title: "DNS 解析失败".to_string(),
            detail: format!("{error} 请检查系统 DNS、路由器 DNS 或 VPN 配置。"),
            latency_ms: None,
            packet_loss_percent: None,
        },
    }
}

fn internet_check() -> DiagnosticCheck {
    if let Ok(metrics) = ping_target("1.1.1.1") {
        if metrics.reachable {
            let loss = metrics.packet_loss_percent.unwrap_or(0);
            return DiagnosticCheck {
                id: DiagnosticCheckId::Internet,
                status: if loss == 0 {
                    DiagnosticStatus::Pass
                } else {
                    DiagnosticStatus::Warning
                },
                title: if loss == 0 {
                    "互联网可达".to_string()
                } else {
                    "互联网可达，但存在丢包".to_string()
                },
                detail: format_probe_detail("公共网络检测端点", metrics),
                latency_ms: metrics.latency_ms,
                packet_loss_percent: metrics.packet_loss_percent,
            };
        }
    }

    match probe_external_tcp() {
        Ok(latency_ms) => DiagnosticCheck {
            id: DiagnosticCheckId::Internet,
            status: DiagnosticStatus::Pass,
            title: "互联网可达".to_string(),
            detail: "外部 TCP 连接成功；检测端点未响应 Ping，因此本次无法计算互联网丢包率。"
                .to_string(),
            latency_ms: Some(latency_ms),
            packet_loss_percent: None,
        },
        Err(error) => DiagnosticCheck {
            id: DiagnosticCheckId::Internet,
            status: DiagnosticStatus::Fail,
            title: "互联网连接异常".to_string(),
            detail: format!("{error} 请检查路由器上网状态、防火墙、代理或 VPN。"),
            latency_ms: None,
            packet_loss_percent: None,
        },
    }
}

fn summarize(
    wifi: &DiagnosticCheck,
    gateway: &DiagnosticCheck,
    dns: &DiagnosticCheck,
    internet: &DiagnosticCheck,
) -> (DiagnosticOverall, String) {
    let wifi_ok = wifi.status == DiagnosticStatus::Pass;
    let gateway_ok = gateway.status == DiagnosticStatus::Pass;
    let dns_ok = dns.status == DiagnosticStatus::Pass;
    let internet_ok = internet.status == DiagnosticStatus::Pass;
    let internet_reachable = internet.status != DiagnosticStatus::Fail;

    if wifi_ok && dns_ok && internet_ok {
        return (
            DiagnosticOverall::Healthy,
            "WiFi、DNS 与互联网连接均正常。".to_string(),
        );
    }
    if internet_reachable && dns_ok && !wifi_ok {
        return (
            DiagnosticOverall::Degraded,
            "设备可以联网，但未确认当前 WiFi 连接，可能正在使用有线网络。".to_string(),
        );
    }
    if internet_reachable && !dns_ok {
        return (
            DiagnosticOverall::Degraded,
            "外部网络可达，但 DNS 解析异常。".to_string(),
        );
    }
    if internet_reachable {
        return (
            DiagnosticOverall::Degraded,
            "互联网可达，但检测到丢包，连接质量下降。".to_string(),
        );
    }
    if gateway_ok {
        return (
            DiagnosticOverall::Degraded,
            "本地网络可达，但互联网连接异常。".to_string(),
        );
    }

    (
        DiagnosticOverall::Offline,
        "未确认可用的网络连接，请从 WiFi、网关到互联网逐项排查。".to_string(),
    )
}

fn format_probe_detail(target: &str, metrics: PingMetrics) -> String {
    match (metrics.latency_ms, metrics.packet_loss_percent) {
        (Some(latency), Some(loss)) => {
            format!("{target}：平均延迟 {latency} ms，丢包 {loss}%。")
        }
        (Some(latency), None) => format!("{target}：平均延迟 {latency} ms。"),
        (None, Some(loss)) => format!("{target}：丢包 {loss}%。"),
        (None, None) => format!("{target} 可达。"),
    }
}

fn default_gateway() -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        let output = command_output("/sbin/route", &["-n", "get", "default"]).ok()?;
        if !output.status.success() {
            return None;
        }
        parse_macos_default_gateway(&command_text::decode(&output.stdout))
    }

    #[cfg(target_os = "windows")]
    {
        let script = "Get-NetRoute -AddressFamily IPv4 -DestinationPrefix '0.0.0.0/0' | Sort-Object RouteMetric | Select-Object -First 1 -ExpandProperty NextHop";
        let output = command_output(
            "powershell.exe",
            &["-NoProfile", "-NonInteractive", "-Command", script],
        )
        .ok()?;
        if !output.status.success() {
            return None;
        }
        parse_windows_default_gateway(&command_text::decode(&output.stdout))
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    None
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn parse_macos_default_gateway(raw: &str) -> Option<String> {
    raw.lines().find_map(|line| {
        line.trim()
            .strip_prefix("gateway:")
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    })
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn parse_windows_default_gateway(raw: &str) -> Option<String> {
    raw.lines()
        .map(str::trim)
        .find(|line| line.parse::<IpAddr>().is_ok())
        .map(str::to_string)
}

fn ping_target(target: &str) -> Result<PingMetrics, String> {
    #[cfg(target_os = "macos")]
    let output = command_output("/sbin/ping", &["-n", "-c", "3", "-W", "1000", target]);

    #[cfg(target_os = "windows")]
    let output = command_output("ping.exe", &["-n", "3", "-w", "1000", target]);

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let output: std::io::Result<Output> = Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "当前平台不支持 Ping 检查",
    ));

    let output = output.map_err(|error| error.to_string())?;
    let raw = format!(
        "{}\n{}",
        command_text::decode(&output.stdout),
        command_text::decode(&output.stderr)
    );
    Ok(parse_ping_metrics(&raw, output.status.success()))
}

fn parse_ping_metrics(raw: &str, command_succeeded: bool) -> PingMetrics {
    let packet_loss_percent = parse_packet_loss(raw);

    PingMetrics {
        reachable: packet_loss_percent.map_or(command_succeeded, |loss| loss < 100),
        latency_ms: parse_average_latency(raw),
        packet_loss_percent,
    }
}

fn parse_packet_loss(raw: &str) -> Option<u8> {
    for (percent_index, _) in raw.match_indices('%') {
        let before = &raw[..percent_index];
        let digits_reversed: String = before
            .chars()
            .rev()
            .skip_while(|ch| ch.is_whitespace())
            .take_while(|ch| ch.is_ascii_digit() || *ch == '.' || *ch == ',')
            .collect();
        if digits_reversed.is_empty() {
            continue;
        }
        let digits: String = digits_reversed.chars().rev().collect();
        if let Ok(value) = digits.replace(',', ".").parse::<f64>() {
            if (0.0..=100.0).contains(&value) {
                return Some(value.round() as u8);
            }
        }
    }
    None
}

fn parse_average_latency(raw: &str) -> Option<u32> {
    for line in raw.lines() {
        let normalized = line.to_ascii_lowercase();
        if normalized.contains("min/avg/max") {
            let values = line.split_once('=')?.1.trim();
            let average = values.split('/').nth(1)?.trim().parse::<f64>().ok()?;
            return Some(average.round().clamp(0.0, u32::MAX as f64) as u32);
        }
        if normalized.contains("average") || line.contains("平均") {
            let value = line.rsplit_once('=')?.1;
            let numeric: String = value
                .chars()
                .skip_while(|ch| ch.is_whitespace())
                .take_while(|ch| ch.is_ascii_digit() || *ch == '.')
                .collect();
            if let Ok(average) = numeric.parse::<f64>() {
                return Some(average.round().clamp(0.0, u32::MAX as f64) as u32);
            }
        }
    }
    None
}

fn resolve_with_timeout(host: &str, timeout: Duration) -> Result<Vec<SocketAddr>, String> {
    let host = host.to_string();
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let result = (host.as_str(), 443)
            .to_socket_addrs()
            .map(|addresses| addresses.collect::<Vec<_>>())
            .map_err(|error| error.to_string());
        let _ = sender.send(result);
    });

    receiver
        .recv_timeout(timeout)
        .map_err(|_| "DNS 解析超时。".to_string())?
}

fn probe_external_tcp() -> Result<u32, String> {
    let endpoints = [
        SocketAddr::new(IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1)), 443),
        SocketAddr::new(IpAddr::V4(Ipv4Addr::new(223, 5, 5, 5)), 53),
    ];
    let mut last_error = None;

    for endpoint in endpoints {
        let started = Instant::now();
        match TcpStream::connect_timeout(&endpoint, PROBE_TIMEOUT) {
            Ok(stream) => {
                drop(stream);
                return Ok(elapsed_ms(started));
            }
            Err(error) => last_error = Some(error),
        }
    }

    Err(last_error
        .map(|error| format!("无法连接外部检测端点：{error}。"))
        .unwrap_or_else(|| "无法连接外部检测端点。".to_string()))
}

fn elapsed_ms(started: Instant) -> u32 {
    started.elapsed().as_millis().min(u128::from(u32::MAX)) as u32
}

fn command_output(program: &str, args: &[&str]) -> std::io::Result<Output> {
    let mut command = Command::new(program);
    command.args(args);
    #[cfg(target_os = "windows")]
    command.creation_flags(CREATE_NO_WINDOW);
    command.output()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(id: DiagnosticCheckId, status: DiagnosticStatus) -> DiagnosticCheck {
        DiagnosticCheck {
            id,
            status,
            title: String::new(),
            detail: String::new(),
            latency_ms: None,
            packet_loss_percent: None,
        }
    }

    #[test]
    fn parses_default_gateways() {
        let macos = "   route to: default\ndestination: default\n    gateway: 192.168.1.1\n";
        assert_eq!(
            parse_macos_default_gateway(macos).as_deref(),
            Some("192.168.1.1")
        );

        let windows = "\r\n192.168.50.1\r\n";
        assert_eq!(
            parse_windows_default_gateway(windows).as_deref(),
            Some("192.168.50.1")
        );
    }

    #[test]
    fn parses_macos_ping_metrics() {
        let raw = "3 packets transmitted, 3 packets received, 0.0% packet loss\nround-trip min/avg/max/stddev = 1.123/2.456/4.100/0.800 ms\n";
        assert_eq!(parse_packet_loss(raw), Some(0));
        assert_eq!(parse_average_latency(raw), Some(2));
    }

    #[test]
    fn preserves_decimal_packet_loss_including_total_loss() {
        for (raw, expected) in [
            (
                "3 packets transmitted, 0 packets received, 100.0% packet loss",
                100,
            ),
            (
                "3 packets transmitted, 2 packets received, 33.3% packet loss",
                33,
            ),
            (
                "3 packets transmitted, 1 packets received, 66.7% packet loss",
                67,
            ),
            (
                "3 packets transmitted, 3 packets received, 0.0% packet loss",
                0,
            ),
        ] {
            assert_eq!(parse_packet_loss(raw), Some(expected), "{raw}");
        }
    }

    #[test]
    fn total_loss_is_unreachable_and_partial_replies_remain_reachable() {
        for exit_success in [false, true] {
            assert!(!parse_ping_metrics("100.0% packet loss", exit_success).reachable);
        }
        let partial = parse_ping_metrics("33.3% packet loss", false);
        assert!(partial.reachable);
        assert_eq!(partial.packet_loss_percent, Some(33));
    }

    #[test]
    fn reachable_internet_with_loss_is_degraded_even_without_gateway_ping() {
        let wifi = check(DiagnosticCheckId::Wifi, DiagnosticStatus::Pass);
        let dns = check(DiagnosticCheckId::Dns, DiagnosticStatus::Pass);
        let internet = check(DiagnosticCheckId::Internet, DiagnosticStatus::Warning);
        for status in [
            DiagnosticStatus::Pass,
            DiagnosticStatus::Warning,
            DiagnosticStatus::Fail,
        ] {
            let gateway = check(DiagnosticCheckId::Gateway, status);
            let (overall, summary) = summarize(&wifi, &gateway, &dns, &internet);
            assert_eq!(overall, DiagnosticOverall::Degraded);
            assert!(summary.contains("丢包"));
        }
    }

    #[test]
    fn parses_localized_windows_ping_metrics() {
        let raw = "数据包: 已发送 = 3，已接收 = 2，丢失 = 1 (33% 丢失)，\n往返行程的估计时间(以毫秒为单位):\n    最短 = 2ms，最长 = 8ms，平均 = 5ms\n";
        assert_eq!(parse_packet_loss(raw), Some(33));
        assert_eq!(parse_average_latency(raw), Some(5));
    }

    #[test]
    fn summarizes_healthy_and_dns_failure_paths() {
        let wifi = check(DiagnosticCheckId::Wifi, DiagnosticStatus::Pass);
        let gateway = check(DiagnosticCheckId::Gateway, DiagnosticStatus::Warning);
        let dns = check(DiagnosticCheckId::Dns, DiagnosticStatus::Pass);
        let internet = check(DiagnosticCheckId::Internet, DiagnosticStatus::Pass);

        assert_eq!(
            summarize(&wifi, &gateway, &dns, &internet).0,
            DiagnosticOverall::Healthy
        );

        let dns_failed = check(DiagnosticCheckId::Dns, DiagnosticStatus::Fail);
        assert_eq!(
            summarize(&wifi, &gateway, &dns_failed, &internet).0,
            DiagnosticOverall::Degraded
        );
    }
}
