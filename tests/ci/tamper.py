"""Change a count inside a receipt's signed payload without re-signing it."""

import base64
import json
import sys

path = sys.argv[1]
env = json.load(open(path, encoding="utf-8"))
body = base64.b64decode(env["payload"])
forged = body.replace(b'"passed":3', b'"passed":30')
assert forged != body, "fixture receipt did not contain the expected count"
env["payload"] = base64.b64encode(forged).decode()
json.dump(env, open(path, "w", encoding="utf-8"))
