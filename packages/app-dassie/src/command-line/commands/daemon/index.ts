import { command, oneOf, option } from "cmd-ts"

import { createReactor } from "@dassie/lib-reactive"

import { NodejsDaemonActor } from "./nodejs"
import { NodejsSystemdDaemonActor } from "./nodejs-systemd"

const RUNTIMES = {
  nodejs: NodejsDaemonActor,
  "nodejs-systemd": NodejsSystemdDaemonActor,
} as const

const RUNTIME_OPTIONS = Object.keys(RUNTIMES) as (keyof typeof RUNTIMES)[]

export const DaemonCommand = () =>
  command({
    name: "daemon",
    description:
      "This command starts the Dassie daemon in the foreground. It is intended to be used by a process manager like systemd.",
    args: {
      runtime: option({
        type: oneOf(RUNTIME_OPTIONS),
        long: "runtime",
        description: "The runtime to use",
        defaultValue: () => "nodejs" as const,
      }),
    },
    handler({ runtime }) {
      createReactor(RUNTIMES[runtime])
    },
  })
