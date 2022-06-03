import { ChildProcess, fork } from "node:child_process"
import type { InputConfig } from "../../../config"
import { Readable, Writable } from "node:stream"
import split from "../../../utils/stream-by-line"
import prefix from "../../../utils/stream-prefix"
import colors from "picocolors"
import type DevRpcHost from "./dev-rpc-host"
import { createLogger, Logger, ViteDevServer } from "vite"

const entryPoint = new URL("../dev-runner.js", import.meta.url).pathname

function pipeOutput(
  input: Readable,
  output: Writable,
  prefixString: string = ""
) {
  Readable.from(prefix(prefixString)(split(input))).pipe(output)
}

export default class DevProcess {
  private state: "idle" | "running" | "stopping" = "idle"
  private child: ChildProcess | undefined
  private startPromise: Promise<void> | undefined
  private stopPromise: Promise<void> | undefined
  private logger: Logger

  constructor(
    readonly nodeId: string,
    private readonly nodeConfig: InputConfig,
    private readonly viteServer: ViteDevServer,
    private readonly rpcHost: DevRpcHost
  ) {
    this.logger = createLogger("info", {
      prefix: `${colors.green(`[${this.nodeId}]`)} `,
    })
  }

  get url() {
    return `https://${this.nodeId}.localhost:${this.nodeConfig.port || 8443}/`
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
          `${colors.green(`[${this.nodeId}]`)} ${colors.red(
            `child process error: `
          )} ${error}`
        )
        break
      case "stopping":
        this.logger.error(
          `${colors.green(`[${this.nodeId}]`)} ${colors.red(
            `child process error while stopping: `
          )} ${error}`
        )
        break
    }
  }

  handleChildExit = () => {
    this.state = "idle"
    this.child = undefined
  }

  handleChildMessage = async (message: Record<string, unknown>) => {
    try {
      this.child!.send({
        id: message["id"],
        result: await this.rpcHost.call(message),
      })
    } catch (err) {
      this.child!.send({
        id: message["id"],
        error: {
          code: 0,
          message: String(err),
        },
      })
    }
  }

  async start() {
    if (this.state === "running") {
      return this.startPromise
    } else if (this.state !== "idle") {
      throw new Error("runner must be idle to start")
    }

    this.state = "running"

    return (this.startPromise = (async () => {
      const child = (this.child = fork(entryPoint, [], {
        silent: true,
      }))
      child.addListener("error", this.handleChildError)
      child.addListener("exit", this.handleChildExit)
      child.addListener("message", this.handleChildMessage)

      const logPrefix = `${colors.cyan(`[${this.nodeId}]`)} `
      pipeOutput(this.child.stdout!, process.stdout, logPrefix)
      pipeOutput(this.child.stderr!, process.stderr, logPrefix)

      child.send({
        id: "start",
        method: "start",
        params: {
          root: this.viteServer.config.root,
          base: this.viteServer.config.base,
          entry: "./src/index.ts",
          config: this.nodeConfig,
        },
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
