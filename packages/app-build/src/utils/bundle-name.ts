import path from "node:path"

import type { Architecture } from "../constants/architectures"
import { PATH_DIST_STAGING, PATH_DIST_UPLOAD } from "../constants/paths"
import type { DassieVersion } from "../constants/version"

export const getBundleName = (
  version: DassieVersion,
  architecture: Architecture,
) => `dassie-${version}-linux-${architecture}`

export const getTarFilename = (
  version: DassieVersion,
  architecture: Architecture,
) => `${getBundleName(version, architecture)}.tar`

export const getTarPath = (
  version: DassieVersion,
  architecture: Architecture,
) => path.resolve(PATH_DIST_STAGING, getTarFilename(version, architecture))

export const getCompressedFilename = (
  version: DassieVersion,
  architecture: Architecture,
  compression: string,
) => `${getTarFilename(version, architecture)}.${compression}`

export const getCompressedPath = (
  version: DassieVersion,
  architecture: Architecture,
  compression: string,
) =>
  path.resolve(
    PATH_DIST_UPLOAD,
    version,
    getCompressedFilename(version, architecture, compression),
  )
