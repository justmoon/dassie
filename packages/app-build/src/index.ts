import { format } from "date-fns"

import { LATEST_DASSIE_VERSION } from "./constants/version"
import { buildBundle } from "./flows/build-bundle"
import { buildInstaller } from "./flows/build-installer"
import { getHeadCommitShort } from "./utils/git"

export const build = async (target = "release") => {
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
    case "installer": {
      await buildInstaller()
      break
    }
    default: {
      throw new Error(`Unknown target: ${target}`)
    }
  }
}
