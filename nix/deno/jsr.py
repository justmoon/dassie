"""Writes nix/deno/jsr.json: the files of every JSR version deno.lock pins.

    python3 nix/deno/jsr.py            (from the repository root; `nix run .#deno-jsr`)

A JSR version's `_meta.json`, whose hash deno.lock records, lists its files
and their hashes. Nix fetches each file by that hash, so it needs the list
before it builds anything; committing it here keeps evaluation free of
fetches. A version's files never change, so this only adds and drops
versions as deno.lock does. The build checks the list against the fetched
`_meta.json` (nix/deno/deps.nix).
"""

import hashlib
import json
import re
import sys
import urllib.request

lock = json.load(open("deno.lock"))
out = {}
for key, entry in sorted(lock.get("jsr", {}).items()):
    name, version = re.fullmatch(r"(@[^@]+)@(.+)", key).groups()
    url = f"https://jsr.io/{name}/{version}_meta.json"
    # jsr.io refuses urllib's default user agent.
    req = urllib.request.Request(url, headers={"User-Agent": "dassie nix/deno/jsr.py"})
    with urllib.request.urlopen(req) as r:
        data = r.read()
    if hashlib.sha256(data).hexdigest() != entry["integrity"]:
        sys.exit(f"{url}: does not match deno.lock")
    manifest = json.loads(data)["manifest"]
    out[key] = {path: f["checksum"] for path, f in sorted(manifest.items())}

with open("nix/deno/jsr.json", "w") as f:
    json.dump(out, f, indent=1, sort_keys=True)
    f.write("\n")
