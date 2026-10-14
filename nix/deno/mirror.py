"""A read-only npm and JSR registry over a directory of pinned files.

    mirror.py <tree> <host:port>

<tree> holds npm/<name>/-/<file>.tgz and jsr/<name>/<version>_meta.json plus
jsr/<name>/<version>/<path> (deps.nix builds it). npm is served under /npm/,
JSR at the root as jsr.io serves it (so Deno's cache of it can become
jsr.io's: runtime-cache.py). Deno asks for some
documents that change over time upstream, so they're made here from the files:
an npm package's packument, from its tarballs' package.json, and a JSR
package's meta.json, listing the versions present.
"""

import base64
import functools
import hashlib
import http.server
import json
import os
import sys
import tarfile
import urllib.parse

tree = sys.argv[1]
host, port = sys.argv[2].rsplit(":", 1)
base = f"http://{host}:{port}"


# The tree doesn't change while this runs, and Deno asks for the same
# documents many times, from many connections at once.
@functools.cache
def packument(name):
    d = os.path.join(tree, "npm", name, "-")
    if not os.path.isdir(d):
        return None
    versions = {}
    for f in sorted(os.listdir(d)):
        path = os.path.join(d, f)
        with tarfile.open(path) as t:
            member = next(m for m in t.getmembers() if m.name.count("/") == 1 and m.name.endswith("/package.json"))
            pkg = json.load(t.extractfile(member))
        data = open(path, "rb").read()
        pkg["dist"] = {
            "tarball": f"{base}/npm/{name}/-/{f}",
            "integrity": "sha512-" + base64.b64encode(hashlib.sha512(data).digest()).decode(),
            "shasum": hashlib.sha1(data).hexdigest(),
        }
        versions[pkg["version"]] = pkg
    latest = list(versions)[-1]
    return {"name": name, "dist-tags": {"latest": latest}, "versions": versions}


@functools.cache
def jsr_meta(name):
    d = os.path.join(tree, "jsr", name)
    if not os.path.isdir(d):
        return None
    versions = {f[: -len("_meta.json")]: {} for f in os.listdir(d) if f.endswith("_meta.json")}
    scope, pkg = name[1:].split("/", 1)
    return {"scope": scope, "name": pkg, "latest": sorted(versions)[-1], "versions": versions}


class Handler(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def do_GET(self):
        path = urllib.parse.unquote(urllib.parse.urlparse(self.path).path)
        if path == "/healthz":
            return self.send(200, b"ok", "text/plain")
        # JSR at the root, as jsr.io has it (JSR_URL=http://<host:port>/), so
        # a module's path here is its path there.
        if path.startswith("/@"):
            path = "/jsr" + path
        body, ctype = None, "application/json"
        if path.startswith("/npm/") and "/-/" not in path:
            doc = packument(path[len("/npm/"):].strip("/"))
            body = doc and json.dumps(doc).encode()
        elif path.startswith("/jsr/") and path.endswith("/meta.json") and path.count("/") == 4:
            doc = jsr_meta(path[len("/jsr/"):-len("/meta.json")])
            body = doc and json.dumps(doc).encode()
        else:
            f = os.path.normpath(os.path.join(tree, path.lstrip("/")))
            if f.startswith(tree + os.sep) and os.path.isfile(f):
                body, ctype = open(f, "rb").read(), "application/octet-stream"
        if body is None:
            sys.stderr.write(f"mirror: not here: {path}\n")
            return self.send(404, b"not found", "text/plain")
        self.send(200, body, ctype)

    def send(self, status, body, ctype):
        self.send_response(status)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *args):
        pass


class Server(http.server.ThreadingHTTPServer):
    # `deno install` connects for hundreds of files at once. With the stdlib's
    # backlog of 5, connections queue in the kernel; on a busy machine some
    # wait so long that they're dropped, and Deno sees a reset.
    request_queue_size = 1024


Server((host, int(port)), Handler).serve_forever()
