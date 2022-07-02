import CDP from "chrome-remote-interface"
import colors from "picocolors"
import type { ViteDevServer } from "vite"
import type { ViteNodeServer } from "vite-node/server"

import { ChildProcess, Serializable, fork } from "node:child_process"
import type { Readable } from "node:stream"

import type { EventBroker } from "@xen-ilp/lib-events"
import { byLine } from "@xen-ilp/lib-itergen-utils"
import type { SerializableLogLine } from "@xen-ilp/lib-logger"
import type { Logger } from "@xen-ilp/lib-logger"
import { UnreachableCaseError, assertDefined } from "@xen-ilp/lib-type-utils"

import { ClientRequest, schema } from "../schemas/client-request"
import type { NodeDefinition } from "../servers/node-server"
import { createCliOnlyLogger } from "../services/logger"
import { logLineTopic } from "../topics/log-message"
import RpcHost from "./rpc-host"

const VITE_NODE_BIN = new URL(
  "../../node_modules/vite-node/vite-node.mjs",
  import.meta.url
).pathname
const RUNNER_MODULE = new URL("../runner.ts", import.meta.url).pathname

export interface ChildProcessWrapperContext {
  eventBroker: EventBroker
}

export default class ChildProcessWrapper<T> {
  readonly prefix: string
  private state: "idle" | "running" | "stopping" = "idle"
  private child: ChildProcess | undefined
  private startPromise: Promise<void> | undefined
  private stopPromise: Promise<void> | undefined
  private logger: Logger
  private rpcHost: RpcHost<ClientRequest>
  private cdpClient: CDP.Client | undefined

  constructor(
    readonly context: ChildProcessWrapperContext,
    private readonly viteServer: ViteDevServer,
    private readonly nodeServer: ViteNodeServer,
    public readonly node: NodeDefinition<T>
  ) {
    this.prefix = `◼ ${this.node.id}`
    this.logger = createCliOnlyLogger(this.prefix)

    this.rpcHost = new RpcHost(
      schema,
      this.handleChildRequest,
      this.sendToChild
    )
  }

  fetchModule(id: string) {
    return this.nodeServer.fetchModule(id)
  }

  resolveId(id: string, importer: string) {
    return this.nodeServer.resolveId(id, importer)
  }

  sendToChild = (message: Serializable) => {
    this.child?.send(message)
  }

  handleChildError = (error: Error) => {
    switch (this.state) {
      case "idle":
        this.logger.error("unexpected error from idle child process")
        this.logger.logError(error)
        break
      case "running":
        this.logger.error("child process error")
        this.logger.logError(error)
        break
      case "stopping":
        this.logger.error("child process error while stopping")
        this.logger.logError(error)
        break
      default:
        throw new UnreachableCaseError(this.state)
    }
  }

  handleChildExit = (code: number | null) => {
    if (code === 0) {
      this.logger.info(`${colors.green(`child exited`)}`)
    } else {
      this.logger.error(`child exited with code: ${code ?? "unknown"}`)
    }
    this.state = "idle"
    this.child = undefined
  }

  handleChildMessage = (message: unknown) => {
    this.rpcHost
      .handleMessage(message)
      .catch((error) => this.logger.logError(error))
  }

  handleChildRequest = async (request: ClientRequest) => {
    switch (request.method) {
      case "fetchModule":
        return this.nodeServer.fetchModule(...request.params)
      case "resolveId":
        return this.nodeServer.resolveId(
          request.params[0],
          request.params[1] === null ? undefined : request.params[1]
        )
      default:
        throw new UnreachableCaseError(request)
    }
  }

  async processLog(input: Readable) {
    for await (const line of byLine(input)) {
      try {
        const logLine = JSON.parse(line) as SerializableLogLine
        this.context.eventBroker.emit(logLineTopic, {
          node: this.node.id,
          ...logLine,
        })
        this.logger.info(
          `${logLine.component} ${logLine.message}`,
          logLine.data
        )
      } catch {
        this.context.eventBroker.emit(logLineTopic, {
          node: this.node.id,
          component: "raw",
          message: line,
          date: new Date().toISOString(),
          level: "info",
        })
        this.logger.info(line)
      }
    }
  }

  async start() {
    switch (this.state) {
      case "running":
        return this.startPromise
      case "stopping":
        return
      case "idle":
        break
      default:
        throw new UnreachableCaseError(this.state)
    }

    this.state = "running"

    return (this.startPromise = (async () => {
      const resolvedEntryPoint = await this.nodeServer.resolveId(RUNNER_MODULE)

      if (!resolvedEntryPoint) {
        throw new Error(`${RUNNER_MODULE} not resolvable`)
      }

      const child = (this.child = fork(VITE_NODE_BIN, [resolvedEntryPoint.id], {
        detached: false,
        silent: true,
        execArgv: ["--enable-source-maps", `--inspect=${this.node.debugPort}`],
        env: {
          FORCE_COLOR: "1",
          ...process.env,
          XEN_LOG_FORMATTER: "json",
          XEN_CONFIG: JSON.stringify(this.node.config),
          XEN_DEV_ROOT: this.viteServer.config.root,
          XEN_DEV_BASE: this.viteServer.config.base,
          XEN_DEV_ENTRY: this.node.entry ?? "src/index.ts",
        },
      }))
      child.addListener("error", this.handleChildError)
      child.addListener("exit", this.handleChildExit)
      child.addListener("message", this.handleChildMessage)

      assertDefined(child.stdout)
      assertDefined(child.stderr)

      this.processLog(child.stdout).catch((error) =>
        this.logger.logError(error)
      )
      this.processLog(child.stderr).catch((error) =>
        this.logger.logError(error)
      )

      // Wait for first message from child to indicate it is ready
      await new Promise((resolve) => child.once("message", resolve))

      this.cdpClient = await CDP({
        port: this.node.debugPort,
      })

      this.cdpClient.on("disconnect", () => {
        this.cdpClient = undefined
      })

      this.startPromise = undefined
    })())
  }

  async stop() {
    switch (this.state) {
      case "stopping":
        return this.stopPromise
      case "idle":
        return
      case "running":
        break
      default:
        throw new UnreachableCaseError(this.state)
    }

    this.state = "stopping"

    return (this.stopPromise = (async () => {
      if (!this.child) {
        throw new Error("expected child to be set")
      }

      const child = this.child

      child.removeListener("exit", this.handleChildExit)
      child.removeListener("message", this.handleChildMessage)

      const childEndPromise = new Promise((resolve) => {
        child.once("error", resolve)
        child.once("exit", resolve)
      })

      child.kill("SIGINT")
      // if (this.cdpClient) {
      //   this.logger.info("evaluating")
      //   console.log(
      //     await Promise.all([
      //       this.cdpClient.Runtime.evaluate({
      //         expression: "require('inspector').close()",
      //         includeCommandLineAPI: true,
      //       }),
      //       this.cdpClient.close(),
      //     ])
      //   )

      //   this.logger.info("debugger closed")
      // } else if (child.connected) {
      //   this.rpcHost.callNoReturn("exit", [])
      // } else if (this.child.exitCode === null) {
      //   child.kill()
      // }

      await childEndPromise

      this.state = "idle"

      this.child = undefined
      this.stopPromise = undefined
    })())
  }

  async restart() {
    await this.stop()
    await this.start()
  }
}
