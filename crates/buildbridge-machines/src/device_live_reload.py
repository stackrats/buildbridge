"""Prepare a disposable iPhone app copy; the signed build and source stay unchanged."""

import json
import pathlib
import plistlib
import shutil
import sys
import urllib.parse


def prepare(source, destination, url):
    source = pathlib.Path(source)
    destination = pathlib.Path(destination)
    if (
        not source.is_absolute()
        or not destination.is_absolute()
        or source.is_symlink()
        or not source.is_dir()
        or source.suffix != ".app"
        or destination.suffix != ".app"
        or destination.exists()
        or destination.is_symlink()
        or destination.resolve() != destination
        or source.resolve() in destination.parents
        or destination in source.resolve().parents
    ):
        raise ValueError("The live reload app must be a separate, new app copy.")

    # Refuse links for the two files we change. Framework links are copied as links, so
    # embedded code stays byte-for-byte intact and does not need to be signed again.
    for name in ("capacitor.config.json", "Info.plist"):
        path = source / name
        if path.is_symlink() or not path.is_file() or path.stat().st_size > 1024 * 1024:
            raise ValueError("The built app needs regular Capacitor config and Info.plist files.")
    with (source / "capacitor.config.json").open() as file:
        config = json.load(file)
    with (source / "Info.plist").open("rb") as file:
        info = plistlib.load(file)
    if not isinstance(config, dict) or not isinstance(info, dict):
        raise ValueError("The built app's Capacitor config or Info.plist is invalid.")
    server = config.setdefault("server", {})
    if not isinstance(server, dict):
        raise ValueError("The built app's Capacitor server settings are invalid.")
    server["url"] = url

    host = urllib.parse.urlsplit(url).hostname
    if not host:
        raise ValueError("The development server URL has no hostname.")
    if url.startswith("http://"):
        ats = info.setdefault("NSAppTransportSecurity", {})
        if not isinstance(ats, dict):
            raise ValueError("The built app's transport security settings are invalid.")
        domains = ats.setdefault("NSExceptionDomains", {})
        if not isinstance(domains, dict):
            raise ValueError("The built app's transport security exceptions are invalid.")
        exception = domains.setdefault(host, {})
        if not isinstance(exception, dict):
            raise ValueError("The development server's transport security exception is invalid.")
        # Scope HTTP to this host, keeping HTTPS trust and existing native API policies.
        # IP exception keys are supported since iOS 17; older iOS exempted IPs from ATS.
        exception["NSExceptionAllowsInsecureHTTPLoads"] = True
    usage = info.get("NSLocalNetworkUsageDescription")
    if not isinstance(usage, str) or not usage.strip():
        info["NSLocalNetworkUsageDescription"] = (
            "Connect to your development server to reload app changes while developing."
        )
    if "WKAppBoundDomains" in info:
        domains = info["WKAppBoundDomains"]
        if (
            not isinstance(domains, list)
            or len(domains) > 10
            or not all(isinstance(item, str) for item in domains)
        ):
            raise ValueError("The built app's app-bound domains are invalid.")
        if host not in domains:
            if len(domains) >= 10:
                raise ValueError("Add the development server to WKAppBoundDomains; its ten domains are already used.")
            domains.append(host)

    destination.parent.mkdir(parents=True, exist_ok=True)
    shutil.copytree(source, destination, symlinks=True)
    # Replace files instead of following any copied link or hard link when writing.
    for name, contents in (
        ("capacitor.config.json", (json.dumps(config, indent=2) + "\n").encode()),
        ("Info.plist", plistlib.dumps(info)),
    ):
        path = destination / name
        path.unlink()
        path.write_bytes(contents)


if __name__ == "__main__":
    try:
        payload = json.loads(sys.stdin.buffer.read(16384))
        prepare(payload["source"], payload["destination"], payload["url"])
    except (OSError, ValueError, KeyError, TypeError, plistlib.InvalidFileException) as error:
        print("Could not prepare the live reload app: " + str(error), file=sys.stderr)
        sys.exit(1)
