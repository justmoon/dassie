import { command, restPositionals, run, string } from "cmd-ts"
import { format } from "date-fns"

import { LATEST_DASSIE_VERSION } from "./constants/version"
import { buildBundle } from "./flows/build-bundle"
import { getHeadCommitShort } from "./utils/git"

export const build = async (parameters: string[]) => {
  const commandSpecification = command({
    name: "build",
    description: "Command-line utility for building Dassie",
    version: "1.0.0",
    args: {
      targets: restPositionals({ type: string, displayName: "target" }),
    },
    handler: async ({ targets }) => {
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
