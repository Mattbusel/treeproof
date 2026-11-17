"""Turn a `treeproof verify --json` report into GitHub Actions error annotations."""

import json
import sys

report = json.load(open(sys.argv[1], encoding="utf-8"))
for problem in report.get("problems", []):
    print("::error title=treeproof::" + problem)
for rep in report.get("reports", []):
    for err in rep.get("errors", []):
        print("::error title=treeproof::" + rep.get("source", "") + ": " + err)
