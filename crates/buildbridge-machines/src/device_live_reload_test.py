import json
import pathlib
import plistlib
import subprocess
import sys
import tempfile
import unittest

SCRIPT = sys.argv.pop()
namespace = {"__name__": "device_live_reload"}
exec(SCRIPT, namespace)
prepare = namespace["prepare"]


class LiveReloadTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = pathlib.Path(self.temporary.name).resolve()
        self.source = self.root / "DerivedData" / "App.app"
        self.source.mkdir(parents=True)
        self.destination = self.root / "DeviceLiveReload" / "App.app"
        self.config = {"appId": "com.example.app", "server": {"hostname": "app.local"}, "plugins": {"Test": {"enabled": True}}}
        self.info = {"CFBundleIdentifier": "com.example.app", "CFBundleVersion": "42"}
        self.write_source()

    def write_source(self):
        (self.source / "capacitor.config.json").write_text(json.dumps(self.config))
        (self.source / "Info.plist").write_bytes(plistlib.dumps(self.info, fmt=plistlib.FMT_BINARY))

    def read_copy(self):
        return (
            json.loads((self.destination / "capacitor.config.json").read_bytes()),
            plistlib.loads((self.destination / "Info.plist").read_bytes()),
        )

    def test_stdin_recipe_only_changes_the_copy_and_keeps_framework_links(self):
        framework = self.source / "Frameworks" / "Example.framework"
        (framework / "Versions" / "A").mkdir(parents=True)
        (framework / "Versions" / "A" / "Example").write_bytes(b"signed native code")
        (framework / "Example").symlink_to("Versions/A/Example")
        original_config = (self.source / "capacitor.config.json").read_bytes()
        original_info = (self.source / "Info.plist").read_bytes()
        # A shell-shaped URL path stays data all the way through the guest recipe.
        url = "http://192.168.1.5:5173/$(touch%20unexpected)/"
        result = subprocess.run(
            [sys.executable, "-c", SCRIPT],
            input=json.dumps({"source": str(self.source), "destination": str(self.destination), "url": url}),
            text=True,
            capture_output=True,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        config, info = self.read_copy()
        expected_config = dict(self.config)
        expected_config["server"] = {"hostname": "app.local", "url": url}
        self.assertEqual(config, expected_config)
        self.assertEqual(info["CFBundleVersion"], "42")
        self.assertTrue(info["NSAppTransportSecurity"]["NSExceptionDomains"]["192.168.1.5"]["NSExceptionAllowsInsecureHTTPLoads"])
        self.assertTrue(info["NSLocalNetworkUsageDescription"])
        self.assertEqual((self.source / "capacitor.config.json").read_bytes(), original_config)
        self.assertEqual((self.source / "Info.plist").read_bytes(), original_info)
        self.assertTrue((self.destination / "Frameworks" / "Example.framework" / "Example").is_symlink())
        self.assertEqual((self.destination / "Frameworks" / "Example.framework" / "Example").read_bytes(), b"signed native code")

    def test_http_exception_preserves_other_policies_and_local_permission_text(self):
        self.info.update({
            "NSAppTransportSecurity": {
                "NSAllowsArbitraryLoads": True,
                "NSExceptionDomains": {
                    "dev.local": {"NSExceptionAllowsInsecureHTTPLoads": False, "NSRequiresCertificateTransparency": True},
                    "api.example": {"NSExceptionMinimumTLSVersion": "TLSv1.3"},
                },
            },
            "NSLocalNetworkUsageDescription": "Find nearby tools.",
            "WKAppBoundDomains": ["app.local"],
        })
        self.write_source()
        prepare(self.source, self.destination, "http://dev.local:5173/")
        _, info = self.read_copy()
        expected = dict(self.info)
        expected["NSAppTransportSecurity"]["NSExceptionDomains"]["dev.local"]["NSExceptionAllowsInsecureHTTPLoads"] = True
        expected["WKAppBoundDomains"] = ["app.local", "dev.local"]
        self.assertEqual(info, expected)

    def test_https_does_not_add_or_relax_transport_security(self):
        for transport in (None, {"NSExceptionDomains": {"dev.example": {"NSRequiresCertificateTransparency": True}}}):
            with self.subTest(transport=transport):
                self.destination = self.root / str(bool(transport)) / "App.app"
                if transport is not None:
                    self.info["NSAppTransportSecurity"] = transport
                self.write_source()
                prepare(self.source, self.destination, "https://dev.example/")
                _, info = self.read_copy()
                self.assertEqual(info.get("NSAppTransportSecurity"), transport)

    def test_unqualified_and_ipv6_host_exceptions_use_the_host_without_port(self):
        for url, host in [("http://workstation:5173/", "workstation"), ("http://[fd00::1]:5173/", "fd00::1")]:
            with self.subTest(url=url):
                self.destination = self.root / host / "App.app"
                prepare(self.source, self.destination, url)
                _, info = self.read_copy()
                self.assertEqual(list(info["NSAppTransportSecurity"]["NSExceptionDomains"]), [host])

    def test_full_app_bound_domains_fail_before_copy(self):
        self.info["WKAppBoundDomains"] = ["host" + str(index) + ".example" for index in range(10)]
        self.write_source()
        with self.assertRaisesRegex(ValueError, "ten domains"):
            prepare(self.source, self.destination, "http://dev.local/")
        self.assertFalse(self.destination.exists())

    def test_symlinked_config_and_plist_cannot_modify_shared_files(self):
        for name in ("capacitor.config.json", "Info.plist"):
            with self.subTest(name=name):
                target = self.root / name
                path = self.source / name
                original = path.read_bytes()
                path.rename(target)
                path.symlink_to(target)
                with self.assertRaisesRegex(ValueError, "regular"):
                    prepare(self.source, self.destination, "http://dev.local/")
                self.assertEqual(target.read_bytes(), original)
                self.assertFalse(self.destination.exists())
                path.unlink()
                target.rename(path)

    def test_destination_cannot_alias_the_original_app(self):
        alias = self.root / "Alias"
        alias.symlink_to(self.source.parent, target_is_directory=True)
        with self.assertRaisesRegex(ValueError, "separate"):
            prepare(self.source, alias / "Copy.app", "http://dev.local/")
        with self.assertRaisesRegex(ValueError, "separate"):
            prepare(self.source, self.source / "Copy.app", "http://dev.local/")
        self.assertFalse((self.source.parent / "Copy.app").exists())


unittest.main()
