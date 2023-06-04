import { resolve } from "node:path"

import { Architecture } from "../constants/architectures"
import { PATH_DIST_BUNDLE, PATH_DIST_STAGING } from "../constants/paths"
import { getBundleName } from "./bundle-name"

export const getStagingPath = (architecture: Architecture) =>
  resolve(PATH_DIST_STAGING, architecture)

export const getBundlePath = (architecture: Architecture) =>
  resolve(PATH_DIST_BUNDLE, getBundleName(architecture), "dassie")
