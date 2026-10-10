//! `--bench`: measure throughput from the router itself (download, upload, latency) plus its CPU load.
//! The transfers run on the router over SSH with its own `uclient-fetch`, against Cloudflare's public
//! speed endpoints, so nothing is installed and the laptop's network is irrelevant.
use crate::{owrt::Device, ssh::Router, ui};
use anyhow::{Context, Result};
use std::io::Write;
use std::path::Path;

const URL: &str = "https://speed.cloudflare.com/__";
const STREAMS: u32 = 4;

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

/// Router-side parallel transfer for `secs`; prints Mbps counted over completed requests.
fn transfer(r: &Router, secs: u64, up: bool) -> Result<f64> {
    let (prep, work) = if up {
        (
            "head -c 4000000 /dev/zero > /tmp/wf.up",
            format!(
                "uclient-fetch -q -O /dev/null --post-file=/tmp/wf.up \"{URL}up\" 2>/dev/null || break; n=$((n+4000000))"
            ),
        )
    } else {
        (
            ":",
            format!(
                "b=$(uclient-fetch -q -O - \"{URL}down?bytes=5000000\" 2>/dev/null | wc -c); [ \"$b\" -gt 0 ] || break; n=$((n+b))"
            ),
        )
    };
    let script = format!(
        r#"{prep}
now() {{ cut -d' ' -f1 /proc/uptime; }}
t0=$(now); end=$(awk -v t=$t0 -v s={secs} 'BEGIN{{print t+s}}')
rm -f /tmp/wf.n.*
for i in $(seq {STREAMS}); do (
  n=0
  while awk -v a="$(now)" -v e=$end 'BEGIN{{exit !(a<e)}}'; do {work}; done
  echo $n > /tmp/wf.n.$i
) & done
wait
t1=$(now)
cat /tmp/wf.n.* | awk -v a=$t0 -v b=$t1 '{{s+=$1}} END{{printf "%.1f\n", s*8/(b-a)/1e6}}'
rm -f /tmp/wf.n.* /tmp/wf.up"#
    );
    r.sh(&script)?
        .trim()
        .parse()
        .context("bad speed result from the router")
}

/// Median time (ms) of a tiny HTTPS request made from the router.
fn latency_ms(r: &Router) -> Result<f64> {
    r.sh(&format!(
        r#"now() {{ cut -d' ' -f1 /proc/uptime; }}
for i in 1 2 3 4 5 6 7 8; do a=$(now); uclient-fetch -q -O /dev/null "{URL}down?bytes=0" || exit 1; b=$(now)
awk -v a=$a -v b=$b 'BEGIN{{print (b-a)*1000}}'; done | tail -n 7 | sort -n | sed -n 4p"#
    ))
    .context("the router cannot reach the speed test server")?
    .trim()
    .parse()
    .context("bad latency from the router")
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
        "{} on OpenWrt {}, active tunnel: {pw}.",
        d.model, d.release
    ));
    let label = label.map_or_else(
        || format!("{}-{pw}", d.board.replace(',', "_")),
        String::from,
    );
    let (mut dl, mut ul, mut cd, mut cu) = (vec![], vec![], vec![], vec![]);
    let ping = latency_ms(r)?;
    for i in 1..=rounds {
        let s = ui::Step::new(&format!("Round {i}/{rounds}: download {secs}s"));
        let a = stat(r)?;
        let x = transfer(r, secs, false)?;
        let b = stat(r)?;
        s.done(&format!("{x:.0} Mbps"));
        let s = ui::Step::new(&format!("Round {i}/{rounds}: upload {secs}s"));
        let y = transfer(r, secs, true)?;
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
        "\n| label | router | OpenWrt | passwall | down Mbps | up Mbps | http ms | cpu% down | cpu% up |\n|---|---|---|---|---|---|---|---|---|\n| {label} | {} | {} | {pw} | {dl:.0} | {ul:.0} | {ping:.0} | {cd:.0} | {cu:.0} |\n",
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
            "label,router,openwrt,passwall,down_mbps,up_mbps,http_ms,cpu_down,cpu_up"
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
