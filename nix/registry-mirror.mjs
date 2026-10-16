// A read-only registry on loopback over a tree of pinned tarballs, for
// `pnpm install` in the build sandbox (nix/pnpm.nix).
//
//   node registry-mirror.mjs <tree> <port file>
//
// <tree> holds each tarball at its URL's host and path
// (registry.npmjs.org/<name>/-/<file>.tgz, npm.jsr.io/~/11/@jsr/...), which
// is where the mirror serves it. It listens on a free port and writes the
// port to <port file> once it does. pnpm installs from a lockfile it trusts,
// so it asks for tarballs only: anything else is a 404, logged, which fails
// the install rather than letting it guess.

import { createReadStream, renameSync, statSync, writeFileSync } from "node:fs";
import { createServer } from "node:http";
import { resolve, sep } from "node:path";

const [tree, portFile] = process.argv.slice(2).map((path) => resolve(path));

const server = createServer((request, response) => {
  const path = decodeURIComponent(new URL(request.url, "http://mirror").pathname);
  const file = resolve(tree, "." + path);
  let size = null;
  if (request.method === "GET" && file.startsWith(tree + sep)) {
    try {
      const stat = statSync(file);
      if (stat.isFile()) size = stat.size;
    } catch {
      // Not here.
    }
  }
  if (size === null) {
    console.error(`registry-mirror: not here: ${request.method} ${path}`);
    response.writeHead(404, { "content-type": "text/plain" }).end("not found\n");
    return;
  }
  response.writeHead(200, {
    "content-type": "application/octet-stream",
    "content-length": size,
  });
  createReadStream(file).pipe(response);
});

server.listen(0, "127.0.0.1", () => {
  // Written whole, then renamed, so that a reader never sees half of it.
  writeFileSync(portFile + ".tmp", String(server.address().port));
  renameSync(portFile + ".tmp", portFile);
});
