import { Architecture } from "../constants/architectures"
import { DASSIE_VERSION } from "../constants/version"

export const getBundleName = (architecture: Architecture) =>
  `dassie-${DASSIE_VERSION}-linux-${architecture}`

export const getBundleFilename = (architecture: Architecture) =>
  `${getBundleName(architecture)}.tar.xz`
