#!/usr/bin/env python3
"""wrtforge - forge a clean, official OpenWrt (+ Passwall2) onto your router over SSH.

Stdlib only, Python 3.8+. Credentials are never stored: SSH asks for the
password itself, or you let wrtforge install your public key.
"""
import argparse
import hashlib
import json
import os
import shutil
import socket
import subprocess
import sys
import tarfile
import tempfile
import threading
import time
import urllib.request

VERSION = "0.1.0"
DL = "https://downloads.openwrt.org"
PW = "https://master.dl.sourceforge.net/project/openwrt-passwall-build"
CACHE = os.path.expanduser("~/.cache/wrtforge")
TTY = sys.stdout.isatty()
ARGS = argparse.Namespace(yes=False)


# ───────────────────────────── terminal ui ─────────────────────────────
def _c(code, s):
    return "\033[%sm%s\033[0m" % (code, s) if TTY else s


def bold(s): return _c("1", s)
def dim(s): return _c("2", s)
def red(s): return _c("31", s)
def green(s): return _c("32", s)
def yellow(s): return _c("33", s)
def cyan(s): return _c("36", s)
def mag(s): return _c("35;1", s)


class Fail(Exception):
    pass


class Step:
    """Spinner line: `with Step("doing x") as s: ...; s.done("did x")`."""
    FRAMES = "⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏"

    def __init__(self, msg):
        self.msg, self.final, self.mark = msg, None, None
        self._stop = threading.Event()

    def _spin(self):
        i = 0
        while not self._stop.wait(0.08):
            sys.stdout.write("\r\033[K  %s %s" % (cyan(self.FRAMES[i % 10]), self.msg))
            sys.stdout.flush()
            i += 1

    def __enter__(self):
        if TTY:
            self._t = threading.Thread(target=self._spin, daemon=True)
            self._t.start()
        else:
            print("  … " + self.msg)
        return self

    def update(self, msg):
        self.msg = msg

    def done(self, msg=None):
        self.final = msg

    def warn(self, msg):
        self.final, self.mark = msg, yellow("!")

    def __exit__(self, et, ev, tb):
        self._stop.set()
        if TTY:
            self._t.join()
            sys.stdout.write("\r\033[K")
        mark = red("✖") if et else (self.mark or green("✔"))
        print("  %s %s" % (mark, self.final or self.msg))
        return False


def banner():
    print()
    print("  " + mag("▌wrtforge") + dim("  v%s" % VERSION))
    print("  " + dim("clean official OpenWrt + Passwall2, over SSH"))
    print()


def info(msg): print("    " + dim(msg))
def warn(msg): print("  %s %s" % (yellow("!"), msg))
def head(msg): print("\n  " + bold(msg))


def ask(q, default=None):
    if ARGS.yes and default is not None:
        return default
    d = " " + dim("(%s)" % default) if default else ""
    v = input("  %s %s%s " % (cyan("?"), q, d)).strip()
    return v or default


def confirm(q, default=True):
    if ARGS.yes:
        return default
    v = input("  %s %s %s " % (cyan("?"), q, dim("[Y/n]" if default else "[y/N]"))).strip().lower()
    return default if not v else v[0] == "y"


def choose(q, opts, default=0):
    """opts: [(key, label)] -> key"""
    if ARGS.yes:
        return opts[default][0]
    print("  %s %s" % (cyan("?"), q))
    for i, (_, label) in enumerate(opts, 1):
        print("    %s %s" % (bold(str(i) + "."), label))
    while True:
        v = input("    %s " % dim("choice [%d]:" % (default + 1))).strip()
        if not v:
            return opts[default][0]
        if v.isdigit() and 1 <= int(v) <= len(opts):
            return opts[int(v) - 1][0]


# ───────────────────────────── ssh ─────────────────────────────
class Router:
    def __init__(self, host, user="root", port=22, auth="key"):
        self.host, self.user, self.port, self.auth = host, user, port, auth
        self.dir = tempfile.mkdtemp(prefix="wf-", dir="/tmp")
        self.batch = auth == "key"

    def _base(self, batch=None):
        o = ["ssh", "-p", str(self.port),
             "-o", "ControlMaster=auto", "-o", "ControlPath=%s/c" % self.dir,
             "-o", "ControlPersist=300", "-o", "StrictHostKeyChecking=accept-new",
             "-o", "UserKnownHostsFile=%s/kh" % self.dir,
             "-o", "ServerAliveInterval=5", "-o", "ConnectTimeout=8", "-o", "LogLevel=ERROR"]
        if self.batch if batch is None else batch:
            o += ["-o", "BatchMode=yes"]
        return o + ["%s@%s" % (self.user, self.host)]

    def connect(self):
        """Interactive on purpose: ssh itself asks for the password on the tty."""
        subprocess.run(self._base() + ["-o", "ControlMaster=yes", "true"], check=False,
                       stdout=subprocess.DEVNULL)
        if self.run("true", check=False).returncode != 0:
            raise Fail("could not log in to %s@%s" % (self.user, self.host))

    def run(self, cmd, stdin=None, check=True, timeout=300):
        fh = open(stdin, "rb") if isinstance(stdin, str) else None
        try:
            p = subprocess.run(self._base() + [cmd], input=stdin if isinstance(stdin, bytes) else None,
                               stdin=fh, capture_output=True, timeout=timeout)
        except subprocess.TimeoutExpired:
            raise Fail("timed out: " + cmd[:60])
        finally:
            if fh:
                fh.close()
        p.stdout = p.stdout.decode(errors="replace")
        p.stderr = p.stderr.decode(errors="replace")
        if check and p.returncode:
            tail = (p.stderr + p.stdout).strip().splitlines()[-6:]
            raise Fail("remote command failed (%s…)\n      %s" % (cmd[:40], "\n      ".join(tail)))
        return p

    def sh(self, script, timeout=600):
        return self.run("sh -s", stdin=("set -e\n" + script).encode(), timeout=timeout).stdout

    def put(self, path, remote):
        self.run("cat > " + remote, stdin=path, timeout=600)

    def close(self):
        subprocess.run(self._base(False) + ["-O", "exit"], capture_output=True)
        shutil.rmtree(self.dir, ignore_errors=True)

    def install_key(self):
        pubs = [os.path.expanduser("~/.ssh/" + n) for n in ("id_ed25519.pub", "id_ecdsa.pub", "id_rsa.pub")]
        pub = next((p for p in pubs if os.path.exists(p)), None)
        if not pub:
            subprocess.run(["ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-f",
                            os.path.expanduser("~/.ssh/id_ed25519")], check=True)
            pub = pubs[0]
        key = open(pub).read().strip()
        self.sh("mkdir -p /etc/dropbear; grep -qxF '%s' /etc/dropbear/authorized_keys 2>/dev/null || "
                "echo '%s' >> /etc/dropbear/authorized_keys; chmod 600 /etc/dropbear/authorized_keys" % (key, key))
        probe = self._base(True)[:]
        probe[probe.index("ControlPath=%s/c" % self.dir)] = "ControlPath=none"
        if subprocess.run(probe + ["true"], capture_output=True).returncode:
            raise Fail("installed the key but key login still fails")
        self.batch = True


def port_open(host, port, t=1.5):
    try:
        socket.create_connection((host, port), t).close()
        return True
    except OSError:
        return False


def wait_reboot(host, port):
    end = time.time() + 120
    while port_open(host, port) and time.time() < end:  # wait until it actually goes down
        time.sleep(2)
    end = time.time() + 600
    while not port_open(host, port):
        if time.time() > end:
            raise Fail("router did not come back within 10 minutes")
        time.sleep(3)
    time.sleep(6)


# ───────────────────────────── pure helpers (unit-tested) ─────────────────────────────
def series(release):
    return ".".join(release.split(".")[:2])


def pick_image(profiles, board):
    """Return the sysupgrade image dict for a board id like 'linksys,mx5300'."""
    for key, p in profiles["profiles"].items():
        if board in p.get("supported_devices", []) or key == board.replace(",", "_"):
            imgs = [i for i in p["images"] if i["type"] == "sysupgrade"]
            imgs.sort(key=lambda i: i.get("filesystem") != "squashfs")
            if imgs:
                return imgs[0]
    return None


def feeds(pm, ser, arch):
    base = "%s/releases/packages-%s/%s" % (PW, ser, arch)
    return ["%s/%s%s" % (base, n, "/packages.adb" if pm == "apk" else "") for n in ("passwall_packages", "passwall2")]


# ───────────────────────────── steps ─────────────────────────────
def fetch_json(url):
    with urllib.request.urlopen(url, timeout=30) as r:
        return json.load(r)


def download(url, dest, sha):
    if os.path.exists(dest) and hashlib.sha256(open(dest, "rb").read()).hexdigest() == sha:
        return
    os.makedirs(os.path.dirname(dest), exist_ok=True)
    h = hashlib.sha256()
    with urllib.request.urlopen(url, timeout=60) as r, open(dest + ".part", "wb") as f:
        total, got = int(r.headers.get("Content-Length", 0)), 0
        while True:
            b = r.read(1 << 16)
            if not b:
                break
            f.write(b)
            h.update(b)
            got += len(b)
            if TTY and total:
                n = int(got * 30 / total)
                sys.stdout.write("\r\033[K  %s %s%s %3d%%" % (cyan("↓"), "█" * n, dim("░" * (30 - n)), got * 100 // total))
                sys.stdout.flush()
    if TTY:
        sys.stdout.write("\r\033[K")
    if h.hexdigest() != sha:
        os.remove(dest + ".part")
        raise Fail("checksum mismatch for " + os.path.basename(dest))
    os.rename(dest + ".part", dest)


def inspect(r):
    out = r.sh(""". /etc/openwrt_release 2>/dev/null || { echo NOTOPENWRT; exit 0; }
echo "$DISTRIB_ID|$DISTRIB_RELEASE|$DISTRIB_TARGET|$DISTRIB_ARCH"
jsonfilter -i /etc/board.json -e '@.model.id' -e '@.model.name'
awk '/MemAvailable/{print $2}' /proc/meminfo
command -v sysupgrade >/dev/null && echo HAS_SYSUPGRADE
(fw_printenv -n boot_part >/dev/null 2>&1 && grep -q alt_kernel /proc/mtd && grep -q alt_rootfs /proc/mtd) && echo DUAL || true
command -v apk >/dev/null && echo PM_APK || echo PM_OPKG
[ -e /etc/config/passwall2 ] && echo HAS_PW2 || true
""").splitlines()
    if out[0] == "NOTOPENWRT":
        raise Fail("this router is not running OpenWrt; flash the first OpenWrt image with the vendor tools first")
    did, rel, target, arch = out[0].split("|")
    return dict(distro=did, release=rel, target=target, arch=arch, board=out[1], model=out[2],
                mem=int(out[3]) * 1024, sysupgrade="HAS_SYSUPGRADE" in out, dual="DUAL" in out,
                pm="apk" if "PM_APK" in out else "opkg", had_pw2="HAS_PW2" in out)


def build_keep(r, carry_net, carry_pw):
    paths = ["etc/dropbear", "etc/passwd", "etc/shadow", "etc/group"]
    if carry_net:
        paths += ["etc/config/%s" % n for n in ("network", "wireless", "dhcp", "system", "dropbear")]
    if carry_pw:  # config + custom rule files next to the stock geoip/geosite
        paths.append("etc/config/passwall2")
    extra = "ls usr/share/v2ray/*.dat 2>/dev/null | grep -vE '/geo(ip|site)\\.dat$' || true" if carry_pw else "true"
    # tar runs on the router, so files keep root ownership (dropbear rejects anything else)
    r.sh("cd /; x=$(%s); tar czf /tmp/wrtforge-keep.tgz $(for f in %s $x; do [ -e $f ] && echo $f; done)"
         % (extra, " ".join(paths)))


def flash(r, img_path):
    r.put(img_path, "/tmp/wrtforge-fw.bin")
    r.sh("sysupgrade -T -f /tmp/wrtforge-keep.tgz /tmp/wrtforge-fw.bin")
    r.run("nohup sh -c 'sleep 2; sysupgrade -f /tmp/wrtforge-keep.tgz /tmp/wrtforge-fw.bin' "
          ">/tmp/wrtforge-up.log 2>&1 </dev/null &", check=False, timeout=20)


FALLBACK = r"""
# write the official image into the INACTIVE slot (the one we are not running from)
KERN=@KERN@; ROOT=@ROOT@
kn=$(awk -v n="\"$KERN\"" '$4==n{print substr($1,4,length($1)-4)}' /proc/mtd)
rn=$(awk -v n="\"$ROOT\"" '$4==n{print substr($1,4,length($1)-4)}' /proc/mtd)
[ -n "$kn" ] && [ -n "$rn" ]
for u in /sys/class/ubi/ubi[0-9]*; do
  m=$(cat $u/mtd_num 2>/dev/null) || continue
  [ "$m" = "$rn" ] || [ "$m" = "$kn" ] && ubidetach -d ${u##*ubi} || true
done
mtd -q erase $KERN >/dev/null 2>&1
ubiformat /dev/mtd$rn -y -q >/dev/null 2>&1
ubiattach -m $rn -d 8 >/dev/null
ubimkvol /dev/ubi8 -N rootfs -s @RSIZE@ >/dev/null
ubimkvol /dev/ubi8 -N rootfs_data -m >/dev/null
ubiupdatevol /dev/ubi8_0 -s @RSIZE@ /tmp/wf-root.bin
mtd -q write /tmp/wf-kernel.bin $KERN
k=$(dd if=/dev/mtd$kn bs=@KSIZE@ count=1 2>/dev/null | sha256sum | cut -d' ' -f1)
x=$(dd if=/dev/ubi8_0 bs=@RSIZE@ count=1 2>/dev/null | sha256sum | cut -d' ' -f1)
ubidetach -d 8
rm -f /tmp/wf-kernel.bin /tmp/wf-root.bin
echo "$k $x"
"""


def fallback(r, img_path):
    with tarfile.open(img_path) as t:
        parts = {os.path.basename(m.name): t.extractfile(m).read() for m in t.getmembers() if m.isfile()
                 and os.path.basename(m.name) in ("kernel", "root")}
    cmdline = r.run("cat /proc/cmdline").stdout
    kern, root = ("kernel", "rootfs") if "ubi.mtd=alt_rootfs" in cmdline else ("alt_kernel", "alt_rootfs")
    r.run("cat > /tmp/wf-kernel.bin", stdin=parts["kernel"])
    r.run("cat > /tmp/wf-root.bin", stdin=parts["root"])
    s = FALLBACK.replace("@KERN@", kern).replace("@ROOT@", root)
    s = s.replace("@RSIZE@", str(len(parts["root"]))).replace("@KSIZE@", str(len(parts["kernel"])))
    got = r.sh(s, timeout=900).split()[-2:]
    want = [hashlib.sha256(parts["kernel"]).hexdigest(), hashlib.sha256(parts["root"]).hexdigest()]
    if got != want:
        raise Fail("fallback slot read-back does not match the image; slot %s left unbootable" % kern)
    return kern


PASSWALL = r"""
PM=@PM@
BASE=@BASE@
DEPS="xray-core chinadns-ng geoview v2ray-geoip v2ray-geosite dns2socks tcping ip-full kmod-nft-tproxy kmod-nft-socket coreutils-nohup coreutils-base64 resolveip"
if [ $PM = apk ]; then
  wget -qO /etc/apk/keys/passwall.pem $BASE/apk.pub
  for f in @FEEDS@; do grep -qxF "$f" /etc/apk/repositories.d/customfeeds.list 2>/dev/null || echo "$f" >> /etc/apk/repositories.d/customfeeds.list; done
  apk update >/dev/null
  apk info -e dnsmasq-full >/dev/null || { apk del dnsmasq >/dev/null; apk add dnsmasq-full >/dev/null || apk add dnsmasq >/dev/null; }
  /etc/init.d/dnsmasq restart >/dev/null 2>&1 || true
  [ -e /www/cgi-bin/luci ] || apk add luci >/dev/null
  apk add $DEPS >/dev/null
  apk add luci-app-passwall2 >/dev/null
else
  wget -qO /tmp/ipk.pub $BASE/ipk.pub && opkg-key add /tmp/ipk.pub
  for f in @FEEDS@; do n=pw_$(basename $f); grep -q "$f" /etc/opkg/customfeeds.conf 2>/dev/null || echo "src/gz $n $f" >> /etc/opkg/customfeeds.conf; done
  opkg update >/dev/null
  opkg list-installed | grep -q '^dnsmasq-full ' || { opkg remove dnsmasq >/dev/null; opkg install dnsmasq-full >/dev/null || opkg install dnsmasq >/dev/null; }
  /etc/init.d/dnsmasq restart >/dev/null 2>&1 || true
  [ -e /www/cgi-bin/luci ] || opkg install luci >/dev/null
  opkg install $DEPS >/dev/null
  opkg install luci-app-passwall2 >/dev/null
fi
# passwall2 rejects the legacy value "disable" for these ports (nft errors on every start)
for o in tcp_no_redir_ports udp_no_redir_ports; do
  [ "$(uci -q get passwall2.@global_forwarding[0].$o)" = disable ] && uci delete passwall2.@global_forwarding[0].$o || true
done
uci commit passwall2 2>/dev/null || true
if [ "$(uci -q get passwall2.@global[0].enabled)" = 1 ]; then /etc/init.d/passwall2 restart >/dev/null 2>&1 || true; sleep 8; pgrep -f 'bin/(xray|sing-box)' >/dev/null && echo RUNNING; fi
echo INSTALLED
"""


def passwall(r, info_):
    if r.run("wget -q --spider --timeout=8 %s/apk.pub" % PW, check=False).returncode:
        raise Fail("router cannot reach the Passwall feed (no internet from the router?)")
    fl = feeds(info_["pm"], series(info_["release"]), info_["arch"])
    s = PASSWALL.replace("@PM@", info_["pm"]).replace("@BASE@", PW).replace("@FEEDS@", " ".join(fl))
    return r.sh(s, timeout=900).split()


# ───────────────────────────── main ─────────────────────────────
def main(argv=None):
    ap = argparse.ArgumentParser(prog="wrtforge", description=__doc__.splitlines()[0])
    ap.add_argument("host", nargs="?", help="router address (asked if omitted)")
    ap.add_argument("-u", "--user", default="root")
    ap.add_argument("-p", "--port", type=int, default=22)
    ap.add_argument("--auth", choices=["key", "password", "copy-key"], help="how to log in (asked if omitted)")
    ap.add_argument("--version", dest="ver", help="OpenWrt version (default: latest stable)")
    ap.add_argument("--no-passwall", action="store_true")
    ap.add_argument("--no-fallback", action="store_true", help="do not provision the second firmware slot")
    ap.add_argument("--dry-run", action="store_true", help="inspect and plan only; change nothing")
    ap.add_argument("-y", "--yes", action="store_true", help="accept defaults, no questions")
    a = ap.parse_args(argv)
    ARGS.yes = a.yes

    banner()
    host = a.host or ask("Router address", "192.168.1.1")
    auth = a.auth or choose("How should I log in?", [
        ("key", "SSH key already authorised on the router"),
        ("password", "Password " + dim("(ssh asks for it; never stored)")),
        ("copy-key", "Password once, then install my SSH key " + dim("(no more prompts)"))], 2)
    r = Router(host, a.user, a.port, auth)
    try:
        run(a, r, host)
    except Fail as e:
        print("\n  %s %s\n" % (red("✖"), e))
        sys.exit(1)
    except KeyboardInterrupt:
        print("\n  interrupted; the router is untouched unless the flash already started\n")
        sys.exit(130)
    finally:
        r.close()


def run(a, r, host):
    head("Connect")
    if r.auth != "key":
        info("ssh will ask for the password for %s@%s" % (r.user, host))
    r.connect()
    if r.auth == "copy-key":
        with Step("Installing your SSH public key") as s:
            r.install_key()
            s.done("SSH key installed; no more password prompts")

    head("Inspect")
    with Step("Reading device info") as s:
        d = inspect(r)
        s.done("%s %s" % (bold(d["model"]), dim("(%s)" % d["board"])))
    info("running %s %s   %s / %s" % (d["distro"], d["release"], d["target"], d["arch"]))
    if not d["sysupgrade"]:
        raise Fail("no sysupgrade on the router")

    with Step("Finding the latest stable OpenWrt") as s:
        ver = a.ver or fetch_json(DL + "/.versions.json")["stable_version"]
        prof = fetch_json("%s/releases/%s/targets/%s/profiles.json" % (DL, ver, d["target"]))
        img = pick_image(prof, d["board"])
        if not img:
            raise Fail("OpenWrt %s has no image for %s" % (ver, d["board"]))
        s.done("OpenWrt %s  %s" % (bold(ver), dim("%s, %.1f MB" % (img["name"], img["size"] / 1e6))))
    if d["mem"] < img["size"] * 2:
        raise Fail("not enough free RAM on the router to stage the image")

    slot = d["dual"] and not a.no_fallback
    carry_net = carry_pw = False
    if not a.dry_run:
        head("Plan")
        carry_net = confirm("Carry over LAN/Wi-Fi/DHCP settings? (keeps the router reachable)", True)
        if d["had_pw2"] and not a.no_passwall:
            carry_pw = confirm("Carry over your existing Passwall2 config?", True)
    print()
    for line in ["flash OpenWrt %s (currently %s)" % (ver, d["release"]),
                 "keep: SSH keys + root password%s%s" % (", network/Wi-Fi" if carry_net else "", ", Passwall2" if carry_pw else ""),
                 "fallback slot: " + ("write the same official image to the second slot" if slot else "skipped"),
                 "Passwall2: " + ("install from official feeds (%s)" % d["pm"] if not a.no_passwall else "skipped")]:
        print("    " + dim("•") + " " + line)
    if a.dry_run:
        print("\n  " + green("dry run: nothing changed") + "\n")
        return
    warn("the router reboots and your internet through it drops for ~5-15 minutes")
    if not confirm("Go ahead?", False):
        raise Fail("cancelled; the router was not changed")

    head("Flash")
    dest = os.path.join(CACHE, img["name"])
    url = "%s/releases/%s/targets/%s/%s" % (DL, ver, d["target"], img["name"])
    with Step("Downloading %s" % img["name"]) as s:
        download(url, dest, img["sha256"])
        s.done("Downloaded and verified sha256 %s…" % img["sha256"][:12])
    after = host
    if not carry_net:
        after = ask("Router address after flashing", "192.168.1.1")
    with Step("Uploading and checking the image on the router") as s:
        build_keep(r, carry_net, carry_pw)
        flash(r, dest)
        s.done("Image accepted by sysupgrade; flashing and rebooting")
    with Step("Waiting for the router to come back") as s:
        wait_reboot(after, r.port)
        s.done("Router is back")
    r.host = after
    subprocess.run(r._base(False) + ["-O", "exit"], capture_output=True)
    r.connect()
    now = inspect(r)
    if now["release"] != ver:
        raise Fail("router reports %s after flashing, expected %s" % (now["release"], ver))
    info("now running %s %s" % (now["distro"], now["release"]))

    if slot:
        head("Fallback slot")
        with Step("Writing official OpenWrt to the second slot") as s:
            k = fallback(r, dest)
            s.done("Fallback slot ready %s" % dim("(%s, verified by hash)" % k))
        info("it boots automatically if the main slot fails; stock settings, 192.168.1.1")

    if not a.no_passwall:
        head("Passwall2")
        with Step("Installing Passwall2 from the official feeds") as s:
            res = passwall(r, now)
            s.done("Passwall2 installed" + (" and running" if "RUNNING" in res else ""))
        if "RUNNING" not in res:
            info("open LuCI -> Services -> Passwall 2, add your node, enable it")

    print("\n  " + green("✔ done") + "  " + dim("LuCI: http://%s" % r.host) + "\n")


if __name__ == "__main__":
    main()
