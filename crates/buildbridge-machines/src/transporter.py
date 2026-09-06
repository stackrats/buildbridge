"""Receive one retained IPA and a one-use App Store Connect credential over SSH."""

import hashlib
import os
import re
import select
import signal
import struct
import subprocess
import sys
import tempfile
import threading
import time


UPLOAD_TIMEOUT_SECONDS = 7200
PREFIX = "__BUILDBRIDGE_TRANSPORTER__:"


class UploadError(Exception):
    pass


def report(message):
    print(PREFIX + message, flush=True)


def read_exact(stream, length):
    chunks = bytearray()
    while len(chunks) < length:
        chunk = stream.read(length - len(chunks))
        if not chunk:
            raise UploadError("The IPA transfer was interrupted; no upload was started.")
        chunks.extend(chunk)
    return bytes(chunks)


def read_frame(stream, maximum):
    length = struct.unpack(">I", read_exact(stream, 4))[0]
    if length > maximum:
        raise UploadError("An upload credential exceeds its size limit.")
    return read_exact(stream, length).decode("utf-8")


def find_transporter():
    applications = [os.path.expanduser("~/Applications"), "/Applications"]
    candidates = []
    for directory in applications:
        candidates.append(directory + "/Transporter.app/Contents/itms/bin/iTMSTransporter")
        candidates.append(directory + "/Xcode.app/Contents/SharedFrameworks/ContentDeliveryServices.framework/Versions/A/itms/bin/iTMSTransporter")
        candidates.append(directory + "/Xcode.app/Contents/Applications/Application Loader.app/Contents/itms/bin/iTMSTransporter")
    candidates.append("/usr/local/itms/bin/iTMSTransporter")
    try:
        developer = subprocess.check_output(
            ["/usr/bin/xcode-select", "--print-path"],
            stderr=subprocess.DEVNULL,
            timeout=5,
        ).decode("utf-8").strip()
        candidates.append(os.path.dirname(developer) + "/SharedFrameworks/ContentDeliveryServices.framework/Versions/A/itms/bin/iTMSTransporter")
    except (OSError, subprocess.SubprocessError, UnicodeError):
        pass
    for candidate in candidates:
        if os.path.isfile(candidate) and os.access(candidate, os.X_OK):
            return candidate
    raise UploadError("Transporter is not installed in the macOS machine. Install Apple's Transporter app from the Mac App Store in that machine, then retry.")


def stop_process(process):
    # Transporter's launcher starts Java children. Stop the whole session before
    # removing the key and IPA, including when the SSH connection disappears.
    if process is None:
        return
    try:
        os.killpg(process.pid, signal.SIGTERM)
    except ProcessLookupError:
        return
    try:
        process.wait(timeout=2)
    except subprocess.TimeoutExpired:
        pass
    try:
        os.killpg(process.pid, signal.SIGKILL)
    except ProcessLookupError:
        pass
    process.wait(timeout=3)


def summarize_output(stream, summary):
    # Never forward Transporter's raw output, which may contain authentication
    # material. Keep only a small window for classification and numeric ITMS codes.
    window = b""
    while True:
        chunk = stream.read(4096)
        if not chunk:
            return
        window = (window + chunk)[-8192:]
        lower = window.lower()
        if any(value in lower for value in (b"unauthorized", b"authentication", b"invalid jwt", b"not authorized")):
            summary["authentication"] = True
        if any(value in lower for value in (b"already been used", b"duplicate binary", b"redundant binary", b"itms-90189", b"itms-90186")):
            summary["duplicate"] = True
        for code in re.findall(rb"\bITMS-[0-9]{4,6}\b", window):
            if len(summary["codes"]) < 10:
                summary["codes"].add(code)


def disconnected(stream):
    ready, _, _ = select.select([stream], [], [], 0)
    if not ready:
        return False
    # After the payload the host retains stdin without sending anything. EOF
    # means SSH was cancelled or disconnected; extra bytes are also invalid.
    stream.read(1)
    return True


def upload(stream):
    transporter = find_transporter()
    key_id = read_frame(stream, 64)
    issuer_id = read_frame(stream, 64)
    private_key = read_frame(stream, 16384)
    expected_sha256 = read_frame(stream, 64)
    size = struct.unpack(">Q", read_exact(stream, 8))[0]
    if not re.fullmatch(r"[A-Z0-9]{10}", key_id):
        raise UploadError("Enter the 10-character App Store Connect API Key ID.")
    if not re.fullmatch(r"[0-9a-fA-F]{8}(?:-[0-9a-fA-F]{4}){3}-[0-9a-fA-F]{12}", issuer_id):
        raise UploadError("Enter the App Store Connect team API key's Issuer ID.")
    if not re.fullmatch(r"[0-9a-fA-F]{64}", expected_sha256) or not 0 < size <= 16 * 1024**3:
        raise UploadError("The retained IPA has invalid size or checksum metadata.")
    if not private_key.startswith("-----BEGIN PRIVATE KEY-----") or not private_key.rstrip().endswith("-----END PRIVATE KEY-----"):
        raise UploadError("Choose the App Store Connect API private key (.p8).")

    os.umask(0o077)
    with tempfile.TemporaryDirectory(prefix="buildbridge-transporter-", dir="/tmp") as directory:
        ipa = os.path.join(directory, "App-AppStore.ipa")
        remaining = size
        digest = hashlib.sha256()
        with open(ipa, "xb") as target:
            while remaining:
                chunk = read_exact(stream, min(remaining, 256 * 1024))
                target.write(chunk)
                digest.update(chunk)
                remaining -= len(chunk)
        if digest.hexdigest() != expected_sha256.lower():
            raise UploadError("The IPA changed during transfer. Its checksum does not match the retained release; no upload was started.")
        if disconnected(stream):
            raise UploadError("Upload stopped before delivery to Apple.")
        keys = os.path.join(directory, "private_keys")
        os.mkdir(keys, 0o700)
        with open(os.path.join(keys, "AuthKey_" + key_id + ".p8"), "x") as key:
            key.write(private_key)

        process = None
        try:
            report("The retained IPA is verified. Transporter is uploading to App Store Connect.")
            process = subprocess.Popen(
                [transporter, "-m", "upload", "-assetFile", ipa, "-apiKey", key_id, "-apiIssuer", issuer_id, "-v", "critical"],
                cwd=directory,
                stdin=subprocess.DEVNULL,
                stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT,
                start_new_session=True,
            )
            summary = {"authentication": False, "duplicate": False, "codes": set()}
            reader = threading.Thread(target=summarize_output, args=(process.stdout, summary), daemon=True)
            reader.start()
            while process.poll() is None:
                if disconnected(stream):
                    raise UploadError("Upload stopped. Check App Store Connect before retrying; Apple may have received the build.")
                time.sleep(0.2)
            reader.join(timeout=2)
            if disconnected(stream):
                raise UploadError("Upload stopped. Check App Store Connect before retrying; Apple may have received the build.")
            if process.returncode:
                if summary["authentication"]:
                    raise UploadError("Transporter could not authenticate. Check the team API key, Issuer ID, and the key's access to this app in App Store Connect.")
                if summary["duplicate"]:
                    raise UploadError("Apple has already received this version or build number. Check the existing build in App Store Connect, or archive a new build with a higher build number.")
                codes = ", ".join(code.decode("ascii") for code in sorted(summary["codes"])[:10])
                detail = (" Apple reported " + codes + ".") if codes else ""
                raise UploadError("Transporter rejected the upload (exit " + str(process.returncode) + ")." + detail + " Check this app's bundle ID, version/build number and account access in App Store Connect before retrying.")
        finally:
            stop_process(process)
    report("Upload delivered. Apple must process the build before it appears in TestFlight.")


def stopped(signum, frame):
    if signum == signal.SIGALRM:
        raise UploadError("The upload timed out. Check App Store Connect before retrying; Apple may have received the build.")
    raise UploadError("Upload stopped. Check App Store Connect before retrying; Apple may have received the build.")


def main():
    for signum in (signal.SIGHUP, signal.SIGTERM, signal.SIGINT, signal.SIGALRM):
        signal.signal(signum, stopped)
    signal.alarm(UPLOAD_TIMEOUT_SECONDS)
    try:
        # Unbuffered input is essential: select must observe EOF only after the
        # exact payload, while the host holds the connection open for completion.
        with os.fdopen(os.dup(sys.stdin.fileno()), "rb", buffering=0) as stream:
            upload(stream)
    except UploadError as error:
        report("ERROR: " + str(error))
        return 1
    except (OSError, ValueError, UnicodeError, subprocess.SubprocessError):
        report("ERROR: The macOS upload could not finish. Check the machine's connection and available storage, then retry.")
        return 1
    finally:
        signal.alarm(0)
    return 0


if __name__ == "__main__":
    sys.exit(main())
