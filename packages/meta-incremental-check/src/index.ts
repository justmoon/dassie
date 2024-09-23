import path from "node:path"
import { MessageChannel, Worker } from "node:worker_threads"

import {
  createClient,
  createNodejsMessagePortLink,
} from "@dassie/lib-rpc/client"
import {
  createFlow,
  header,
  note,
  tasklist,
} from "@dassie/lib-terminal-graphics"

import type { RpcRouter } from "./entry/worker"
import type { ProgressMessage } from "./utils/report-status"

export async function runChecks() {
  const worker = new Worker(
    path.resolve(new URL(import.meta.url).pathname, "../scripts/check.js"),
  )

  const rpcChannel = new MessageChannel()

  const rpcClient = createClient<RpcRouter>({
    connection: createNodejsMessagePortLink(rpcChannel.port1),
  })

  worker.postMessage(rpcChannel.port2, [rpcChannel.port2])

  const flow = createFlow()

  flow.show(header({ title: "Dassie Check" }))

  flow.show(note({ title: `Running TypeScript compiler...` }))

  await flow.attach(tasklist({}), async (state) => {
    state.act.addTask("meta/search-stale", {
      description: "Searching for stale packages",
      progress: "indeterminate",
    })

    function markStalePackagesDone() {
      state.act.updateTask("meta/search-stale", (task) =>
        task.progress === "done" ?
          task
        : {
            ...task,
            progress: "done",
          },
      )
    }

    function handleCheckerEvent(event: unknown) {
      if (Array.isArray(event)) {
        const progressMessage = event as ProgressMessage

        switch (progressMessage[0]) {
          case "print": {
            const [, message] = progressMessage

            flow.show(message)
            break
          }
          case "status": {
            const [, packageName, status] = progressMessage

            markStalePackagesDone()

            if (status === "start") {
              state.act.addTask(packageName, {
                description: `Compiling ${packageName}`,
                progress: "indeterminate",
              })
            } else {
              state.act.updateTask(packageName, {
                progress: status === "error" ? "error" : "done",
              })
            }
            break
          }
        }
      }
    }
    worker.on("message", handleCheckerEvent)
    const result = await rpcClient.rpc.runTypeScriptCompiler.mutate(".")

    markStalePackagesDone()

    process.exitCode = result

    worker.off("message", handleCheckerEvent)
  })

  if (process.exitCode === 0) {
    flow.show(note({ title: `Running ESLint...` }))

    await flow.attach(tasklist({}), async (state) => {
      state.act.addTask("meta/search-stale", {
        description: "Searching for stale packages",
        progress: "indeterminate",
      })

      function markStalePackagesDone() {
        state.act.updateTask("meta/search-stale", (task) =>
          task.progress === "done" ?
            task
          : {
              ...task,
              progress: "done",
            },
        )
      }

      function handleCheckerEvent(event: unknown) {
        if (Array.isArray(event)) {
          const progressMessage = event as ProgressMessage

          switch (progressMessage[0]) {
            case "print": {
              const [, message] = progressMessage

              flow.show(message)
              break
            }
            case "status": {
              const [, packageName, status] = progressMessage

              markStalePackagesDone()

              if (status === "start") {
                state.act.addTask(packageName, {
                  description: `Linting ${packageName}`,
                  progress: "indeterminate",
                })
              } else {
                state.act.updateTask(packageName, {
                  progress: status === "error" ? "error" : "done",
                })
              }
              break
            }
          }
        }
      }

      worker.on("message", handleCheckerEvent)
      const hasErrors = await rpcClient.rpc.runEslint.mutate()
      worker.off("message", handleCheckerEvent)

      if (hasErrors) {
        process.exitCode = 1
      }
    })
  }

  rpcClient.close()
  await worker.terminate()
}
