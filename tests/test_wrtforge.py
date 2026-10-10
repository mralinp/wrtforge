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


if __name__ == "__main__":
    unittest.main()
