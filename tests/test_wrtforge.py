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


if __name__ == "__main__":
    unittest.main()
