import chalk from "chalk"
import type { Promisable } from "type-fest"
import type { ViteNodeServer } from "vite-node/server"

import { ChildProcess, fork } from "node:child_process"
import type { Readable } from "node:stream"

import { byLine } from "@dassie/lib-itergen-utils"
import { createLogger } from "@dassie/lib-logger"
import { createActor } from "@dassie/lib-reactive"
import { assertDefined, isObject } from "@dassie/lib-type-utils"

import { logsStore } from "../../common/stores/logs"
import { createCliOnlyLogger } from "../utils/cli-only-logger"

const logger = createLogger("das:dev:run-child-process")

const RUNNER_MODULE = new URL("../../../dist/runner.js", import.meta.url)
  .pathname
const CHILD_SHUTDOWN_GRACE_PERIOD = 1000

const handleChildError = (error: Error) => {
  logger.error("child process error", { error })
}

interface RunNodeChildProcessProperties {
  nodeServer: ViteNodeServer
  id: string
  environment: Record<string, string> & NodeJS.ProcessEnv
}

export const runChildProcess = () =>
  createActor(
    async (
      sig,
      { nodeServer, id, environment }: RunNodeChildProcessProperties
    ) => {
      let child: ChildProcess | undefined

      const prefix = `◼ ${id}`
      const cliLogger = createCliOnlyLogger(prefix)

      const handleChildExit = (code: number | null) => {
        if (code === 0) {
          logger.info(`${chalk.green(`child exited`)}`)
        } else {
          logger.error(`child exited with code: ${code ?? "unknown"}`)
        }
        child = undefined
      }

      const handleChildMessage = (message: unknown) => {
        void (async () => {
          if (
            isObject(message) &&
            "method" in message &&
            "params" in message &&
            Array.isArray(message["params"])
          ) {
            try {
              let result: unknown
              switch (message["method"]) {
                case "fetchModule": {
                  result = await nodeServer.fetchModule(
                    message["params"][0] as string
                  )
                  break
                }
                case "resolveId": {
                  result = await nodeServer.resolveId(
                    message["params"][0] as string,
                    message["params"][1] as string | undefined
                  )
                  break
                }
              }
              child?.send({
                id: message["id"],
                result,
              })
            } catch (error) {
              logger.error(`child process rpc error`, {
                method: message["method"],
                error,
              })
              child?.send({
                id: message["id"],
                error: String(error),
              })
            }
          } else {
            logger.error(`malformed RPC call from child`, { message })
          }
        })()
      }

      const processLog = async (input: Readable, inputName: string) => {
        for await (const line of byLine(input)) {
          // Suppress annoying node debugger spam
          if (
            line.startsWith("Debugger listening on ") ||
            line === "For help, see: https://nodejs.org/en/docs/inspector" ||
            line === "Debugger attached."
          )
            continue

          sig.use(logsStore).addLogLine({
            node: id,
            component: inputName,
            message: line,
            date: new Date().toISOString(),
            level: "info",
          })
          if (process.env["DASSIE_LOG_TO_CLI"] === "true") {
            cliLogger.info(line)
          }
        }
      }

      const resolvedEntryPoint = await nodeServer.resolveId(RUNNER_MODULE)

      if (!resolvedEntryPoint) {
        throw new Error(`${RUNNER_MODULE} not resolvable`)
      }

      logger.debug("launching child...", { node: id })
      child = fork(resolvedEntryPoint.id, [], {
        detached: false,
        silent: true,
        execArgv: ["--enable-source-maps"],
        env: environment,
      })
      child.addListener("error", handleChildError)
      child.addListener("exit", handleChildExit)
      child.addListener("message", handleChildMessage)

      assertDefined(child.stdout)
      assertDefined(child.stderr)

      processLog(child.stdout, "stdout").catch((error: unknown) =>
        logger.error("error processing child stdout", { node: id, error })
      )
      processLog(child.stderr, "stderr").catch((error: unknown) =>
        logger.error("error processing child stdout", { node: id, error })
      )

      sig.onCleanup((): Promisable<void> => {
        if (child) {
          child.removeListener("exit", handleChildExit)
          child.removeListener("message", handleChildMessage)

          child.disconnect()

          const childReference = child
          return new Promise((resolve) => {
            const handleExit = () => {
              clearTimeout(timer)
              resolve()
            }
            childReference.once("exit", handleExit)
            const timer = setTimeout(() => {
              childReference.off("exit", handleExit)
              childReference.kill("SIGKILL")
              resolve()
            }, CHILD_SHUTDOWN_GRACE_PERIOD)
          })
        }
      })

      // Wait for first message from child to indicate it is ready
      await new Promise((resolve) => child?.once("message", resolve))
    }
  )
