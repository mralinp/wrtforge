//! Optional extras: WireGuard, OpenVPN and an SSH SOCKS tunnel. (Passwall2 lives in steps.rs.)
//! Every change that can cut the LAN's internet is verified and rolled back on failure.
use crate::owrt::Device;
use crate::ssh::Router;
use crate::{steps, ui};
use anyhow::{bail, Context, Result};
use clap::ValueEnum;
use std::io::Write;
use std::process::{Command, Stdio};

#[derive(Clone, Copy, PartialEq, Eq, Debug, ValueEnum)]
pub enum Extra {
    Passwall2,
    Wireguard,
    Openvpn,
    SshTunnel,
    /// choose nothing
    None,
}

/// Off, install the packages only, or install and configure from user input.
#[derive(Debug, PartialEq)]
pub enum Setup<T> {
    Off,
    Install,
    Configure(T),
}

impl<T> Setup<T> {
    pub fn on(&self) -> bool {
        !matches!(self, Setup::Off)
    }
}

pub struct Extras {
    pub passwall: bool,
    pub wireguard: Setup<Wg>,
    pub openvpn: Setup<String>,
    pub ssh: Setup<SshSpec>,
}

impl Extras {
    pub fn lines(&self, apk: bool) -> Vec<String> {
        let how = |on: bool, cfg: bool| {
            if !on {
                "off"
            } else if cfg {
                "install + configure"
            } else {
                "install only (configure in LuCI)"
            }
        };
        vec![
            format!(
                "Passwall2: {}",
                if self.passwall {
                    format!(
                        "install from official feeds ({})",
                        if apk { "apk" } else { "opkg" }
                    )
                } else {
                    "skipped".into()
                }
            ),
            format!(
                "WireGuard: {}",
                how(
                    self.wireguard.on(),
                    matches!(self.wireguard, Setup::Configure(_))
                )
            ),
            format!(
                "OpenVPN: {}",
                how(
                    self.openvpn.on(),
                    matches!(self.openvpn, Setup::Configure(_))
                )
            ),
            format!(
                "SSH tunnel: {}",
                how(self.ssh.on(), matches!(self.ssh, Setup::Configure(_)))
            ),
        ]
    }
}

// ───────────────────────────── helpers ─────────────────────────────

/// Values that are pasted into router shell scripts must be boring.
fn safe(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "+/=._:,-%[]@".contains(c))
}

fn q(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

fn pkg(apk: bool, pkgs: &str) -> String {
    if apk {
        format!("apk update >/dev/null; apk add {pkgs} >/dev/null")
    } else {
        format!("opkg update >/dev/null; opkg install {pkgs} >/dev/null")
    }
}

const WAN_ZONE: &str = r#"wan=$(i=0; while n=$(uci -q get firewall.@zone[$i].name); do [ "$n" = wan ] && { echo $i; break; }; i=$((i+1)); done)"#;

fn status(out: &str) -> Option<&str> {
    out.lines().rev().find_map(|l| l.strip_prefix("STATUS:"))
}

// ───────────────────────────── WireGuard ─────────────────────────────

#[derive(Debug, PartialEq)]
pub struct Peer {
    pub public_key: String,
    pub preshared: Option<String>,
    pub host: String,
    pub port: u16,
    pub allowed: Vec<String>,
    pub keepalive: u32,
}

#[derive(Debug, PartialEq)]
pub struct Wg {
    pub private_key: String,
    pub addresses: Vec<String>,
    pub mtu: Option<u32>,
    pub peers: Vec<Peer>,
}

fn endpoint(v: &str) -> Result<(String, u16)> {
    let (h, p) = v
        .rsplit_once(':')
        .with_context(|| format!("bad Endpoint '{v}'"))?;
    Ok((
        h.trim_matches(['[', ']']).to_string(),
        p.parse()
            .with_context(|| format!("bad port in Endpoint '{v}'"))?,
    ))
}

fn list(v: &str) -> Vec<String> {
    v.split(',')
        .map(|x| x.trim().to_string())
        .filter(|x| !x.is_empty())
        .collect()
}

/// Parse a standard wg-quick client config.
pub fn parse_wg(text: &str) -> Result<Wg> {
    let (mut private_key, mut addresses, mut mtu) = (String::new(), vec![], None);
    let mut peers: Vec<Peer> = vec![];
    let mut in_peer = false;
    for raw in text.lines() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.eq_ignore_ascii_case("[interface]") {
            in_peer = false;
        } else if line.eq_ignore_ascii_case("[peer]") {
            in_peer = true;
            peers.push(Peer {
                public_key: String::new(),
                preshared: None,
                host: String::new(),
                port: 0,
                allowed: vec![],
                keepalive: 25,
            });
        } else if let Some((k, v)) = line.split_once('=') {
            let (k, v) = (k.trim().to_lowercase(), v.trim());
            match (in_peer, k.as_str()) {
                (false, "privatekey") => private_key = v.to_string(),
                (false, "address") => addresses.extend(list(v)),
                (false, "mtu") => mtu = v.parse().ok(),
                (true, _) => {
                    let p = peers.last_mut().unwrap();
                    match k.as_str() {
                        "publickey" => p.public_key = v.to_string(),
                        "presharedkey" => p.preshared = Some(v.to_string()),
                        "endpoint" => (p.host, p.port) = endpoint(v)?,
                        "allowedips" => p.allowed.extend(list(v)),
                        "persistentkeepalive" => p.keepalive = v.parse().unwrap_or(25).max(1),
                        _ => {}
                    }
                }
                _ => {}
            }
        }
    }
    if private_key.is_empty() || addresses.is_empty() {
        bail!("WireGuard config needs PrivateKey and Address in [Interface]");
    }
    if peers.is_empty()
        || peers
            .iter()
            .any(|p| p.public_key.is_empty() || p.host.is_empty() || p.allowed.is_empty())
    {
        bail!("every [Peer] needs PublicKey, Endpoint and AllowedIPs");
    }
    let all: Vec<&str> = std::iter::once(private_key.as_str())
        .chain(addresses.iter().map(String::as_str))
        .chain(peers.iter().flat_map(|p| {
            [p.public_key.as_str(), p.host.as_str()]
                .into_iter()
                .chain(p.preshared.as_deref())
                .chain(p.allowed.iter().map(String::as_str))
        }))
        .collect();
    if let Some(bad) = all.iter().find(|s| !safe(s)) {
        bail!("unexpected characters in WireGuard config value '{bad}'");
    }
    if peers.len() > 10 {
        bail!("at most 10 peers are supported");
    }
    Ok(Wg {
        private_key,
        addresses,
        mtu,
        peers,
    })
}

pub fn wg_script(wg: &Wg, apk: bool) -> String {
    let mut s = format!("{}\nuci -q delete network.wg0 || true\nfor n in 0 1 2 3 4 5 6 7 8 9; do uci -q delete network.wg0_peer$n || true; done\n", pkg(apk, "wireguard-tools luci-proto-wireguard"));
    s += &format!("uci set network.wg0=interface\nuci set network.wg0.proto='wireguard'\nuci set network.wg0.private_key={}\n", q(&wg.private_key));
    for a in &wg.addresses {
        s += &format!("uci add_list network.wg0.addresses={}\n", q(a));
    }
    if let Some(m) = wg.mtu {
        s += &format!("uci set network.wg0.mtu='{m}'\n");
    }
    for (i, p) in wg.peers.iter().enumerate() {
        let n = format!("network.wg0_peer{i}");
        s += &format!("uci set {n}=wireguard_wg0\nuci set {n}.public_key={}\nuci set {n}.endpoint_host={}\nuci set {n}.endpoint_port='{}'\nuci set {n}.persistent_keepalive='{}'\nuci set {n}.route_allowed_ips='1'\n", q(&p.public_key), q(&p.host), p.port, p.keepalive);
        if let Some(k) = &p.preshared {
            s += &format!("uci set {n}.preshared_key={}\n", q(k));
        }
        for a in &p.allowed {
            s += &format!("uci add_list {n}.allowed_ips={}\n", q(a));
        }
    }
    s += &format!(
        r#"uci commit network
{WAN_ZONE}
[ -z "$wan" ] || {{ uci -q del_list firewall.@zone[$wan].network=wg0 || true; uci add_list firewall.@zone[$wan].network=wg0; uci commit firewall; }}
rollback() {{
  uci -q delete network.wg0 || true
  for n in 0 1 2 3 4 5 6 7 8 9; do uci -q delete network.wg0_peer$n || true; done
  uci commit network
  [ -z "$wan" ] || {{ uci -q del_list firewall.@zone[$wan].network=wg0 || true; uci commit firewall; }}
  /etc/init.d/network reload >/dev/null 2>&1; /etc/init.d/firewall reload >/dev/null 2>&1
}}
/etc/init.d/network reload >/dev/null 2>&1; /etc/init.d/firewall reload >/dev/null 2>&1
i=0; ok=
while [ $i -lt 25 ]; do
  wg show wg0 latest-handshakes 2>/dev/null | awk '$2>0{{f=1}} END{{exit !f}}' && {{ ok=1; break; }}
  i=$((i+1)); sleep 1
done
if [ -n "$ok" ]; then echo STATUS:WG_OK; else rollback; echo STATUS:WG_FAILED; fi
"#
    );
    s
}

// ───────────────────────────── OpenVPN ─────────────────────────────

pub fn check_ovpn(text: &str) -> Result<()> {
    if !text.lines().any(|l| l.trim_start().starts_with("remote ")) {
        bail!("this does not look like an OpenVPN client config (no 'remote' line)");
    }
    if text.len() > 256 * 1024 {
        bail!("OpenVPN config is unreasonably large");
    }
    Ok(())
}

pub fn ovpn_needs_credentials(text: &str) -> bool {
    text.lines().any(|l| l.trim() == "auth-user-pass")
}

pub fn ovpn_script(apk: bool) -> String {
    format!(
        r#"{pkg}
chmod 600 /etc/openvpn/wrtforge.ovpn
uci -q delete openvpn.wrtforge || true
uci set openvpn.wrtforge=openvpn
uci set openvpn.wrtforge.enabled='1'
uci set openvpn.wrtforge.config='/etc/openvpn/wrtforge.ovpn'
uci commit openvpn
uci -q delete network.vpn0 || true
uci set network.vpn0=interface
uci set network.vpn0.proto='none'
uci set network.vpn0.device='tun0'
uci commit network
{WAN_ZONE}
[ -z "$wan" ] || {{ uci -q del_list firewall.@zone[$wan].network=vpn0 || true; uci add_list firewall.@zone[$wan].network=vpn0; uci commit firewall; }}
rollback() {{
  /etc/init.d/openvpn stop >/dev/null 2>&1
  uci -q delete openvpn.wrtforge || true; uci commit openvpn
  uci -q delete network.vpn0 || true; uci commit network
  [ -z "$wan" ] || {{ uci -q del_list firewall.@zone[$wan].network=vpn0 || true; uci commit firewall; }}
  /etc/init.d/network reload >/dev/null 2>&1; /etc/init.d/firewall reload >/dev/null 2>&1
}}
/etc/init.d/network reload >/dev/null 2>&1; /etc/init.d/firewall reload >/dev/null 2>&1
/etc/init.d/openvpn enable; /etc/init.d/openvpn restart >/dev/null 2>&1
i=0; ok=
while [ $i -lt 40 ]; do
  ip -4 addr show tun0 2>/dev/null | grep -q inet && {{ ok=1; break; }}
  i=$((i+1)); sleep 1
done
if [ -n "$ok" ]; then echo STATUS:OVPN_OK; else logread 2>/dev/null | grep -i openvpn | tail -4 | sed 's/^/LOG:/'; rollback; echo STATUS:OVPN_FAILED; fi
"#,
        pkg = pkg(apk, "openvpn-openssl luci-app-openvpn")
    )
}

// ───────────────────────────── SSH tunnel ─────────────────────────────

#[derive(Debug, PartialEq)]
pub struct SshSpec {
    pub user: String,
    pub host: String,
    pub port: u16,
    pub lan: bool,
}

pub fn parse_target(t: &str, lan: bool) -> Result<SshSpec> {
    let (user, rest) = t
        .split_once('@')
        .context("use user@host or user@host:port")?;
    let (host, port) = match rest.rsplit_once(':') {
        Some((h, p)) if !h.contains(':') => (h, p.parse().context("bad port")?),
        _ => (rest, 22),
    };
    if !safe(user) || !safe(host) {
        bail!("unexpected characters in '{t}'");
    }
    Ok(SshSpec {
        user: user.into(),
        host: host.into(),
        port,
        lan,
    })
}

const SSH_KEY_SCRIPT: &str = r#"[ -f /root/.ssh/id_ed25519 ] || { mkdir -p /root/.ssh; chmod 700 /root/.ssh; ssh-keygen -q -t ed25519 -N '' -f /root/.ssh/id_ed25519; }
echo "STATUS:PUBKEY $(cat /root/.ssh/id_ed25519.pub)"
"#;

pub fn ssh_install_script(apk: bool) -> String {
    format!(
        "{}\n{SSH_KEY_SCRIPT}",
        pkg(
            apk,
            "openssh-client openssh-keygen sshtunnel luci-app-sshtunnel"
        )
    )
}

pub fn ssh_config_script(s: &SshSpec) -> String {
    format!(
        r#"uci -q delete sshtunnel.wrtforge || true
uci -q delete sshtunnel.wrtforge_socks || true
uci set sshtunnel.wrtforge=server
uci set sshtunnel.wrtforge.user={user}
uci set sshtunnel.wrtforge.hostname={host}
uci set sshtunnel.wrtforge.port='{port}'
uci set sshtunnel.wrtforge.retrydelay='5'
uci set sshtunnel.wrtforge.IdentityFile='/root/.ssh/id_ed25519'
uci set sshtunnel.wrtforge.ServerAliveInterval='30'
uci set sshtunnel.wrtforge.ServerAliveCountMax='3'
uci set sshtunnel.wrtforge.StrictHostKeyChecking='accept-new'
uci set sshtunnel.wrtforge_socks=tunnelD
uci set sshtunnel.wrtforge_socks.server='wrtforge'
uci set sshtunnel.wrtforge_socks.localaddress='{bind}'
uci set sshtunnel.wrtforge_socks.localport='1080'
uci commit sshtunnel
/etc/init.d/sshtunnel enable; /etc/init.d/sshtunnel restart >/dev/null 2>&1
i=0; ok=
while [ $i -lt 20 ]; do
  netstat -ln 2>/dev/null | grep -q ':1080 ' && {{ ok=1; break; }}
  i=$((i+1)); sleep 1
done
if [ -n "$ok" ]; then echo STATUS:SSH_OK; else logread 2>/dev/null | grep -i sshtunnel | tail -3 | sed 's/^/LOG:/'; echo STATUS:SSH_DOWN; fi
"#,
        user = q(&s.user),
        host = q(&s.host),
        port = s.port,
        bind = if s.lan { "*" } else { "127.0.0.1" }
    )
}

/// Put the router's public key on the user's own server, from the laptop (ssh asks for that server's password).
fn authorize_on_server(s: &SshSpec, pubkey: &str) -> Result<bool> {
    let remote = "k=$(cat); mkdir -p ~/.ssh; chmod 700 ~/.ssh; grep -qxF \"$k\" ~/.ssh/authorized_keys 2>/dev/null || echo \"$k\" >> ~/.ssh/authorized_keys; chmod 600 ~/.ssh/authorized_keys";
    let mut child = Command::new("ssh")
        .args([
            "-p",
            &s.port.to_string(),
            "-o",
            "StrictHostKeyChecking=accept-new",
        ])
        .arg(format!("{}@{}", s.user, s.host))
        .arg(remote)
        .stdin(Stdio::piped())
        .spawn()
        .context("could not start ssh")?;
    child
        .stdin
        .take()
        .context("no stdin")?
        .write_all(format!("{pubkey}\n").as_bytes())?;
    Ok(child.wait()?.success())
}

// ───────────────────────────── entry point ─────────────────────────────

pub fn apply(r: &Router, d: &Device, e: &Extras) -> Result<()> {
    if let Setup::Install | Setup::Configure(_) = &e.wireguard {
        let s = ui::Step::new("Setting up WireGuard");
        match &e.wireguard {
            Setup::Configure(wg) => match status(&r.sh(&wg_script(wg, d.apk))?) {
                Some("WG_OK") => s.done("WireGuard is up, handshake verified"),
                _ => s.warn("WireGuard did not connect in 25s; rolled back, the router's internet is unchanged"),
            },
            _ => {
                r.sh(&pkg(d.apk, "wireguard-tools luci-proto-wireguard"))?;
                s.done("WireGuard installed; add your interface in LuCI -> Network -> Interfaces");
            }
        }
    }
    if e.openvpn.on() {
        let s = ui::Step::new("Setting up OpenVPN");
        match &e.openvpn {
            Setup::Configure(text) => {
                r.sh("mkdir -p /etc/openvpn")?;
                r.put(text.as_bytes(), "/etc/openvpn/wrtforge.ovpn")?;
                let out = r.sh(&ovpn_script(d.apk))?;
                match status(&out) {
                    Some("OVPN_OK") => s.done("OpenVPN is up (tun0)"),
                    _ => {
                        let log: Vec<&str> =
                            out.lines().filter_map(|l| l.strip_prefix("LOG:")).collect();
                        s.warn("OpenVPN did not come up in 40s; rolled back");
                        log.iter().for_each(|l| ui::info(l));
                    }
                }
            }
            _ => {
                r.sh(&pkg(d.apk, "openvpn-openssl luci-app-openvpn"))?;
                s.done("OpenVPN installed; add your profile in LuCI -> VPN -> OpenVPN");
            }
        }
    }
    if e.ssh.on() {
        let s = ui::Step::new("Setting up the SSH tunnel");
        let out = r.sh(&ssh_install_script(d.apk))?;
        let pubkey = status(&out)
            .and_then(|x| x.strip_prefix("PUBKEY "))
            .context("could not read the router's SSH key")?
            .to_string();
        match &e.ssh {
            Setup::Configure(spec) => {
                s.done("SSH tunnel packages installed, router key ready");
                if ui::confirm(&format!("Authorize the router's key on {}@{} now? (ssh will ask for that server's password)", spec.user, spec.host), true)? && !authorize_on_server(spec, &pubkey)? {
                    ui::warn("could not authorize the key; add this line to the server's ~/.ssh/authorized_keys:");
                    ui::info(&pubkey);
                }
                let s = ui::Step::new("Starting the SOCKS5 tunnel");
                match status(&r.sh(&ssh_config_script(spec))?) {
                    Some("SSH_OK") => s.done(&format!("SOCKS5 proxy on port 1080 ({})", if spec.lan { "LAN" } else { "router only: use it as a SOCKS node in Passwall2" })),
                    _ => s.warn("tunnel is configured but not connected yet; check the server key, then: /etc/init.d/sshtunnel restart"),
                }
            }
            _ => {
                s.done("SSH tunnel installed; configure it in LuCI -> Services -> SSH Tunnels");
                ui::info(&format!("router public key: {pubkey}"));
            }
        }
    }
    if e.passwall {
        let s = ui::Step::new("Installing Passwall2 from the official feeds");
        let res = steps::passwall(r, d)?;
        let running = res.iter().any(|w| w == "RUNNING");
        s.done(&format!(
            "Passwall2 installed{}",
            if running { " and running" } else { "" }
        ));
        if !running {
            ui::info("open LuCI -> Services -> Passwall 2, add your node, enable it");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const WG: &str = "[Interface]\nPrivateKey = aGVsbG8gd29ybGQgdGhpcyBpcyBhIGtleSBmb3IgdGVzdCE=\nAddress = 10.8.0.2/32, fd00::2/128\nDNS = 1.1.1.1\nMTU = 1380\n\n[Peer]\nPublicKey = cHVibGljIGtleSBmb3IgdGVzdGluZyBwdXJwb3NlcyBvbmx5IQ==\nEndpoint = vpn.example.com:51820\nAllowedIPs = 0.0.0.0/0, ::/0\n";

    #[test]
    fn parses_wireguard() {
        let w = parse_wg(WG).unwrap();
        assert_eq!(w.addresses, ["10.8.0.2/32", "fd00::2/128"]);
        assert_eq!(w.mtu, Some(1380));
        assert_eq!(
            (
                w.peers[0].host.as_str(),
                w.peers[0].port,
                w.peers[0].keepalive
            ),
            ("vpn.example.com", 51820, 25)
        );
        assert_eq!(w.peers[0].allowed, ["0.0.0.0/0", "::/0"]);
        assert_eq!(
            endpoint("[2001:db8::1]:51820").unwrap(),
            ("2001:db8::1".to_string(), 51820)
        );
    }

    #[test]
    fn rejects_bad_wireguard() {
        assert!(parse_wg("[Interface]\nAddress=10.0.0.2/32\n").is_err());
        assert!(parse_wg(&WG.replace("vpn.example.com", "x;reboot")).is_err());
        assert!(parse_wg(&WG.replace("Endpoint = vpn.example.com:51820\n", "")).is_err());
    }

    #[test]
    fn scripts_have_the_safety_net() {
        let s = wg_script(&parse_wg(WG).unwrap(), true);
        assert!(
            s.contains("uci set network.wg0.proto='wireguard'")
                && s.contains("route_allowed_ips='1'")
        );
        assert!(
            s.contains("rollback")
                && s.contains("STATUS:WG_FAILED")
                && s.contains("apk add wireguard-tools")
        );
        assert!(
            ovpn_script(false).contains("opkg install openvpn-openssl")
                && ovpn_script(true).contains("STATUS:OVPN_FAILED")
        );
        let spec = parse_target("me@example.com:2222", true).unwrap();
        assert!(
            ssh_config_script(&spec).contains("localaddress='*'")
                && ssh_config_script(&spec).contains("port='2222'")
        );
    }

    #[test]
    fn parses_targets_and_ovpn() {
        assert_eq!(
            parse_target("me@h.example", false).unwrap(),
            SshSpec {
                user: "me".into(),
                host: "h.example".into(),
                port: 22,
                lan: false
            }
        );
        assert!(parse_target("nouser", false).is_err() && parse_target("a@b;c", false).is_err());
        assert!(
            check_ovpn("client\nremote vpn.example.com 1194\n").is_ok()
                && check_ovpn("client\n").is_err()
        );
        assert!(
            ovpn_needs_credentials("auth-user-pass\n")
                && !ovpn_needs_credentials("auth-user-pass creds.txt\n")
        );
        assert_eq!(q("a'b"), r"'a'\''b'");
    }

    /// Generated scripts must at least be valid shell.
    #[cfg(unix)]
    #[test]
    fn scripts_are_valid_sh() {
        let spec = parse_target("me@example.com", false).unwrap();
        for s in [
            wg_script(&parse_wg(WG).unwrap(), true),
            ovpn_script(true),
            ssh_install_script(false),
            ssh_config_script(&spec),
        ] {
            let mut c = Command::new("sh")
                .arg("-n")
                .stdin(Stdio::piped())
                .spawn()
                .unwrap();
            c.stdin.take().unwrap().write_all(s.as_bytes()).unwrap();
            assert!(c.wait().unwrap().success(), "invalid sh:\n{s}");
        }
    }

    /// Syntax-check the scripts with the router's own busybox ash (changes nothing):
    /// `WRTFORGE_TEST_HOST=192.168.1.1 cargo test -- --ignored`
    #[test]
    #[ignore = "needs a live router"]
    fn scripts_parse_on_router() {
        let host = std::env::var("WRTFORGE_TEST_HOST").expect("set WRTFORGE_TEST_HOST");
        let mut r = Router::new(&host, "root", 22, crate::ssh::Auth::Key).unwrap();
        r.connect().unwrap();
        let spec = parse_target("me@example.com", true).unwrap();
        for s in [
            wg_script(&parse_wg(WG).unwrap(), true),
            ovpn_script(true),
            ssh_install_script(true),
            ssh_config_script(&spec),
        ] {
            let o = r.run("sh -n", Some(s.as_bytes())).unwrap();
            assert!(o.ok, "router sh rejected:\n{}\n{s}", o.stderr);
        }
    }
}
