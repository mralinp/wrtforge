<h1 align="center">▌wrtforge</h1>

<p align="center"><b>Paste one command. Get a clean, official OpenWrt, Passwall2 and a recovery slot.</b><br>
No install, no dependencies. It talks to your router over SSH.</p>

---

## Quick start

On your laptop (macOS or Linux, Python 3 is already there):

```bash
python3 <(curl -fsSL https://raw.githubusercontent.com/mralinp/wrtforge/main/wrtforge.py)
```

That is it. It asks for the router address and how to log in, shows you the plan, and waits for your OK before touching anything.

Want to look first? Add `--dry-run` and nothing is changed:

```bash
python3 <(curl -fsSL https://raw.githubusercontent.com/mralinp/wrtforge/main/wrtforge.py) 192.168.1.1 --dry-run
```

<sub>Prefer reading before running (you should): open [wrtforge.py](wrtforge.py), it is one file, standard library only. `curl -fsSL <url> | python3 -` works too.</sub>

## What you get

| Step | What happens |
|---|---|
| **Inspect** | Detects your exact board and OpenWrt target over SSH. |
| **Flash** | Downloads the latest stable official image for that board, verifies its sha256, runs `sysupgrade`. |
| **Keep** | Carries over only what you choose: SSH keys, root password, optionally LAN/Wi-Fi/DHCP and your Passwall2 config. |
| **Recovery slot** | On dual-slot devices, writes the same official image to the second slot and verifies it by hash. |
| **Passwall2** | Installs it from the official feeds (`apk` on 25.12+, `opkg` before), with `dnsmasq-full` and the dependencies it forgets (xray-core, tproxy modules…). |

```
  ▌wrtforge  v0.1.0

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

## Logging in

| Choice | Behaviour |
|---|---|
| **key** | Uses a key the router already trusts. |
| **password** | `ssh` asks for the password itself. wrtforge never sees or stores it. |
| **copy-key** | Type the password once; wrtforge installs your public key so the rest runs without prompts. |

## Options

```
wrtforge.py [host] [-u USER] [-p PORT] [--auth key|password|copy-key]
            [--version 25.12.5] [--no-passwall] [--no-fallback]
            [--dry-run] [-y]
```

`-y` accepts every default, handy for scripts. Pass the address and `--auth` with it.

## Needs

- The router already runs OpenWrt (any build, rebrands such as PeDitXOS included) and can reach the internet.
- `python3` (3.8+) and `ssh` on your laptop.

## Good to know

- Your Passwall2 nodes are not in this repo. They only move from the router's own `/etc/config/passwall2` to the new firmware, and only if you say yes.
- Expect the router to be offline for about 5 to 15 minutes. The old settings are not kept except what you chose.
- Host keys are trusted on first contact each run, which is fine on a LAN you control.
- Image selection and Passwall2 are generic. The recovery-slot writer knows the Linksys MX layout (`alt_kernel` / `alt_rootfs`) and is skipped on other devices.
- Built from a manual migration of a Linksys MX5300 (PeDitXOS 24.10 to OpenWrt 25.12.5). Parts were re-checked on that router, but a full one-shot run is not yet verified. Run `--dry-run` first. A flash can brick a device; use at your own risk.

<details>
<summary>Lessons baked in (why it does things this way)</summary>

- The keep-bundle is built **on the router**. Built on macOS, the files carry your uid and dropbear rejects `authorized_keys`, locking you out.
- Passwall2's dependencies go in **before** the app. A restored, enabled config starts at once, and with xray missing its half-loaded nft rules cut your internet.
- The legacy values `tcp_no_redir_ports 'disable'` and `udp_no_redir_ports 'disable'` break the new nft rules, so they are removed.
- Every `sysupgrade` swaps slots and drops installed packages. Settings stay, packages do not.
- On the MX5300 the 150 MB `syscfg` partition is read-only in the stock device tree, so it cannot be used as storage without a custom kernel.

</details>

## Develop

```bash
python3 -m unittest discover -s tests
```

MIT licensed.
