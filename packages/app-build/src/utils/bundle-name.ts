import { resolve } from "node:path"

import { Architecture } from "../constants/architectures"
import { PATH_DIST_STAGING, PATH_DIST_UPLOAD } from "../constants/paths"
import { DASSIE_VERSION } from "../constants/version"

export const getBundleName = (architecture: Architecture) =>
  `dassie-${DASSIE_VERSION}-linux-${architecture}`

export const getTarFilename = (architecture: Architecture) =>
  `${getBundleName(architecture)}.tar`

export const getTarPath = (architecture: Architecture) =>
  resolve(PATH_DIST_STAGING, `${getTarFilename(architecture)}`)

export const getCompressedFilename = (
  architecture: Architecture,
  compression: string
) => `${getTarFilename(architecture)}.${compression}`

export const getCompressedPath = (
  architecture: Architecture,
  compression: string
) =>
  resolve(
    PATH_DIST_UPLOAD,
    DASSIE_VERSION,
    `${getCompressedFilename(architecture, compression)}`
  )
