"""Package the built local editor; no signing account or global install required."""
from pathlib import Path
import hashlib
import json
import plistlib
import shutil
import subprocess
import os
import tempfile
import argparse
from macos_release import compile_icon, copy_helper

root = Path(__file__).resolve().parents[1]
binary = root / "target/debug/tandem"
if not binary.is_file():
    raise SystemExit("Build first: cargo build -p tandem")
parser = argparse.ArgumentParser()
parser.add_argument("--profile", choices=["m1", "m2", "m3", "m4", "m5"], default="m5")
profile = parser.parse_args().profile
bundle = root / f".tandem-local/{profile}/Tandem.app"
destination = bundle / "Contents/MacOS/tandem"
destination.parent.mkdir(parents=True, exist_ok=True)
# Replace the directory entry; never truncate an executable a running app maps.
with tempfile.NamedTemporaryFile(dir=destination.parent, delete=False) as pending:
    pending_path = Path(pending.name)
try:
    shutil.copy2(binary, pending_path)
    pending_path.chmod(0o755)
    os.replace(pending_path, destination)
finally:
    pending_path.unlink(missing_ok=True)
icon_keys = compile_icon(bundle / "Contents/Resources")
(bundle / "Contents/Info.plist").write_bytes(plistlib.dumps({
    "CFBundleExecutable": "tandem", "CFBundleIdentifier": "dev.tandem.desktop" if profile == "m1" else f"dev.tandem.{profile}qa",
    "CFBundleName": "Tandem" if profile == "m1" else f"Tandem {profile.upper()}", "CFBundlePackageType": "APPL",
    "CFBundleShortVersionString": "0.1.0", "CFBundleVersion": profile[1:],
    "NSHighResolutionCapable": True, **icon_keys,
}))
copy_helper(bundle / "Contents/Resources")
subprocess.run(["codesign", "--force", "--sign", "-", str(bundle)], check=True)
(bundle.parent / "package-receipt.json").write_text(json.dumps({
    "bundle": str(bundle), "binary_sha256": hashlib.sha256(destination.read_bytes()).hexdigest(),
    "development_build": True, "signature": "ad-hoc", "notarized": False,
}, indent=2) + "\n")
print(bundle)
