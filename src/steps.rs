//! The router-changing steps: backup, keep-bundle, flash, recovery slot, Passwall2.
use crate::fsx;
use crate::owrt::{self, Device};
use crate::ssh::Router;
use anyhow::{bail, Context, Result};
use std::collections::HashMap;
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::path::Path;
use std::time::{Duration, Instant};
use std::{fs, thread};

fn save(dir: &Path, name: &str, data: &[u8]) -> Result<()> {
    Ok(fsx::write_private(&dir.join(name), data)?)
}

/// Config, package list, and the small raw flash partitions (bootloader, env, Wi-Fi calibration), each verified by hash.
/// Returns how many flash partitions were saved.
pub fn backup(r: &Router, dir: &Path) -> Result<usize> {
    fsx::private_dir(dir)?;
    r.sh("sysupgrade -b /tmp/wf-backup.tgz >/dev/null 2>&1")?;
    let cfg = r.get("/tmp/wf-backup.tgz")?;
    r.run("rm -f /tmp/wf-backup.tgz", None)?;
    save(dir, "config-backup.tar.gz", &cfg)?;

    let info = r.run(
        "cat /etc/openwrt_release /etc/board.json /proc/mtd /proc/partitions 2>/dev/null; echo '--- packages'; (apk list -I 2>/dev/null || opkg list-installed 2>/dev/null); echo '--- u-boot env'; fw_printenv 2>/dev/null",
        None,
    )?;
    save(dir, "device-info.txt", &info.stdout)?;

    let mtds = r
        .run(
            r#"awk 'NR>1{gsub(/[:"]/,""); print substr($1,4), $2, $4}' /proc/mtd"#,
            None,
        )?
        .text();
    let small: Vec<(String, String)> = mtds
        .lines()
        .filter_map(|l| {
            let p: Vec<&str> = l.split_whitespace().collect();
            let size = u64::from_str_radix(p.get(1)?, 16).ok()?;
            (p.len() == 3 && size <= 4 << 20).then(|| (p[0].to_string(), p[2].to_string()))
        })
        .collect();
    if small.is_empty() {
        // eMMC device: keep the partition table and boot area
        if r.run("[ -e /dev/mmcblk0 ]", None)?.ok {
            save(
                dir,
                "mmcblk0-first-1MiB.bin",
                &r.check("dd if=/dev/mmcblk0 bs=1M count=1 2>/dev/null", None)?
                    .stdout,
            )?;
        }
        return Ok(0);
    }
    let ids: Vec<&str> = small.iter().map(|(i, _)| i.as_str()).collect();
    let want: HashMap<String, String> = r
        .sh(&format!(
            "for n in {}; do echo $n $(cat /dev/mtd$n | sha256sum | cut -d' ' -f1); done",
            ids.join(" ")
        ))?
        .lines()
        .filter_map(|l| {
            l.split_once(' ')
                .map(|(a, b)| (a.to_string(), b.to_string()))
        })
        .collect();
    for (id, name) in &small {
        let blob = r.get(&format!("/dev/mtd{id}"))?;
        if want.get(id) != Some(&owrt::sha256_hex(&blob)) {
            bail!("backup of flash partition {name} did not verify");
        }
        save(
            dir,
            &format!("mtd{id}-{}.bin", name.replace(':', "_")),
            &blob,
        )?;
    }
    Ok(small.len())
}

/// Built on the router, so files keep root ownership (dropbear rejects anything else).
pub fn build_keep(r: &Router, carry_net: bool, carry_pw: bool) -> Result<()> {
    let mut paths = vec!["etc/dropbear", "etc/passwd", "etc/shadow", "etc/group"];
    if carry_net {
        paths.extend([
            "etc/config/network",
            "etc/config/wireless",
            "etc/config/dhcp",
            "etc/config/system",
            "etc/config/dropbear",
        ]);
    }
    if carry_pw {
        paths.push("etc/config/passwall2");
    }
    // custom rule files (e.g. iran.dat) live next to the stock geoip/geosite
    let extra = if carry_pw {
        r"ls usr/share/v2ray/*.dat 2>/dev/null | grep -vE '/geo(ip|site)\.dat$' || true"
    } else {
        "true"
    };
    r.sh(&format!("cd /; x=$({extra}); tar czf /tmp/wrtforge-keep.tgz $(for f in {} $x; do [ -e $f ] && echo $f; done)", paths.join(" ")))?;
    Ok(())
}

pub fn flash(r: &Router, image: &[u8]) -> Result<()> {
    r.put(image, "/tmp/wrtforge-fw.bin")?;
    r.sh("sysupgrade -T -f /tmp/wrtforge-keep.tgz /tmp/wrtforge-fw.bin")?;
    // detached: the ssh session dies with the reboot, so ignore how it ends
    let _ = r.run("nohup sh -c 'sleep 2; sysupgrade -f /tmp/wrtforge-keep.tgz /tmp/wrtforge-fw.bin' >/tmp/wrtforge-up.log 2>&1 </dev/null &", None);
    Ok(())
}

fn port_open(host: &str, port: u16) -> bool {
    (host, port)
        .to_socket_addrs()
        .ok()
        .and_then(|mut a| a.next())
        .is_some_and(|a: SocketAddr| {
            TcpStream::connect_timeout(&a, Duration::from_millis(1500)).is_ok()
        })
}

pub fn wait_reboot(host: &str, port: u16) -> Result<()> {
    let end = Instant::now() + Duration::from_secs(120);
    while port_open(host, port) && Instant::now() < end {
        thread::sleep(Duration::from_secs(2)); // wait until it actually goes down
    }
    let end = Instant::now() + Duration::from_secs(600);
    while !port_open(host, port) {
        if Instant::now() > end {
            bail!("router did not come back within 10 minutes");
        }
        thread::sleep(Duration::from_secs(3));
    }
    thread::sleep(Duration::from_secs(6));
    Ok(())
}

/// Write the official image into the slot we are NOT running from, verify by hash. Returns the kernel partition name.
pub fn fallback(r: &Router, image_path: &Path) -> Result<String> {
    let (mut kernel, mut root) = (vec![], vec![]);
    for e in tar::Archive::new(fs::File::open(image_path)?).entries()? {
        let mut e = e?;
        let name = e
            .path()?
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        match name.as_str() {
            "kernel" => std::io::Read::read_to_end(&mut e, &mut kernel).map(|_| ())?,
            "root" => std::io::Read::read_to_end(&mut e, &mut root).map(|_| ())?,
            _ => {}
        }
    }
    if kernel.is_empty() || root.is_empty() {
        bail!("the image has no kernel/root parts");
    }
    let (kern, rootp) = if r
        .check("cat /proc/cmdline", None)?
        .text()
        .contains("ubi.mtd=alt_rootfs")
    {
        ("kernel", "rootfs")
    } else {
        ("alt_kernel", "alt_rootfs")
    };
    r.put(&kernel, "/tmp/wf-kernel.bin")?;
    r.put(&root, "/tmp/wf-root.bin")?;
    let script = include_str!("../scripts/fallback.sh")
        .replace("@KERN@", kern)
        .replace("@ROOT@", rootp)
        .replace("@RSIZE@", &root.len().to_string())
        .replace("@KSIZE@", &kernel.len().to_string());
    let out = r.sh(&script)?;
    let got: Vec<&str> = out
        .split_whitespace()
        .rev()
        .take(2)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    if got != [owrt::sha256_hex(&kernel), owrt::sha256_hex(&root)] {
        bail!("fallback slot read-back does not match the image; slot {kern} left unbootable");
    }
    Ok(kern.to_string())
}

/// Installs Passwall2 from the official feeds. Returns the script's status words (RUNNING / INSTALLED).
pub fn passwall(r: &Router, d: &Device) -> Result<Vec<String>> {
    if !r
        .run(
            &format!("wget -q --spider --timeout=8 {}/apk.pub", owrt::PW),
            None,
        )?
        .ok
    {
        bail!("router cannot reach the Passwall feed (no internet from the router?)");
    }
    let fl = owrt::feeds(d.apk, &owrt::series(&d.release), &d.arch);
    let script = include_str!("../scripts/passwall.sh")
        .replace("@PM@", if d.apk { "apk" } else { "opkg" })
        .replace("@BASE@", owrt::PW)
        .replace("@FEEDS@", &fl.join(" "));
    Ok(r.sh(&script)
        .context("Passwall2 install failed")?
        .split_whitespace()
        .map(String::from)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ssh::Auth;

    /// Needs a router that trusts your SSH key: `WRTFORGE_TEST_HOST=192.168.1.1 cargo test -- --ignored`
    #[test]
    #[ignore = "needs a live router"]
    fn backup_live() {
        let host = std::env::var("WRTFORGE_TEST_HOST").expect("set WRTFORGE_TEST_HOST");
        let mut r = Router::new(&host, "root", 22, Auth::Key).unwrap();
        r.connect().unwrap();
        let dir = std::env::temp_dir().join(format!("wf-test-{}", fsx::timestamp()));
        let n = backup(&r, &dir).unwrap();
        assert!(dir.join("config-backup.tar.gz").exists() && dir.join("device-info.txt").exists());
        println!("saved {n} partitions to {}", dir.display());
        fs::remove_dir_all(dir).unwrap();
    }
}
