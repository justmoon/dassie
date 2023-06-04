import { resolve } from "node:path"
import { arch } from "node:process"

import { VERSION } from "./version"

export const PATH_DIST = new URL("../../dist", import.meta.url).pathname
export const PATH_DIST_BUNDLE = resolve(
  PATH_DIST,
  `bundle/dassie-${VERSION}-linux-${arch}`
)

export const PATH_PACKAGE_APP_BUILD = new URL("../../", import.meta.url)
  .pathname
export const PATH_PACKAGE_APP_NODE = new URL(
  "../../../app-node",
  import.meta.url
).pathname

export const PATH_NVMRC = new URL("../../../../.nvmrc", import.meta.url)

export const PATH_RESOURCES = resolve(PATH_PACKAGE_APP_BUILD, "resources")
