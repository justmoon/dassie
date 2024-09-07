import { binary, run, subcommands } from "cmd-ts"

import process from "node:process"

import type { Reactor } from "@dassie/lib-reactive"

import { DaemonCommand } from "./commands/daemon"
import { InitCommand } from "./commands/init"
import { UpdateCommand } from "./commands/update"
import { VerifyInstallCommand } from "./commands/verify-install"

export const main = async (reactor: Reactor) => {
  const rootCommand = subcommands({
    name: "dassie",
    cmds: {
      init: reactor.use(InitCommand),
      "verify-install": reactor.use(VerifyInstallCommand),
      daemon: reactor.use(DaemonCommand),
      update: reactor.use(UpdateCommand),
    },
  })

  const binaryCommand = binary(rootCommand)
  await run(binaryCommand, process.argv)
}
