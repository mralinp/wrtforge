//! wrtforge: forge a clean, official OpenWrt (+ Passwall2, recovery slot) onto a router over SSH.
mod bench;
mod fsx;
mod net;
mod owrt;
mod ssh;
mod steps;
mod ui;
mod vpn;

use anyhow::{bail, Context, Result};
use clap::Parser;
use console::style;
use ssh::{Auth, Router};
use std::path::PathBuf;
use vpn::{Extra, Extras, Setup};

#[derive(Parser)]
#[command(
    version,
    about = "Forge a clean, official OpenWrt + Passwall2 (with a recovery slot) onto your router over SSH"
)]
struct Cli {
    /// Router address (found by scanning the local network if omitted)
    host: Option<String>,
    #[arg(short, long, default_value = "root")]
    user: String,
    #[arg(short, long, default_value_t = 22)]
    port: u16,
    /// How to log in (asked if omitted)
    #[arg(long, value_enum)]
    auth: Option<Auth>,
    /// OpenWrt release to install (default: latest stable)
    #[arg(long)]
    release: Option<String>,
    /// Extras to set up, comma separated (default: passwall2). `none` for nothing
    #[arg(long, value_enum, value_delimiter = ',')]
    with: Vec<Extra>,
    /// WireGuard client config (.conf) to import; implies --with wireguard
    #[arg(long)]
    wireguard_conf: Option<PathBuf>,
    /// OpenVPN client profile (.ovpn) to import; implies --with openvpn
    #[arg(long)]
    openvpn_conf: Option<PathBuf>,
    /// SSH server for a SOCKS5 tunnel (user@host[:port]); implies --with ssh-tunnel
    #[arg(long)]
    ssh_tunnel: Option<String>,
    /// Share the SSH tunnel's SOCKS5 proxy (port 1080) with the LAN, not just the router
    #[arg(long)]
    tunnel_lan: bool,
    /// Same as leaving passwall2 out of --with
    #[arg(long, hide = true)]
    no_passwall: bool,
    /// Keep the old firmware in the second slot (skip the final refresh)
    #[arg(long)]
    no_fallback: bool,
    /// Do not scan the network; just ask for the address
    #[arg(long)]
    no_scan: bool,
    /// Inspect and plan only; change nothing
    #[arg(long)]
    dry_run: bool,
    /// With -y: also answer yes to the final "flash now?"
    #[arg(long)]
    go: bool,
    /// Where backups go (default: ~/wrtforge-backups)
    #[arg(long)]
    backup_dir: Option<PathBuf>,
    /// Benchmark throughput through this router instead of flashing it
    #[arg(long)]
    bench: bool,
    /// Name of this benchmark run, e.g. gale-passwall-on
    #[arg(long, default_value = "run")]
    label: String,
    /// Benchmark rounds (the median is reported) and seconds per transfer
    #[arg(long, default_value_t = 3)]
    rounds: u32,
    #[arg(long, default_value_t = 10)]
    secs: u64,
    /// CSV file the benchmark result is appended to
    #[arg(long, default_value = "wrtforge-bench.csv")]
    bench_out: PathBuf,
    /// Accept every default, no questions
    #[arg(short, long)]
    yes: bool,
}

fn main() {
    let cli = Cli::parse();
    ui::set_yes(cli.yes);
    ui::banner();
    if let Err(e) = run(cli) {
        eprintln!("\n  {} {e:#}\n", style("✖").red());
        std::process::exit(1);
    }
}

fn run(cli: Cli) -> Result<()> {
    let host = match &cli.host {
        Some(h) => h.clone(),
        None if cli.no_scan => ui::ask("Router address", "192.168.1.1")?,
        None => net::discover(cli.yes)?,
    };
    let auth = match cli.auth {
        Some(a) => a,
        None => {
            let items = [
                "SSH key already authorised on the router",
                "Password (ssh asks for it; never stored)",
                "Password once, then install my SSH key (no more prompts)",
            ]
            .map(String::from);
            [Auth::Key, Auth::Password, Auth::CopyKey]
                [ui::choose("How should I log in?", &items, 2)?]
        }
    };
    let mut r = Router::new(&host, &cli.user, cli.port, auth)?;

    ui::head("Network");
    if !cli.bench && !net::network_check(&host, cli.dry_run)? {
        bail!("cancelled; connect a second network and run again");
    }

    ui::head("Connect");
    if auth != Auth::Key {
        ui::info(&format!(
            "ssh will ask for the password for {}@{}",
            cli.user, host
        ));
    }
    if !ssh::MUX && auth != Auth::Key {
        ui::info("Windows: your SSH key is installed with this one password prompt; everything else uses the key");
    }
    r.connect()?;
    if auth == Auth::CopyKey && ssh::MUX {
        let s = ui::Step::new("Installing your SSH public key");
        r.install_key()?;
        s.done("SSH key installed; no more password prompts");
    }

    ui::head("Inspect");
    let s = ui::Step::new("Reading device info");
    let d = owrt::inspect(&r)?;
    s.done(&format!(
        "{} {}",
        style(&d.model).bold(),
        style(format!("({})", d.board)).dim()
    ));
    ui::info(&format!(
        "running {} {}   {} / {}",
        d.distro, d.release, d.target, d.arch
    ));
    if cli.bench {
        ui::head("Benchmark");
        return bench::run(
            &r,
            &d,
            &cli.label,
            cli.rounds.max(1),
            cli.secs.max(1),
            &cli.bench_out,
        );
    }
    if !d.sysupgrade {
        bail!("no sysupgrade on the router");
    }

    let s = ui::Step::new("Finding the latest stable OpenWrt");
    let ver = match &cli.release {
        Some(v) => v.clone(),
        None => owrt::fetch_json(&format!("{}/.versions.json", owrt::DL))?["stable_version"]
            .as_str()
            .context("no stable_version")?
            .to_string(),
    };
    let base = format!("{}/releases/{ver}/targets/{}/", owrt::DL, d.target);
    let prof = owrt::fetch_json(&format!("{base}profiles.json"))?;
    let img = owrt::pick_image(&prof, &d.board, "sysupgrade")
        .with_context(|| format!("OpenWrt {ver} has no image for {}", d.board))?;
    s.done(&format!(
        "OpenWrt {}  {}",
        style(&ver).bold(),
        style(format!("{}, {:.1} MB", img.name, img.size as f64 / 1e6)).dim()
    ));
    if d.mem < img.size * 2 {
        bail!("not enough free RAM on the router to stage the image");
    }

    let slot = d.dual && !cli.no_fallback;
    let recovery = if d.dual {
        "old firmware stays in the other slot until the new one is verified (bootloader falls back to it)"
    } else {
        "backup + official factory image saved here for USB/TFTP recovery"
    };
    let home = fsx::home()?;
    let bdir = cli
        .backup_dir
        .clone()
        .unwrap_or_else(|| home.join("wrtforge-backups"))
        .join(format!(
            "{}-{}",
            d.board.replace(',', "_"),
            fsx::timestamp()
        ));

    let (mut carry_net, mut carry_pw) = (false, false);
    if !cli.dry_run {
        ui::head("Plan");
    }
    let extras = choose_extras(&cli)?;
    if !cli.dry_run {
        carry_net = ui::confirm(
            "Carry over LAN/Wi-Fi/DHCP settings? (keeps the router reachable)",
            true,
        )?;
        if d.had_pw2 && extras.passwall {
            carry_pw = ui::confirm("Carry over your existing Passwall2 config?", true)?;
        }
    }
    println!();
    let keep = format!(
        "keep: SSH keys + root password{}{}",
        if carry_net { ", network/Wi-Fi" } else { "" },
        if carry_pw { ", Passwall2" } else { "" }
    );
    let mut plan = vec![
        format!(
            "back up config, package list and bootloader/calibration partitions to {}",
            bdir.display()
        ),
        format!("recovery: {recovery}"),
        format!("flash OpenWrt {ver} (currently {})", d.release),
        keep,
    ];
    plan.extend(extras.lines(d.apk));
    plan.push(format!(
        "recovery slot: {}",
        if slot {
            "afterwards replace the old firmware with the official image"
        } else {
            "n/a"
        }
    ));
    for line in plan {
        println!("    {} {line}", style("•").dim());
    }
    if cli.dry_run {
        println!("\n  {}\n", style("dry run: nothing changed").green());
        return Ok(());
    }
    ui::warn("the router reboots and goes offline for ~5-15 minutes");
    if !(cli.go || ui::confirm("Go ahead?", false)?) {
        bail!("cancelled; the router was not changed");
    }

    ui::head("Backup & recovery");
    let s = ui::Step::new("Backing up the router to your laptop");
    let n = steps::backup(&r, &bdir)?;
    s.done(&format!(
        "Backup verified {}",
        style(format!("({}, {n} flash partitions)", bdir.display())).dim()
    ));
    ui::info("contains your Wi-Fi passwords and keys; keep it private");
    let dest = dirs_cache().join(&img.name);
    let s = ui::Step::new(&format!("Downloading {}", img.name));
    owrt::download(&format!("{base}{}", img.name), &dest, &img.sha256)?;
    s.done(&format!(
        "Downloaded and verified sha256 {}…",
        &img.sha256[..12]
    ));
    if d.dual {
        ui::info(&format!("recovery path: {recovery}"));
    } else if let Some(fac) = owrt::pick_image(&prof, &d.board, "factory") {
        let s = ui::Step::new("Saving the factory image for recovery");
        owrt::download(
            &format!("{base}{}", fac.name),
            &bdir.join(&fac.name),
            &fac.sha256,
        )?;
        s.done(&format!("Recovery image saved: {}", fac.name));
    } else {
        ui::warn("no factory image published for this board; rely on the vendor recovery method");
    }

    ui::head("Flash");
    let after = if carry_net {
        host.clone()
    } else {
        ui::ask("Router address after flashing", "192.168.1.1")?
    };
    let s = ui::Step::new("Uploading and checking the image on the router");
    steps::build_keep(&r, carry_net, carry_pw)?;
    steps::flash(&r, &std::fs::read(&dest)?)?;
    s.done("Image accepted by sysupgrade; flashing and rebooting");
    let s = ui::Step::new("Waiting for the router to come back");
    steps::wait_reboot(&after, cli.port)?;
    s.done("Router is back");
    r.host = after;
    r.reset();
    r.connect()?;
    let now = owrt::inspect(&r)?;
    if now.release != ver {
        bail!(
            "router reports {} after flashing, expected {ver}",
            now.release
        );
    }
    ui::info(&format!("now running {} {}", now.distro, now.release));

    if extras.passwall || extras.wireguard.on() || extras.openvpn.on() || extras.ssh.on() {
        ui::head("Extras");
        vpn::apply(&r, &now, &extras)?;
    }

    if slot {
        ui::head("Recovery slot");
        ui::info("the new system works, so the old firmware in the other slot is no longer needed");
        if ui::confirm(
            "Replace it with the official OpenWrt image (a clean, fresh fallback)?",
            true,
        )? {
            let s = ui::Step::new("Writing official OpenWrt to the second slot");
            let k = steps::fallback(&r, &dest)?;
            s.done(&format!(
                "Fallback slot ready {}",
                style(format!("({k}, verified by hash)")).dim()
            ));
            ui::info(
                "boots automatically if the main slot ever fails; stock settings, 192.168.1.1",
            );
        }
    }
    println!(
        "\n  {}  {}\n",
        style("✔ done").green(),
        style(format!(
            "LuCI: http://{}   backup: {}",
            r.host,
            bdir.display()
        ))
        .dim()
    );
    Ok(())
}

/// Which extras to set up. Inputs (config files, server) are read and validated now, before anything is flashed.
fn choose_extras(cli: &Cli) -> Result<Extras> {
    let interactive = !cli.yes && !cli.dry_run;
    let mut sel: Vec<Extra> = cli
        .with
        .iter()
        .copied()
        .filter(|e| *e != Extra::None)
        .collect();
    let mut add = |e: Extra, on: bool| {
        if on && !sel.contains(&e) {
            sel.push(e);
        }
    };
    add(Extra::Wireguard, cli.wireguard_conf.is_some());
    add(Extra::Openvpn, cli.openvpn_conf.is_some());
    add(Extra::SshTunnel, cli.ssh_tunnel.is_some());
    if cli.with.is_empty() && sel.is_empty() {
        if interactive {
            let items = [
                "Passwall2 (transparent proxy with xray)",
                "WireGuard",
                "OpenVPN",
                "SSH tunnel (SOCKS5 over ssh)",
            ]
            .map(String::from);
            let all = [
                Extra::Passwall2,
                Extra::Wireguard,
                Extra::Openvpn,
                Extra::SshTunnel,
            ];
            let defaults = [!cli.no_passwall, false, false, false];
            for i in ui::multi(
                "Which extras should I set up? (space toggles, enter confirms)",
                &items,
                &defaults,
            )? {
                sel.push(all[i]);
            }
        } else if !cli.no_passwall {
            sel.push(Extra::Passwall2);
        }
    }
    if cli.no_passwall {
        sel.retain(|e| *e != Extra::Passwall2);
    }
    let has = |e: Extra| sel.contains(&e);

    let file_or_ask = |flag: &Option<PathBuf>, q: &str| -> Result<Option<PathBuf>> {
        match flag {
            Some(p) => Ok(Some(p.clone())),
            None if interactive => {
                let a = ui::ask(q, "")?;
                Ok((!a.trim().is_empty()).then(|| PathBuf::from(a.trim().trim_matches('"'))))
            }
            None => Ok(None),
        }
    };
    let wireguard = if !has(Extra::Wireguard) {
        Setup::Off
    } else {
        match file_or_ask(
            &cli.wireguard_conf,
            "WireGuard client config file (.conf), empty to set it up later in LuCI",
        )? {
            Some(p) => Setup::Configure(vpn::parse_wg(
                &std::fs::read_to_string(&p)
                    .with_context(|| format!("cannot read {}", p.display()))?,
            )?),
            None => Setup::Install,
        }
    };
    let openvpn = if !has(Extra::Openvpn) {
        Setup::Off
    } else {
        match file_or_ask(
            &cli.openvpn_conf,
            "OpenVPN profile (.ovpn), empty to set it up later in LuCI",
        )? {
            Some(p) => {
                let text = std::fs::read_to_string(&p)
                    .with_context(|| format!("cannot read {}", p.display()))?;
                vpn::check_ovpn(&text)?;
                if vpn::ovpn_needs_credentials(&text) {
                    ui::warn("this profile asks for a username/password (auth-user-pass); point it at a credentials file, or it cannot connect unattended");
                }
                Setup::Configure(text)
            }
            None => Setup::Install,
        }
    };
    let ssh = if !has(Extra::SshTunnel) {
        Setup::Off
    } else {
        let target = match &cli.ssh_tunnel {
            Some(t) => Some(t.clone()),
            None if interactive => Some(ui::ask(
                "SSH server for the tunnel (user@host[:port]), empty to set it up later in LuCI",
                "",
            )?)
            .filter(|t| !t.trim().is_empty()),
            None => None,
        };
        match target {
            Some(t) => {
                let lan = cli.tunnel_lan || (interactive && ui::confirm("Share the SOCKS5 proxy with your whole LAN? (otherwise only the router can use it)", false)?);
                Setup::Configure(vpn::parse_target(t.trim(), lan)?)
            }
            None => Setup::Install,
        }
    };
    Ok(Extras {
        passwall: has(Extra::Passwall2),
        wireguard,
        openvpn,
        ssh,
    })
}

fn dirs_cache() -> PathBuf {
    fsx::home()
        .unwrap_or_else(|_| std::env::temp_dir())
        .join(".cache/wrtforge")
}
