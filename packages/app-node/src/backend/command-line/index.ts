import { binary, run, subcommands } from "cmd-ts"

import process from "node:process"

import verifyInstallCommand from "./commands/verify-install"

export const main = async () => {
  const rootCommand = subcommands({
    name: "dassie",
    cmds: { "verify-install": verifyInstallCommand },
  })

  const binaryCommand = binary(rootCommand)
  await run(binaryCommand, process.argv)
}
