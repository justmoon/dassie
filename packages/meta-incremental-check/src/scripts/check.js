#!/usr/bin/env node
import { createServer } from "vite"
import { ViteNodeRunner } from "vite-node/client"
import { ViteNodeServer } from "vite-node/server"
import { installSourcemapsSupport } from "vite-node/source-map"

import { resolve } from "node:path"
import { isMainThread } from "node:worker_threads"

const CURRENT_FILE_PATH = new URL(import.meta.url).pathname

const ROOT_PATH = resolve(CURRENT_FILE_PATH, "../../../")
const server = await createServer({
  root: ROOT_PATH,
  logLevel: "error",
  configFile: "packages/app-dev/vite.backend.config.js",
  server: {
    hmr: false,
    watch: {
      ignored: [
        "**/dist/**",
        "**/.next/**",
        "**/.turbo/**",
        "**/.cache/**",
        "**/.vagrant/**",
      ],
    },
  },
  clearScreen: false,
})
await server.pluginContainer.buildStart({})

const node = new ViteNodeServer(server)

installSourcemapsSupport({
  getSourceMap: (source) => node.getSourceMap(source),
})

const runner = new ViteNodeRunner({
  root: server.config.root,
  base: server.config.base,
  fetchModule(id) {
    return node.fetchModule(id)
  },
  resolveId(id, importer) {
    return node.resolveId(id, importer)
  },
})

await runner.executeFile(
  resolve(
    CURRENT_FILE_PATH,
    `../../entry/${isMainThread ? "main" : "worker"}.ts`,
  ),
)

await server.close()
