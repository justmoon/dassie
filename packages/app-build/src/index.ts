import { command, option, restPositionals, run, string } from "cmd-ts"
import { format } from "date-fns"

import { ArchitecturesParameter } from "./cli-options/architectures"
import { SUPPORTED_ARCHITECTURES } from "./constants/architectures"
import { LATEST_DASSIE_VERSION } from "./constants/version"
import { buildBundle } from "./flows/build-bundle"
import { getHeadCommitShort } from "./utils/git"

export const build = async (parameters: string[]) => {
  const commandSpecification = command({
    name: "build",
    description: "Command-line utility for building Dassie",
    version: "1.0.0",
    args: {
      targets: restPositionals({
        type: string,
        displayName: "target",
        description: "One or more build targets",
      }),
      architectures: option({
        type: ArchitecturesParameter,
        long: "architectures",
        short: "a",
        description: "Comma-separated list of architectures to build for",
        defaultValue: () => SUPPORTED_ARCHITECTURES,
        defaultValueIsSerializable: true,
      }),
    },
    handler: async ({ targets, architectures }) => {
      if (targets.length === 0) {
        targets = ["release"]
      }

      for (const target of targets) {
        switch (target) {
          case "release": {
            await buildBundle({
              version: LATEST_DASSIE_VERSION,
              detailedVersion: LATEST_DASSIE_VERSION,
              isMainRelease: true,
              architectures,
            })
            break
          }
          case "canary": {
            await buildBundle({
              version: "canary",
              detailedVersion: `canary-${format(
                new Date(),
                "yyyy-MM-dd"
              )}-${await getHeadCommitShort()}`,
              isMainRelease: false,
              architectures,
            })
            break
          }
          default: {
            throw new Error(`Unknown target: ${target}`)
          }
        }
      }
    },
  })

  await run(commandSpecification, parameters)
}
