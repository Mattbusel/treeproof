"""Print the CHANGELOG.md section for one version (used by the release workflow)."""

import sys

version = sys.argv[1]
out, on = [], False
for line in open("CHANGELOG.md", encoding="utf-8"):
    if line.startswith("## "):
        on = line[3:].strip().split(" ")[0] == version
        continue
    if on:
        out.append(line)
print("".join(out).strip() or f"treeproof {version}")
