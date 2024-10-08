import path from "node:path"

import type { Architecture } from "../constants/architectures"
import { PATH_DIST_CONTENTS, PATH_DIST_STAGING } from "../constants/paths"
import type { DassieVersion } from "../constants/version"
import { getBundleName } from "./bundle-name"

export const getStagingPath = (architecture: Architecture) =>
  path.resolve(PATH_DIST_STAGING, architecture)

export const getBundlePath = (
  version: DassieVersion,
  architecture: Architecture,
) =>
  path.resolve(
    PATH_DIST_CONTENTS,
    getBundleName(version, architecture),
    "dassie",
  )
