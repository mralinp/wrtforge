import os
import sys
import unittest

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))
import wrtforge as w  # noqa: E402

PROFILES = {"profiles": {"linksys_mx5300": {"supported_devices": ["linksys,mx5300"], "images": [
    {"type": "initramfs", "name": "i"},
    {"type": "sysupgrade", "filesystem": "squashfs", "name": "s.bin", "sha256": "x", "size": 1},
]}}}


class T(unittest.TestCase):
    def test_pick_image(self):
        self.assertEqual(w.pick_image(PROFILES, "linksys,mx5300")["name"], "s.bin")
        self.assertIsNone(w.pick_image(PROFILES, "acme,nope"))

    def test_series(self):
        self.assertEqual(w.series("25.12.5"), "25.12")

    def test_feeds(self):
        f = w.feeds("apk", "25.12", "aarch64_cortex-a53")
        self.assertTrue(f[1].endswith("/packages-25.12/aarch64_cortex-a53/passwall2/packages.adb"))
        self.assertFalse(w.feeds("opkg", "24.10", "x")[0].endswith(".adb"))

    def test_network_verdict(self):
        h = "192.168.1.1"
        self.assertEqual(w.network_verdict(h, ("en5", None), ("en0", "10.0.0.1")), "split")
        self.assertEqual(w.network_verdict(h, ("en0", None), ("en0", h)), "through")
        self.assertEqual(w.network_verdict(h, ("en0", None), ("en0", "10.0.0.1")), "other")
        self.assertEqual(w.network_verdict(h, (None, None), ("en0", "x")), "unknown")

    def test_pick_factory(self):
        p = {"profiles": {"g": {"supported_devices": ["google,wifi"], "images": [
            {"type": "factory", "filesystem": "squashfs", "name": "f.bin"}]}}}
        self.assertEqual(w.pick_image(p, "google,wifi", "factory")["name"], "f.bin")

    def test_parse_subnets(self):
        mac = "lo0: inet 127.0.0.1 netmask 0xff000000\nen0: inet 192.168.80.246 netmask 0xffffff00 broadcast 192.168.80.255"
        lin = "2: eth0    inet 10.1.2.3/16 brd 10.1.255.255 scope global eth0\n1: lo inet 127.0.0.1/8"
        self.assertEqual([str(n) for n in w.parse_subnets(mac)], ["192.168.80.0/24"])
        self.assertEqual([str(n) for n in w.parse_subnets(lin)], ["10.1.2.0/24"])

    def test_classify(self):
        self.assertIn("LuCI", w.classify("", "<title>OpenWrt | Overview</title> luci"))
        self.assertIn("dropbear", w.classify("SSH-2.0-dropbear_2024.86", ""))
        self.assertIsNone(w.classify("SSH-2.0-OpenSSH_9.6", "<html>nas</html>"))


if __name__ == "__main__":
    unittest.main()
