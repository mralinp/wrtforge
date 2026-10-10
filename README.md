# wrtforge

Forge a clean, official OpenWrt onto your router over SSH, with Passwall2 from the official feeds and a recovery slot.

```
  ▌wrtforge  v0.1.0
  clean official OpenWrt + Passwall2, over SSH

  Inspect
  ✔ Linksys MX5300 (linksys,mx5300)
  ✔ OpenWrt 25.12.5  openwrt-…-sysupgrade.bin, 13.8 MB
  Flash
  ✔ Downloaded and verified sha256 1d8895048d5a…
  ✔ Router is back
  Fallback slot
  ✔ Fallback slot ready (alt_kernel, verified by hash)
  Passwall2
  ✔ Passwall2 installed and running
```

## Use

```bash
git clone https://github.com/mralinp/wrtforge && cd wrtforge
./wrtforge.py                 # asks for address and login method
./wrtforge.py 192.168.1.1 --dry-run   # inspect and plan only, changes nothing
```

Needs Python 3.8+ (stdlib only) and `ssh` on your computer. The router must already run OpenWrt (any build, including rebrands) and reach the internet.

Login methods: an authorised SSH key, a password (ssh asks for it itself; wrtforge never sees or stores it), or **copy-key**: type the password once and wrtforge installs your public key so the rest runs without prompts.

Flags: `--version 25.12.5` · `--no-passwall` · `--no-fallback` · `--dry-run` · `-y` (accept defaults).

## What it does

1. Reads the board id and target from the router and picks the matching image from `downloads.openwrt.org` (`profiles.json`), latest stable by default. Verifies its sha256.
2. Builds a small keep-bundle **on the router** (SSH keys, root password, optionally LAN/Wi-Fi/DHCP and your Passwall2 config) and runs `sysupgrade -f` with it. Nothing else of the old firmware survives.
3. Waits for the reboot, reconnects, checks the new version.
4. **Recovery slot**: on dual-slot Linksys MX devices, writes the same official image to the inactive slot and verifies it by hash. The bootloader falls back to it by itself if the main slot fails.
5. Installs Passwall2 from the official feeds, using `apk` (25.12+) or `opkg` (older), swaps `dnsmasq` for `dnsmasq-full`, and installs the dependencies Passwall2 does not pull in by itself (xray-core, tproxy kernel modules, …).

Your proxy nodes are never in this repo. They are only carried over from the router's own `/etc/config/passwall2` if you say yes.

## Lessons baked in

- Build the keep-bundle on the router, never on macOS: files owned by your uid make dropbear reject `authorized_keys` and you lock yourself out.
- Install Passwall2's dependencies *before* the app. A restored, enabled config starts immediately, and with xray missing its half-loaded nft rules cut your internet.
- `tcp_no_redir_ports 'disable'` / `udp_no_redir_ports 'disable'` (legacy values) break the new nft rules; wrtforge removes them.
- Sysupgrade swaps slots each time and drops installed packages. Settings are kept, packages are not.
- On the MX5300 the 150 MB `syscfg` partition is read-only in the stock device tree, so it cannot be reused as storage without a custom kernel.

## Limits

- Every step was first done by hand on a Linksys MX5300 (qualcommax/ipq807x), PeDitXOS 24.10 to OpenWrt 25.12.5, then scripted. The script's inspect/plan, keep-bundle and remote scripts were checked on that router, but a full one-shot run is not yet verified, so use `--dry-run` first. Image selection and Passwall2 are generic; the fallback-slot writer only knows the Linksys MX `alt_kernel`/`alt_rootfs` layout and is skipped elsewhere.
- Host keys are trusted on first contact for each run (fine on a LAN you control).
- A flash can brick a device. Read the plan, keep a recovery path, use at your own risk.

## Test

```bash
python3 -m unittest discover -s tests
```

MIT licensed.
