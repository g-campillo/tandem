"""Export selected, noncredential feasibility receipts into repository docs."""
from datetime import datetime, timezone
from pathlib import Path
import hashlib
import json
import re
import subprocess

ROOT = Path(__file__).resolve().parents[1]
PRIVATE = ROOT / ".tandem-local/m0"
PUBLIC = ROOT / "docs/tandem/evidence/m0"
PUBLIC.mkdir(parents=True, exist_ok=True)


def write(name, data):
    text = json.dumps(data, indent=2, ensure_ascii=False) + "\n"
    text = text.replace(str(Path.home()), "$HOME")
    if re.search(r"sk-[A-Za-z0-9]{20,}|https://auth\.openai\.com/", text):
        raise RuntimeError("Unexpected authentication material in evidence")
    (PUBLIC / name).write_text(text)


for name in [
    "subscription-receipt.json", "boundaries-receipt.json",
    "model-boundary-receipt.json", "language-receipt.json",
    "terminal-receipt.json", "editor-measurement.json",
]:
    write(name, json.loads((PRIVATE / name).read_text()))

contract = json.loads((PRIVATE / "runtime-contract.json").read_text())
config = Path.home() / ".codex/config.toml"
after = hashlib.sha256(config.read_bytes()).hexdigest() if config.exists() else None
contract["global_config_unchanged"] = after == contract["global_config_sha256_before"]
contract.pop("global_config_sha256_before", None)
contract["captured_at_utc"] = datetime.now(timezone.utc).isoformat()
contract["cargo_lock_sha256"] = hashlib.sha256((ROOT / "Cargo.lock").read_bytes()).hexdigest()
contract["hardware"] = {
    "cpu": subprocess.check_output(["sysctl", "-n", "machdep.cpu.brand_string"], text=True).strip(),
    "memory_bytes": int(subprocess.check_output(["sysctl", "-n", "hw.memsize"], text=True)),
    "model": subprocess.check_output(["sysctl", "-n", "hw.model"], text=True).strip(),
}
write("runtime-contract.json", contract)
print(f"Exported seven selected receipts to {PUBLIC.relative_to(ROOT)}")
