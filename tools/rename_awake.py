"""Apply the one-time source rename from the repository root. Safe to rerun."""

import re
import subprocess
from pathlib import Path


COMMAND = re.compile(r"(?<![A-Za-z0-9_-])insomnia(?![A-Za-z0-9_]|-linux|-awake)")
DISPLAY = re.compile(r"(?<![A-Za-z0-9_./])(?<!trait )Insomnia(?![A-Za-z0-9_]| Awake| for macOS)")
SKIP = {"Cargo.lock", "LICENSE", "decisions.tsv", "docs/decisions.tsv", "docs/upgrade-from-insomnia.md", "tools/rename_awake.py"}

for suffix in ["service", "desktop", "svg"]:
    old = Path(f"packaging/insomnia.{suffix}")
    if old.exists():
        subprocess.run(["git", "mv", str(old), f"packaging/insomnia-awake.{suffix}"], check=True)

files = subprocess.check_output(["git", "ls-files", "-z"]).decode().split("\0")
for name in files:
    if not name or name in SKIP or name.endswith(".svg"):
        continue
    path = Path(name)
    text = path.read_text()
    new = text.replace("app.getinsomnia.Insomnia", "io.github.rafaelt0.InsomniaAwake")
    new = new.replace("/app/getinsomnia/Insomnia", "/io/github/rafaelt0/InsomniaAwake")
    new = COMMAND.sub("insomnia-awake", new)
    new = new.replace('CARGO_BIN_EXE_insomnia"', 'CARGO_BIN_EXE_insomnia-awake"')
    if name == "README.md":
        new = new.replace("# Insomnia for Linux", "# Insomnia Awake", 1)
    new = DISPLAY.sub("Insomnia Awake", new)
    if new != text:
        path.write_text(new)
        print(name)
