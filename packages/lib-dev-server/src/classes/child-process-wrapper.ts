import colors from "picocolors"
import type { ViteDevServer } from "vite"
import type { ViteNodeServer } from "vite-node/server"

import { ChildProcess, Serializable, fork } from "node:child_process"
import type { Readable } from "node:stream"

import { byLine } from "@xen-ilp/lib-itergen-utils"
import { createLogger } from "@xen-ilp/lib-logger"
import type { Logger } from "@xen-ilp/lib-logger"
import { UnreachableCaseError, assertDefined } from "@xen-ilp/lib-type-utils"

import { ClientRequest, schema } from "../schemas/client-request"
import type { NodeDefinition } from "../server"
import RpcHost from "./rpc-host"

const RUNNER_MODULE = "@xen-ilp/lib-dev-server/runner"

export default class ChildProcessWrapper<T> {
  readonly prefix: string
  private state: "idle" | "running" | "stopping" = "idle"
  private child: ChildProcess | undefined
  private startPromise: Promise<void> | undefined
  private stopPromise: Promise<void> | undefined
  private logger: Logger
  private rpcHost: RpcHost<ClientRequest>

  constructor(
    private readonly viteServer: ViteDevServer,
    private readonly nodeServer: ViteNodeServer,
    public readonly node: NodeDefinition<T>
  ) {
    this.prefix = `◼ ${this.node.id}`
    this.logger = createLogger(this.prefix)
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
        this.logger.error(
          `${colors.red(`unexpected error from idle child process: `)} ${error}`
        )
        break
      case "running":
        this.logger.error(`${colors.red(`child process error: `)} ${error}`)
        break
      case "stopping":
        this.logger.error(
          `${colors.red(`child process error while stopping: `)} ${error}`
        )
        break
      default:
        throw new UnreachableCaseError(this.state)
    }
  }

  handleChildExit = (code: number | null) => {
    if (code === 0) {
      this.logger.info(`${colors.green(`child exited`)}`)
    } else {
      this.logger.error(`child exited with code: ${code}`)
    }
    this.state = "idle"
    this.child = undefined
  }

  handleChildMessage = (message: unknown) => {
    this.rpcHost.handleMessage(message)
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

  async pipeOutput(input: Readable) {
    for await (const line of byLine(input)) {
      this.logger.info(line)
    }
  }

  async start() {
    if (this.state === "running") {
      return this.startPromise
    } else if (this.state === "stopping") {
      return
    } else if (this.state !== "idle") {
      throw new UnreachableCaseError(this.state)
    }

    this.state = "running"

    return (this.startPromise = (async () => {
      const resolvedEntryPoint = await this.nodeServer.resolveId(RUNNER_MODULE)

      if (!resolvedEntryPoint) {
        throw new Error(`${RUNNER_MODULE} not resolvable`)
      }

      const child = (this.child = fork(
        resolvedEntryPoint.id,
        ["--enable-source-maps"],
        {
          silent: true,
        }
      ))
      assertDefined(child.stdout)
      assertDefined(child.stderr)

      child.addListener("error", this.handleChildError)
      child.addListener("exit", this.handleChildExit)
      child.addListener("message", this.handleChildMessage)

      this.pipeOutput(child.stdout)
      this.pipeOutput(child.stderr)

      try {
        await this.rpcHost.call("start", {
          root: this.viteServer.config.root,
          base: this.viteServer.config.base,
          entry: this.node.entry ?? "src/index.ts",
          config: this.node.config,
        })
      } catch (error) {
        this.logger.error(`${colors.red("child failed to start:")} ${error}`)
        throw error
      }

      this.startPromise = undefined
    })())
  }

  async stop() {
    if (this.state === "stopping") {
      return this.stopPromise
    } else if (this.state === "idle") {
      return
    } else if (this.state !== "running") {
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

      if (child.connected) {
        this.rpcHost.callNoReturn("exit", [])
      } else if (this.child.exitCode === null) {
        child.kill()
      }

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
