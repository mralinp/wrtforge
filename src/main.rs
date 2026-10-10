//! wrtforge: forge a clean, official OpenWrt (+ Passwall2, recovery slot) onto a router over SSH.
mod fsx;
mod net;
mod owrt;
mod ssh;
mod steps;
mod ui;

use anyhow::{bail, Context, Result};
use clap::Parser;
use console::style;
use ssh::{Auth, Router};
use std::path::PathBuf;

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
    #[arg(long)]
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
    if !net::network_check(&host, cli.dry_run)? {
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
        carry_net = ui::confirm(
            "Carry over LAN/Wi-Fi/DHCP settings? (keeps the router reachable)",
            true,
        )?;
        if d.had_pw2 && !cli.no_passwall {
            carry_pw = ui::confirm("Carry over your existing Passwall2 config?", true)?;
        }
    }
    println!();
    let keep = format!(
        "keep: SSH keys + root password{}{}",
        if carry_net { ", network/Wi-Fi" } else { "" },
        if carry_pw { ", Passwall2" } else { "" }
    );
    let pw_line = if cli.no_passwall {
        "Passwall2: skipped".to_string()
    } else {
        format!(
            "Passwall2: install from official feeds ({})",
            if d.apk { "apk" } else { "opkg" }
        )
    };
    for line in [
        format!(
            "back up config, package list and bootloader/calibration partitions to {}",
            bdir.display()
        ),
        format!("recovery: {recovery}"),
        format!("flash OpenWrt {ver} (currently {})", d.release),
        keep,
        pw_line,
        format!(
            "recovery slot: {}",
            if slot {
                "afterwards replace the old firmware with the official image"
            } else {
                "n/a"
            }
        ),
    ] {
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

    if !cli.no_passwall {
        ui::head("Passwall2");
        let s = ui::Step::new("Installing Passwall2 from the official feeds");
        let res = steps::passwall(&r, &now)?;
        let running = res.iter().any(|w| w == "RUNNING");
        s.done(&format!(
            "Passwall2 installed{}",
            if running { " and running" } else { "" }
        ));
        if !running {
            ui::info("open LuCI -> Services -> Passwall 2, add your node, enable it");
        }
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

fn dirs_cache() -> PathBuf {
    fsx::home()
        .unwrap_or_else(|_| std::env::temp_dir())
        .join(".cache/wrtforge")
}
