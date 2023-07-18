import { binary, run, subcommands } from "cmd-ts"

import process from "node:process"

import { Reactor } from "@dassie/lib-reactive"

import { daemonCommand } from "./commands/daemon"
import { initCommand } from "./commands/init"
import { updateCommand } from "./commands/update"
import { verifyInstallCommand } from "./commands/verify-install"

export const main = async (reactor: Reactor) => {
  const rootCommand = subcommands({
    name: "dassie",
    cmds: {
      init: initCommand(reactor),
      "verify-install": verifyInstallCommand,
      daemon: daemonCommand,
      update: updateCommand(reactor),
    },
  })

  const binaryCommand = binary(rootCommand)
  await run(binaryCommand, process.argv)
}
