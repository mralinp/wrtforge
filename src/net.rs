//! Laptop-side networking: which route reaches the router, and finding OpenWrt routers on the LAN.
use crate::ui;
use anyhow::{bail, Result};
use console::style;
use std::io::Read;
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::process::Command;
use std::sync::Mutex;
use std::time::Duration;

pub type Route = (Option<String>, Option<String>); // (interface, next hop)

pub fn route_info(dest: &str) -> Route {
    let run = |cmd: &str, args: &[&str]| {
        Command::new(cmd)
            .args(args)
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
    };
    if cfg!(target_os = "macos") {
        let out = run("route", &["-n", "get", dest]).unwrap_or_default();
        let find = |k: &str| {
            out.lines()
                .find_map(|l| l.trim().strip_prefix(k).map(|v| v.trim().to_string()))
        };
        (find("interface:"), find("gateway:"))
    } else {
        let out = run("ip", &["route", "get", dest]).unwrap_or_default();
        let t: Vec<&str> = out.split_whitespace().collect();
        let after = |k: &str| {
            t.iter()
                .position(|x| *x == k)
                .and_then(|i| t.get(i + 1))
                .map(|s| s.to_string())
        };
        (after("dev"), after("via"))
    }
}

#[derive(Debug, PartialEq)]
pub enum Verdict {
    /// cable to the router, internet over another interface
    Split,
    /// the laptop's internet rides the router that is about to be flashed
    Through,
    /// internet does not involve this router
    Other,
    Unknown,
}

pub fn verdict(host: &str, to_router: &Route, to_internet: &Route) -> Verdict {
    match (&to_router.0, &to_internet.0) {
        (Some(_), Some(_)) if to_internet.1.as_deref() == Some(host) => Verdict::Through,
        (Some(r), Some(i)) if r != i => Verdict::Split,
        (Some(_), Some(_)) => Verdict::Other,
        _ => Verdict::Unknown,
    }
}

/// Returns Ok(false) when the user chose not to continue.
pub fn network_check(host: &str, dry_run: bool) -> Result<bool> {
    let (to_router, to_net) = (route_info(host), route_info("1.1.1.1"));
    match verdict(host, &to_router, &to_net) {
        Verdict::Split => ui::ok(&format!(
            "two networks: router via {}, internet via {}. You will stay online.",
            to_router.0.unwrap_or_default(),
            to_net.0.unwrap_or_default()
        )),
        Verdict::Other => ui::ok(&format!(
            "your internet does not go through this router ({})",
            to_net.0.unwrap_or_default()
        )),
        Verdict::Through => {
            ui::warn(
                "your internet currently goes THROUGH this router and will drop while it flashes",
            );
            ui::info("tip: plug a LAN cable from your laptop into the router for this session and keep Wi-Fi");
            ui::info("(or any other connection) as your main internet, then nothing gets cut off.");
            if !dry_run && !ui::confirm("Continue on this connection anyway?", false)? {
                return Ok(false);
            }
        }
        Verdict::Unknown => {}
    }
    Ok(true)
}

/// Host addresses of the /24-or-smaller networks found in `ifconfig` (macOS) or `ip -4 -o addr` (Linux) output.
pub fn parse_subnets(text: &str) -> Vec<(Ipv4Addr, u8)> {
    let mut nets: Vec<(Ipv4Addr, u8)> = vec![];
    for line in text.lines() {
        let t: Vec<&str> = line.split_whitespace().collect();
        let Some(i) = t.iter().position(|x| *x == "inet") else {
            continue;
        };
        let Some(addr) = t.get(i + 1) else { continue };
        let (ip_s, plen) = match addr.split_once('/') {
            Some((a, p)) => (a, p.parse::<u32>().ok()),
            None => {
                let m = t
                    .iter()
                    .position(|x| *x == "netmask")
                    .and_then(|j| t.get(j + 1));
                (
                    *addr,
                    m.and_then(|m| u32::from_str_radix(m.trim_start_matches("0x"), 16).ok())
                        .map(|m| m.count_ones()),
                )
            }
        };
        if let (Ok(ip), Some(plen)) = (ip_s.parse::<Ipv4Addr>(), plen) {
            add_net(&mut nets, ip, plen);
        }
    }
    nets
}

fn add_net(nets: &mut Vec<(Ipv4Addr, u8)>, ip: Ipv4Addr, plen: u32) {
    if ip.is_loopback() || ip.is_link_local() {
        return;
    }
    let prefix = plen.clamp(24, 30) as u8; // never scan more than a /24
    let net = (
        Ipv4Addr::from(u32::from(ip) & (u32::MAX << (32 - prefix))),
        prefix,
    );
    if !nets.contains(&net) {
        nets.push(net);
    }
}

fn quad_after_colon(line: &str) -> Option<Ipv4Addr> {
    let v = line.split_once(':')?.1.trim();
    v.chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect::<String>()
        .parse()
        .ok()
}

/// Windows `ipconfig` output. Locale-proof: an "IPv4" line is followed by its mask, the next address starting with 255.
pub fn parse_ipconfig(text: &str) -> Vec<(Ipv4Addr, u8)> {
    let lines: Vec<&str> = text.lines().collect();
    let mut nets = vec![];
    for (i, l) in lines.iter().enumerate() {
        if !l.contains("IPv4") {
            continue;
        }
        let Some(ip) = quad_after_colon(l) else {
            continue;
        };
        let mask = lines[i + 1..]
            .iter()
            .take(2)
            .filter_map(|l| quad_after_colon(l))
            .find(|m| m.octets()[0] == 255);
        if let Some(m) = mask {
            add_net(&mut nets, ip, u32::from(m).count_ones());
        }
    }
    nets
}

fn hosts_of((net, prefix): (Ipv4Addr, u8)) -> Vec<Ipv4Addr> {
    let base = u32::from(net);
    let size = 1u32 << (32 - prefix);
    (1..size - 1).map(|i| Ipv4Addr::from(base + i)).collect()
}

/// Label for a host that answers on SSH: an OpenWrt LuCI page, or a dropbear banner; otherwise None.
pub fn classify(banner: &str, html: &str) -> Option<String> {
    let h = html.to_lowercase();
    if h.contains("luci") || h.contains("openwrt") {
        let title = h
            .find("<title>")
            .and_then(|a| html[a + 7..].split('<').next())
            .map(str::trim)
            .filter(|t| !t.is_empty());
        return Some(match title {
            Some(t) => format!("OpenWrt (LuCI: {t})"),
            None => "OpenWrt (LuCI)".to_string(),
        });
    }
    banner
        .to_lowercase()
        .contains("dropbear")
        .then(|| "likely OpenWrt (dropbear SSH)".to_string())
}

fn probe(ip: Ipv4Addr) -> Option<(Ipv4Addr, String)> {
    let mut c =
        TcpStream::connect_timeout(&SocketAddr::from((ip, 22)), Duration::from_millis(600)).ok()?;
    c.set_read_timeout(Some(Duration::from_secs(1))).ok();
    let mut buf = [0u8; 64];
    let n = c.read(&mut buf).unwrap_or(0);
    let banner = String::from_utf8_lossy(&buf[..n]).into_owned();
    let html = ureq::AgentBuilder::new()
        .timeout(Duration::from_millis(1500))
        .build()
        .get(&format!("http://{ip}/"))
        .call()
        .ok()
        .map(|r| {
            let mut b = vec![];
            let _ = r.into_reader().take(8192).read_to_end(&mut b);
            String::from_utf8_lossy(&b).into_owned()
        })
        .unwrap_or_default();
    classify(&banner, &html).map(|l| (ip, l))
}

/// Scan the networks this laptop is on and let the user pick a router.
pub fn discover(yes: bool) -> Result<String> {
    let out = if cfg!(target_os = "macos") {
        Command::new("ifconfig").output()
    } else if cfg!(windows) {
        Command::new("ipconfig").output()
    } else {
        Command::new("ip")
            .args(["-4", "-o", "addr", "show"])
            .output()
    };
    let text = out
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default();
    let nets = if cfg!(windows) {
        parse_ipconfig(&text)
    } else {
        parse_subnets(&text)
    };
    let label = if nets.is_empty() {
        "the local network".to_string()
    } else {
        nets.iter()
            .map(|(n, p)| format!("{n}/{p}"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let step = ui::Step::new(&format!("Scanning {label} for OpenWrt routers"));
    let mut hosts: Vec<Ipv4Addr> = nets.iter().flat_map(|n| hosts_of(*n)).collect();
    for g in ["192.168.1.1", "192.168.0.1", "10.0.0.1"] {
        let g: Ipv4Addr = g.parse().unwrap();
        if !hosts.contains(&g) {
            hosts.push(g);
        }
    }
    let found = Mutex::new(vec![]);
    let next = Mutex::new(hosts.iter());
    std::thread::scope(|s| {
        for _ in 0..64 {
            s.spawn(|| loop {
                let ip = match next.lock().unwrap().next() {
                    Some(ip) => *ip,
                    None => break,
                };
                if let Some(hit) = probe(ip) {
                    found.lock().unwrap().push(hit);
                }
            });
        }
    });
    let mut found = found.into_inner().unwrap();
    found.sort();
    step.done(&if found.is_empty() {
        "No OpenWrt router found on the local network".to_string()
    } else {
        format!(
            "Found {} OpenWrt router{}",
            found.len(),
            if found.len() == 1 { "" } else { "s" }
        )
    });
    if yes {
        return match found.as_slice() {
            [(ip, _)] => Ok(ip.to_string()),
            _ => bail!("cannot pick a router automatically; pass its address"),
        };
    }
    let gw = route_info("1.1.1.1").1;
    let mut items: Vec<String> = found
        .iter()
        .map(|(ip, l)| {
            format!(
                "{}  {}{}",
                style(ip).bold(),
                style(l).dim(),
                if gw.as_deref() == Some(&ip.to_string()) {
                    style("  ← your gateway").cyan().to_string()
                } else {
                    String::new()
                }
            )
        })
        .collect();
    items.push("Enter an address manually".to_string());
    let pick = ui::choose("Which router should I set up?", &items, 0)?;
    match found.get(pick) {
        Some((ip, _)) => Ok(ip.to_string()),
        None => ui::ask("Router address", "192.168.1.1"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verdicts() {
        let h = "192.168.1.1";
        let r = |i: &str, g: Option<&str>| (Some(i.to_string()), g.map(String::from));
        assert_eq!(
            verdict(h, &r("en5", None), &r("en0", Some("10.0.0.1"))),
            Verdict::Split
        );
        assert_eq!(
            verdict(h, &r("en0", None), &r("en0", Some(h))),
            Verdict::Through
        );
        assert_eq!(
            verdict(h, &r("en0", None), &r("en0", Some("10.0.0.1"))),
            Verdict::Other
        );
        assert_eq!(verdict(h, &(None, None), &r("en0", None)), Verdict::Unknown);
    }

    #[test]
    fn subnets() {
        let mac = "lo0: inet 127.0.0.1 netmask 0xff000000\nen0: inet 192.168.80.246 netmask 0xffffff00 broadcast 192.168.80.255";
        let lin = "2: eth0    inet 10.1.2.3/16 brd 10.1.255.255 scope global eth0\n1: lo inet 127.0.0.1/8";
        assert_eq!(
            parse_subnets(mac),
            vec![("192.168.80.0".parse().unwrap(), 24)]
        );
        assert_eq!(parse_subnets(lin), vec![("10.1.2.0".parse().unwrap(), 24)]);
    }

    #[test]
    fn ipconfig_subnets() {
        let win = "Ethernet adapter Ethernet:\r\n\r\n   IPv4 Address. . . . . . . . . . . : 192.168.1.50(Preferred)\r\n   Subnet Mask . . . . . . . . . . . : 255.255.255.0\r\n   Default Gateway . . . . . . . . . : 192.168.1.1\r\n\r\nWireless LAN adapter Wi-Fi:\r\n   Autoconfiguration IPv4 Address. . : 169.254.3.4\r\n   Subnet Mask . . . . . . . . . . . : 255.255.0.0\r\n";
        assert_eq!(
            parse_ipconfig(win),
            vec![("192.168.1.0".parse().unwrap(), 24)]
        );
    }

    #[test]
    fn classifies() {
        assert!(classify("", "<title>OpenWrt | Overview</title> luci")
            .unwrap()
            .contains("LuCI"));
        assert!(classify("SSH-2.0-dropbear_2024.86", "")
            .unwrap()
            .contains("dropbear"));
        assert!(classify("SSH-2.0-OpenSSH_9.6", "<html>nas</html>").is_none());
    }
}
