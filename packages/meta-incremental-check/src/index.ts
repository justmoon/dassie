#!/usr/bin/env node
import {
  createFlow,
  header,
  note,
  tasklist,
} from "@dassie/lib-terminal-graphics"

import { runEslint } from "./eslint"
import { type PackagesToBeLinted, runTypeScriptCompiler } from "./typescript"

export async function runChecks() {
  const flow = createFlow()

  flow.show(header({ title: "Dassie Check" }))

  flow.show(note({ title: `Running TypeScript compiler...` }))

  let packagesToBeLinted!: PackagesToBeLinted
  await flow.attach(tasklist({}), (state) => {
    packagesToBeLinted = runTypeScriptCompiler(
      "./",
      (packageName, status) => {
        if (status === "start") {
          state.addTask(packageName, {
            description: `Compiling ${packageName}`,
            progress: "indeterminate",
          })
        } else {
          state.updateTask(packageName, (task) => ({
            ...task,
            progress: status === "error" ? "error" : "done",
          }))
        }
      },
      (message) => flow.show(message),
    )

    return Promise.resolve()
  })

  if (process.exitCode === 0 && packagesToBeLinted.length > 0) {
    flow.show(note({ title: `Running ESLint...` }))

    await flow.attach(tasklist({}), async (state) => {
      await runEslint(
        packagesToBeLinted,
        (packageName, status) => {
          if (status === "start") {
            state.addTask(packageName, {
              description: `Linting ${packageName}`,
              progress: "indeterminate",
            })
          } else {
            state.updateTask(packageName, (task) => ({
              ...task,
              progress: status === "error" ? "error" : "done",
            }))
          }
        },
        (message) => flow.show(message),
      )
    })
  }
}
