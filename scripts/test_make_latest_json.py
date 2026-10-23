#!/usr/bin/env python3
"""Teste de scripts/make-latest-json.py: python3 scripts/test_make_latest_json.py"""
import importlib.util
import os
import unittest

spec = importlib.util.spec_from_file_location("m", os.path.join(os.path.dirname(__file__), "make-latest-json.py"))
m = importlib.util.module_from_spec(spec)
spec.loader.exec_module(m)

BASE = "https://github.com/x/y/releases/download/v1.3.0/"


def assets(version="1.3.0"):
    names = [p.format(v=version) for _, p in m.PLATFORMS]
    return {n: (BASE + n, f"assinatura-de-{n}\n") for n in names}


class LatestJson(unittest.TestCase):
    def test_all_platforms_and_variants(self):
        d = m.build("1.3.0", "### Novidades\n", assets(), "2026-10-23T12:00:00Z")
        self.assertEqual(d["version"], "1.3.0")
        self.assertEqual(d["notes"], "### Novidades")
        p = d["platforms"]
        for k in ("linux-x86_64", "linux-x86_64-appimage", "windows-x86_64", "windows-x86_64-nsis",
                  "windows-x86_64-msi", "darwin-aarch64", "darwin-x86_64"):
            self.assertIn(k, p)
        # a chave genérica do Windows aponta para o instalador NSIS, não para o MSI
        self.assertTrue(p["windows-x86_64"]["url"].endswith("x64-setup.exe"))
        self.assertTrue(p["windows-x86_64-msi"]["url"].endswith(".msi"))
        self.assertEqual(p["linux-x86_64"]["signature"], "assinatura-de-Guara_1.3.0_amd64.AppImage")

    def test_missing_package_is_left_out(self):
        a = assets()
        del a["Guara_1.3.0_x64.app.tar.gz"]
        d = m.build("1.3.0", "", a, "2026-10-23T12:00:00Z")
        self.assertNotIn("darwin-x86_64", d["platforms"])
        self.assertIn("darwin-aarch64", d["platforms"])

    def test_other_versions_do_not_leak(self):
        d = m.build("1.3.0", "", assets("1.2.0"), "2026-10-23T12:00:00Z")
        self.assertEqual(d["platforms"], {})


if __name__ == "__main__":
    unittest.main()
