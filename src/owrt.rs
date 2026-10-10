//! OpenWrt metadata (versions, images, feeds), downloads, and reading the router's identity.
use crate::ssh::Router;
use anyhow::{bail, Context, Result};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{Read, Write};
use std::path::Path;

pub const DL: &str = "https://downloads.openwrt.org";
pub const PW: &str = "https://master.dl.sourceforge.net/project/openwrt-passwall-build";

pub struct Image {
    pub name: String,
    pub sha256: String,
    pub size: u64,
}

pub struct Device {
    pub distro: String,
    pub release: String,
    pub target: String,
    pub arch: String,
    pub board: String,
    pub model: String,
    pub mem: u64,
    pub sysupgrade: bool,
    pub dual: bool,
    pub apk: bool,
    pub had_pw2: bool,
}

pub fn series(release: &str) -> String {
    release.split('.').take(2).collect::<Vec<_>>().join(".")
}

/// The image of this kind ("sysupgrade", "factory") for a board id like `linksys,mx5300`.
pub fn pick_image(profiles: &Value, board: &str, kind: &str) -> Option<Image> {
    let key_guess = board.replace(',', "_");
    for (key, p) in profiles["profiles"].as_object()? {
        let supported = p["supported_devices"]
            .as_array()
            .is_some_and(|a| a.iter().any(|d| d == board));
        if !supported && *key != key_guess {
            continue;
        }
        let img = p["images"]
            .as_array()?
            .iter()
            .filter(|i| i["type"] == kind)
            .min_by_key(|i| i["filesystem"] != "squashfs");
        if let Some(i) = img {
            return Some(Image {
                name: i["name"].as_str()?.to_string(),
                sha256: i["sha256"].as_str()?.to_string(),
                size: i["size"].as_u64().unwrap_or(0),
            });
        }
    }
    None
}

pub fn feeds(apk: bool, series: &str, arch: &str) -> Vec<String> {
    let base = format!("{PW}/releases/packages-{series}/{arch}");
    ["passwall_packages", "passwall2"]
        .iter()
        .map(|n| format!("{base}/{n}{}", if apk { "/packages.adb" } else { "" }))
        .collect()
}

pub fn fetch_json(url: &str) -> Result<Value> {
    serde_json::from_reader(
        ureq::get(url)
            .call()
            .with_context(|| format!("GET {url}"))?
            .into_reader(),
    )
    .context("invalid JSON")
}

pub fn sha256_hex(data: &[u8]) -> String {
    format!("{:x}", Sha256::digest(data))
}

pub fn download(url: &str, dest: &Path, sha: &str) -> Result<()> {
    if fs::read(dest).is_ok_and(|b| sha256_hex(&b) == sha) {
        return Ok(());
    }
    fs::create_dir_all(dest.parent().unwrap())?;
    let resp = ureq::get(url)
        .call()
        .with_context(|| format!("GET {url}"))?;
    let total: u64 = resp
        .header("Content-Length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let pb = indicatif::ProgressBar::new(total);
    pb.set_style(
        indicatif::ProgressStyle::with_template(
            "  ↓ {bar:30.cyan/blue} {percent:>3}%  {bytes}/{total_bytes}",
        )
        .unwrap()
        .progress_chars("█░ "),
    );
    let (mut r, mut buf, mut h) = (resp.into_reader(), Vec::new(), Sha256::new());
    let mut chunk = [0u8; 1 << 16];
    loop {
        let n = r.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        h.update(&chunk[..n]);
        buf.extend_from_slice(&chunk[..n]);
        pb.inc(n as u64);
    }
    pb.finish_and_clear();
    if format!("{:x}", h.finalize()) != sha {
        bail!(
            "checksum mismatch for {}",
            dest.file_name().unwrap().to_string_lossy()
        );
    }
    let part = dest.with_extension("part");
    fs::File::create(&part)?.write_all(&buf)?;
    fs::rename(part, dest)?;
    Ok(())
}

pub fn inspect(r: &Router) -> Result<Device> {
    let out = r.sh(include_str!("../scripts/inspect.sh"))?;
    let l: Vec<&str> = out.lines().collect();
    if l.first() == Some(&"NOTOPENWRT") {
        bail!("this router is not running OpenWrt; flash the first OpenWrt image with the vendor tools first");
    }
    let id: Vec<&str> = l.first().context("empty reply")?.split('|').collect();
    if id.len() != 4 || l.len() < 4 {
        bail!("could not read the router's identity");
    }
    Ok(Device {
        distro: id[0].into(),
        release: id[1].into(),
        target: id[2].into(),
        arch: id[3].into(),
        board: l[1].into(),
        model: l[2].into(),
        mem: l[3].parse::<u64>().unwrap_or(0) * 1024,
        sysupgrade: l.contains(&"HAS_SYSUPGRADE"),
        dual: l.contains(&"DUAL"),
        apk: l.contains(&"PM_APK"),
        had_pw2: l.contains(&"HAS_PW2"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn profiles() -> Value {
        json!({"profiles": {"linksys_mx5300": {"supported_devices": ["linksys,mx5300"], "images": [
            {"type": "initramfs", "name": "i"},
            {"type": "sysupgrade", "filesystem": "squashfs", "name": "s.bin", "sha256": "x", "size": 1},
            {"type": "factory", "filesystem": "squashfs", "name": "f.bin", "sha256": "y", "size": 2}]}}})
    }

    #[test]
    fn picks_images() {
        assert_eq!(
            pick_image(&profiles(), "linksys,mx5300", "sysupgrade")
                .unwrap()
                .name,
            "s.bin"
        );
        assert_eq!(
            pick_image(&profiles(), "linksys,mx5300", "factory")
                .unwrap()
                .name,
            "f.bin"
        );
        assert!(pick_image(&profiles(), "acme,nope", "sysupgrade").is_none());
    }

    #[test]
    fn series_and_feeds() {
        assert_eq!(series("25.12.5"), "25.12");
        assert!(feeds(true, "25.12", "aarch64_cortex-a53")[1]
            .ends_with("/packages-25.12/aarch64_cortex-a53/passwall2/packages.adb"));
        assert!(!feeds(false, "24.10", "x")[0].ends_with(".adb"));
    }

    #[test]
    fn hashes() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
