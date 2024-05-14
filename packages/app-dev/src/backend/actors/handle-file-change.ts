import chalk from "chalk"
import { ModuleNode } from "vite"

import path from "node:path"

import { Reactor, createActor } from "@dassie/lib-reactive"

import {
  DEVELOPMENT_SERVER_ENTRYPOINT,
  NODE_ENTRYPOINT,
} from "../constants/entrypoints"
import { vite as logger } from "../logger/instances"
import { PeeringStateStore } from "../stores/peering-state"
import { ViteNodeServer } from "../unconstructables/vite-node-server"
import { ViteServer } from "../unconstructables/vite-server"
import { RunNodesActor } from "./run-nodes"

export function getShortName(file: string, root: string): string {
  return file.startsWith(root + "/") ? path.posix.relative(root, file) : file
}

const isWithinBoundary = (
  node: ModuleNode,
  traversedModules: Set<ModuleNode>,
  boundaryModules: Set<ModuleNode>,
): boolean => {
  if (traversedModules.has(node)) {
    return false
  }
  traversedModules.add(node)

  if (boundaryModules.has(node)) {
    return true
  }

  for (const importer of node.importers) {
    if (isWithinBoundary(importer, traversedModules, boundaryModules)) {
      return true
    }
  }

  return false
}

const areModulesWithinBoundary = (
  changedModules: Set<ModuleNode>,
  boundaryModules: Set<ModuleNode> | undefined,
): boolean => {
  if (!boundaryModules) return false

  const traversedModules = new Set<ModuleNode>()
  for (const module of changedModules) {
    if (isWithinBoundary(module, traversedModules, boundaryModules)) {
      return true
    }
  }
  return false
}

export const HandleFileChangeActor = (reactor: Reactor) => {
  const viteServer = reactor.use(ViteServer)
  const viteNodeServer = reactor.use(ViteNodeServer)
  const peeringStateStore = reactor.use(PeeringStateStore)
  const runNodesActor = reactor.use(RunNodesActor)

  return createActor((sig) => {
    const onFileChange = (file: string) => {
      const { config, moduleGraph } = viteServer
      const shortFile = getShortName(file, config.root)

      // The cache in vite-node doesn't correctly invalidate, so we need to clear it on each file change.
      // It still helps a lot with performance because we are compiling the code once and then re-using it for every running process.
      viteNodeServer.fetchCache = new Map()

      const changedModules = moduleGraph.getModulesByFile(file)
      if (!changedModules) return

      const developmentServerBoundaryModules = moduleGraph.getModulesByFile(
        DEVELOPMENT_SERVER_ENTRYPOINT,
      )
      if (
        areModulesWithinBoundary(
          changedModules,
          developmentServerBoundaryModules,
        )
      ) {
        return
      }

      const nodeBoundaryModules = moduleGraph.getModulesByFile(NODE_ENTRYPOINT)
      if (areModulesWithinBoundary(changedModules, nodeBoundaryModules)) {
        peeringStateStore.clear()

        logger.info(`${chalk.green(`change`)} ${chalk.dim(shortFile)}`)
        runNodesActor.forceRestart()
      }
    }

    viteServer.watcher.on("change", onFileChange)

    sig.onCleanup(() => {
      viteServer.watcher.off("change", onFileChange)
    })
  })
}
