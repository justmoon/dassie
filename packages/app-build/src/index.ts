import { LATEST_DASSIE_VERSION } from "./constants/version"
import { buildBundle } from "./flows/build-bundle"
import { buildInstaller } from "./flows/build-installer"

export const build = async (target = "release") => {
  switch (target) {
    case "release": {
      await buildBundle({
        version: LATEST_DASSIE_VERSION,
        isMainRelease: true,
      })
      break
    }
    case "canary": {
      await buildBundle({
        version: "canary",
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

export default build
