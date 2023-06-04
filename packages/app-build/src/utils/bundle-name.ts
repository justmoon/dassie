import { DASSIE_VERSION } from "../constants/version"

export const getBundleName = (architecture: string) =>
  `dassie-${DASSIE_VERSION}-linux-${architecture}`

export const getBundleFilename = (architecture: string) =>
  `${getBundleName(architecture)}.tar.xz`
