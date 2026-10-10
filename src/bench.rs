//! `--bench`: measure throughput through the router from this machine, plus the router's CPU load.
//! Uses Cloudflare's public speed endpoints so there is no external speedtest binary to install.
use crate::{owrt::Device, ssh::Router, ui};
use anyhow::{Context, Result};
use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::time::{Duration, Instant};

const DOWN: &str = "https://speed.cloudflare.com/__down?bytes=";
const UP: &str = "https://speed.cloudflare.com/__up";
const STREAMS: usize = 4;

/// Busy fraction (0..100) between two `/proc/stat` "cpu" lines.
fn cpu_busy(a: &str, b: &str) -> f64 {
    // user nice system idle iowait irq softirq steal
    let f = |s: &str| -> (f64, f64) {
        let v: Vec<f64> = s
            .split_whitespace()
            .skip(1)
            .take(8)
            .filter_map(|x| x.parse().ok())
            .collect();
        (
            v.iter().sum(),
            v.get(3).unwrap_or(&0.0) + v.get(4).unwrap_or(&0.0),
        )
    };
    let ((t0, i0), (t1, i1)) = (f(a), f(b));
    if t1 <= t0 {
        return 0.0;
    }
    100.0 * (1.0 - (i1 - i0) / (t1 - t0))
}

fn median(v: &mut [f64]) -> f64 {
    v.sort_by(|a, b| a.total_cmp(b));
    v[v.len() / 2]
}

fn stat(r: &Router) -> Result<String> {
    r.sh("head -n1 /proc/stat")
}

/// Run `STREAMS` parallel transfers for `secs`; returns Mbps counted over completed requests.
fn transfer(secs: u64, up: bool) -> f64 {
    let bytes = AtomicU64::new(0);
    let start = Instant::now();
    let end = start + Duration::from_secs(secs);
    std::thread::scope(|s| {
        for _ in 0..STREAMS {
            s.spawn(|| {
                let agent = ureq::AgentBuilder::new()
                    .timeout(Duration::from_secs(30))
                    .build();
                let body = vec![0u8; 4_000_000];
                let mut sink = [0u8; 65536];
                while Instant::now() < end {
                    let n = if up {
                        agent.post(UP).send_bytes(&body).map(|_| body.len() as u64)
                    } else {
                        agent.get(&format!("{DOWN}25000000")).call().map(|r| {
                            let mut rd = r.into_reader();
                            let mut n = 0u64;
                            while let Ok(k) = rd.read(&mut sink) {
                                if k == 0 {
                                    break;
                                }
                                n += k as u64;
                            }
                            n
                        })
                    };
                    match n {
                        Ok(n) => {
                            bytes.fetch_add(n, Relaxed);
                        }
                        Err(_) => break,
                    }
                }
            });
        }
    });
    bytes.load(Relaxed) as f64 * 8.0 / start.elapsed().as_secs_f64() / 1e6
}

fn latency_ms() -> Result<f64> {
    let agent = ureq::agent();
    let url = format!("{DOWN}0");
    agent
        .get(&url)
        .call()
        .context("cannot reach the speed test server")?; // warm the connection
    let mut v = vec![];
    for _ in 0..7 {
        let t = Instant::now();
        agent.get(&url).call()?;
        v.push(t.elapsed().as_secs_f64() * 1e3);
    }
    Ok(median(&mut v))
}

pub fn run(
    r: &Router,
    d: &Device,
    label: Option<&str>,
    rounds: u32,
    secs: u64,
    out: &Path,
) -> Result<()> {
    let pw = r
        .sh("m=''; pgrep xray >/dev/null || pgrep sing-box >/dev/null && m=passwall2\n[ -n \"$(wg show interfaces 2>/dev/null)\" ] && m=\"${m:+$m+}wireguard\"\npgrep openvpn >/dev/null && m=\"${m:+$m+}openvpn\"\necho \"${m:-none}\"")?
        .trim()
        .to_string();
    ui::info(&format!(
        "{} on OpenWrt {}, active tunnel: {pw}. This machine must reach the internet THROUGH this router.",
        d.model, d.release
    ));
    let label = label.map_or_else(
        || format!("{}-{pw}", d.board.replace(',', "_")),
        String::from,
    );
    let (mut dl, mut ul, mut cd, mut cu) = (vec![], vec![], vec![], vec![]);
    let ping = latency_ms()?;
    for i in 1..=rounds {
        let s = ui::Step::new(&format!("Round {i}/{rounds}: download {secs}s"));
        let a = stat(r)?;
        let x = transfer(secs, false);
        let b = stat(r)?;
        s.done(&format!("{x:.0} Mbps"));
        let s = ui::Step::new(&format!("Round {i}/{rounds}: upload {secs}s"));
        let y = transfer(secs, true);
        let c = stat(r)?;
        s.done(&format!("{y:.0} Mbps"));
        dl.push(x);
        ul.push(y);
        cd.push(cpu_busy(&a, &b));
        cu.push(cpu_busy(&b, &c));
    }
    let (dl, ul, cd, cu) = (
        median(&mut dl),
        median(&mut ul),
        median(&mut cd),
        median(&mut cu),
    );
    println!(
        "\n| label | router | OpenWrt | passwall | down Mbps | up Mbps | ping ms | cpu% down | cpu% up |\n|---|---|---|---|---|---|---|---|---|\n| {label} | {} | {} | {pw} | {dl:.0} | {ul:.0} | {ping:.0} | {cd:.0} | {cu:.0} |\n",
        d.model, d.release
    );
    let new = !out.exists();
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(out)?;
    if new {
        writeln!(
            f,
            "label,router,openwrt,passwall,down_mbps,up_mbps,ping_ms,cpu_down,cpu_up"
        )?;
    }
    writeln!(
        f,
        "{label},{},{},{pw},{dl:.0},{ul:.0},{ping:.0},{cd:.0},{cu:.0}",
        d.model, d.release
    )?;
    ui::ok(&format!("appended to {}", out.display()));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn busy_and_median() {
        let a = "cpu  100 0 100 800 0 0 0 0";
        let b = "cpu  200 0 200 1000 0 0 0 0"; // +200 busy, +200 idle
        assert!((cpu_busy(a, b) - 50.0).abs() < 0.1);
        assert_eq!(cpu_busy(a, a), 0.0);
        assert_eq!(median(&mut [3.0, 1.0, 2.0]), 2.0);
    }
}
