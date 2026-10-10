<h1 align="center">▌wrtforge</h1>

<p align="center"><b>Paste one command. Get a clean, official OpenWrt, Passwall2 and a recovery slot.</b><br>
A single static binary. No Python, no install. It talks to your router over SSH.</p>

---

## Quick start

On your laptop (macOS or Linux):

```bash
curl -fsSL https://raw.githubusercontent.com/mralinp/wrtforge/main/get.sh | sh
```

That downloads the latest release for your machine, checks its sha256, and runs it from a temporary folder. Nothing is installed. It scans your networks, lists the OpenWrt routers it finds and asks which one, asks how to log in, shows you the plan, and waits for your OK before touching anything.

Want to look first? Nothing is changed with `--dry-run`:

```bash
curl -fsSL https://raw.githubusercontent.com/mralinp/wrtforge/main/get.sh | sh -s -- 192.168.1.1 --dry-run
```

<sub>Rather not pipe to `sh`? Grab a binary from the [Releases](https://github.com/mralinp/wrtforge/releases) page (linux and macOS, x86_64 and arm64, each with a `.sha256`), or build it: `cargo install --git https://github.com/mralinp/wrtforge`. Read [get.sh](get.sh) first, it is short.</sub>

## Stay online while it works

Connect your laptop to the router with a **LAN cable**, and keep **Wi-Fi (or any other connection) as your main internet**. The router can then reboot as often as it likes and you never lose your connection. wrtforge checks this first and warns you if your internet currently runs through the router being flashed.

## What you get

| Step | What happens |
|---|---|
| **Discovery** | Scans your local networks for OpenWrt routers (LuCI page or dropbear SSH) and lets you pick one. Or pass the address, or `--no-scan` to type it. |
| **Network check** | Tells you whether the flash would cut your own internet, and how to avoid it. |
| **Inspect** | Detects your exact board and OpenWrt target over SSH. |
| **Backup & recovery** | *Before* anything is flashed: saves your config, package list, and the bootloader and Wi-Fi calibration partitions to `~/wrtforge-backups/` (every partition verified by hash). Dual-slot devices keep the old firmware as the recovery path; single-slot devices also get the official factory image saved for USB/TFTP recovery. |
| **Flash** | Downloads the latest stable official image for that board, verifies its sha256, runs `sysupgrade`. |
| **Keep** | Carries over only what you choose: SSH keys, root password, optionally LAN/Wi-Fi/DHCP and your Passwall2 config. |
| **Passwall2** | Installs it from the official feeds (`apk` on 25.12+, `opkg` before), with `dnsmasq-full` and the dependencies it forgets (xray-core, tproxy modules…). |
| **Recovery slot** | Once the new system works, replaces the old firmware in the second slot with the official image, verified by hash. |

```
  ▌wrtforge  v0.1.0

  ✔ Found 2 OpenWrt routers
  ? Which router should I set up?
  ❯ 192.168.1.1  OpenWrt (LuCI)  ← your gateway
    192.168.1.2  likely OpenWrt (dropbear SSH)
    Enter an address manually

  Network
  ✔ two networks: router via en5, internet via en0. You will stay online.

  Inspect
  ✔ Linksys MX5300 (linksys,mx5300)
  ✔ OpenWrt 25.12.5  openwrt-…-sysupgrade.bin, 13.8 MB

  Backup & recovery
  ✔ Backup verified (~/wrtforge-backups/linksys_mx5300-…, 23 flash partitions)
  ✔ Downloaded and verified sha256 1d8895048d5a…

  Flash
  ✔ Image accepted by sysupgrade; flashing and rebooting
  ✔ Router is back

  Passwall2
  ✔ Passwall2 installed and running

  Recovery slot
  ✔ Fallback slot ready (alt_kernel, verified by hash)
```

## Logging in

| Choice | Behaviour |
|---|---|
| **key** | Uses a key the router already trusts. |
| **password** | `ssh` asks for the password itself. wrtforge never sees or stores it. |
| **copy-key** | Type the password once; wrtforge installs your public key so the rest runs without prompts. |

## Options

```
wrtforge [HOST] [--no-scan] [-u USER] [-p PORT] [--auth key|password|copy-key]
         [--release 25.12.5] [--no-passwall] [--no-fallback]
         [--backup-dir DIR] [--dry-run] [-y [--go]]
```

`-y` accepts every default, handy for scripts. Pass the address and `--auth` with it. The final "flash now?" still needs a real yes, so scripts add `--go`.

## Backups

Each run writes `~/wrtforge-backups/<board>-<time>/` with `config-backup.tar.gz`, `device-info.txt` (release, partition table, package list) and raw copies of the small flash partitions (bootloader, U-Boot env, Wi-Fi calibration `art`). That calibration data cannot be recreated if lost. **The folder contains your Wi-Fi passwords and keys**, so it is created private (`700`); keep it that way.

## Google Wifi (Gale) and other single-slot devices

wrtforge works on a router that already runs OpenWrt. The Google Wifi ships with ChromeOS-based firmware, so its **first** OpenWrt install is a one-time USB-boot procedure from the [OpenWrt Google Wifi page](https://openwrt.org/toh/google/wifi). After that, wrtforge handles upgrades and Passwall2. It has a single eMMC firmware slot, so there is no second slot to fall back on: the recovery path is the backup plus the official factory image, which wrtforge saves next to it for re-doing the USB recovery. Not yet tested on Gale hardware.

## Needs

- A router that already runs OpenWrt (any build, rebrands such as PeDitXOS included) and can reach the internet.
- `ssh` on your laptop. macOS or Linux (Windows' OpenSSH has no connection sharing, so it is not supported).

## Good to know

- Your Passwall2 nodes are not in this repo. They only move from the router's own `/etc/config/passwall2` to the new firmware, and only if you say yes.
- Expect the router to be offline for about 5 to 15 minutes. The old settings are not kept except what you chose.
- Host keys are trusted on first contact each run, which is fine on a LAN you control.
- Image selection, backup and Passwall2 are generic. The second-slot writer knows the Linksys MX layout (`alt_kernel` / `alt_rootfs`) and is skipped on other devices.
- Built from a manual migration of a Linksys MX5300 (PeDitXOS 24.10 to OpenWrt 25.12.5). Parts were re-checked on that router, but a full one-shot run is not yet verified. Run `--dry-run` first. A flash can brick a device; use at your own risk.

<details>
<summary>Lessons baked in (why it does things this way)</summary>

- The keep-bundle is built **on the router**. Built on macOS, the files carry your uid and dropbear rejects `authorized_keys`, locking you out.
- Passwall2's dependencies go in **before** the app. A restored, enabled config starts at once, and with xray missing its half-loaded nft rules cut your internet.
- The legacy values `tcp_no_redir_ports 'disable'` and `udp_no_redir_ports 'disable'` break the new nft rules, so they are removed.
- Every `sysupgrade` swaps slots and drops installed packages. Settings stay, packages do not.
- On the MX5300 the 150 MB `syscfg` partition is read-only in the stock device tree, so it cannot be used as storage without a custom kernel.

</details>

## Contributing

Every task is an issue (with labels), a branch off `main`, and a PR into `main`; merging to `main` bumps the version and publishes a release with a changelog from the PR titles. The rules are in [CLAUDE.md](CLAUDE.md).

```bash
cargo fmt --all && cargo clippy --all-targets -- -D warnings && cargo test
WRTFORGE_TEST_HOST=192.168.1.1 cargo test -- --ignored   # live backup test, needs a router
```

MIT licensed.
