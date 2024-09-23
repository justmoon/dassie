/* eslint-disable @dassie/no-top-level-side-effects */
import { z } from "zod"

import { MessagePort, parentPort } from "node:worker_threads"

import {
  createNodejsMessagePortAdapter,
  createRoute,
  createRouter,
  createServer,
} from "@dassie/lib-rpc/server"

import { runEslint } from "../eslint"
import { runTypeScriptCompiler } from "../typescript"

const rpcRouter = createRouter({
  runTypeScriptCompiler: createRoute()
    .input(z.string())
    .mutation(({ input: projectRoot }) => runTypeScriptCompiler(projectRoot)),
  runEslint: createRoute().mutation(() => runEslint()),
})

export type RpcRouter = typeof rpcRouter

const server = createServer({ router: rpcRouter })

parentPort!.on("message", (message) => {
  if (message instanceof MessagePort) {
    server.handleConnection({
      context: {},
      connection: createNodejsMessagePortAdapter(message),
    })
  }
})
