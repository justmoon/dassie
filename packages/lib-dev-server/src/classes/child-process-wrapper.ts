import colors from "picocolors"
import { Logger, ViteDevServer, createLogger } from "vite"
import type { ViteNodeServer } from "vite-node/server"

import { ChildProcess, Serializable, fork } from "node:child_process"
import { Readable, Writable } from "node:stream"

import { byLine, prefix } from "@xen-ilp/lib-itergen-utils"
import { UnreachableCaseError, assertDefined } from "@xen-ilp/lib-type-utils"

import { ClientRequest, schema } from "../schemas/client-request"
import type { NodeDefinition } from "../server"
import RpcHost from "./rpc-host"

const entryPoint = new URL("../dist/runner.js", import.meta.url).pathname

function pipeOutput(input: Readable, output: Writable, prefixString = "") {
  Readable.from(prefix(prefixString)(byLine(input))).pipe(output)
}

export default class ChildProcessWrapper<T> {
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
    this.logger = createLogger("info", {
      prefix: `${colors.green(`[${this.node.id}]`)} `,
    })
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
        this.logger.error(
          `${colors.green(`[${this.node.id}]`)} ${colors.red(
            `child process error: `
          )} ${error}`
        )
        break
      case "stopping":
        this.logger.error(
          `${colors.green(`[${this.node.id}]`)} ${colors.red(
            `child process error while stopping: `
          )} ${error}`
        )
        break
      default:
        throw new UnreachableCaseError(this.state)
    }
  }

  handleChildExit = (code: number | null) => {
    if (code === 0) {
      this.logger.info(
        `${colors.green(`[${this.node.id}]`)} ${colors.green(`child exited`)}`
      )
    } else {
      this.logger.error(
        `${colors.green(`[${this.node.id}]`)} ${colors.red(
          `child exited with code: `
        )} ${code}`
      )
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
      const child = (this.child = fork(entryPoint, [], {
        silent: true,
      }))
      assertDefined(child.stdout)
      assertDefined(child.stderr)

      child.addListener("error", this.handleChildError)
      child.addListener("exit", this.handleChildExit)
      child.addListener("message", this.handleChildMessage)

      const logPrefix = `${colors.cyan(`[${this.node.id}]`)} `
      pipeOutput(child.stdout, process.stdout, logPrefix)
      pipeOutput(child.stderr, process.stderr, logPrefix)

      this.rpcHost.call("start", {
        root: this.viteServer.config.root,
        base: this.viteServer.config.base,
        entry: this.node.entry ?? "src/index.ts",
        config: this.node.config,
      })

      this.startPromise = undefined
    })())
  }

  async stop() {
    if (this.state === "stopping") {
      return this.stopPromise
    } else if (this.state !== "running") {
      throw new Error("runner must be running to stop")
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
        child.send({ id: "exit", method: "exit" })
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
