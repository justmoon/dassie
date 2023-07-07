import { resolve } from "node:path"

export const PATH_PACKAGE_APP_BUILD = new URL("../../", import.meta.url)
  .pathname
export const PATH_PACKAGE_APP_NODE = new URL(
  "../../../app-node/",
  import.meta.url
).pathname

export const PATH_DIST = resolve(PATH_PACKAGE_APP_BUILD, "dist")
export const PATH_DIST_UPLOAD = resolve(PATH_DIST, "upload")
export const PATH_DIST_CONTENTS = resolve(PATH_DIST, "contents")
export const PATH_DIST_STAGING = resolve(PATH_DIST, "staging")
export const PATH_DIST_STAGING_SHARED = resolve(PATH_DIST_STAGING, "shared")

export const PATH_PACKAGE_LIB_SQLITE = new URL(
  "../../../lib-sqlite/",
  import.meta.url
).pathname

export const PATH_RESOURCES = resolve(PATH_PACKAGE_APP_BUILD, "resources")
export const PATH_RESOURCES_SYSTEMD = resolve(PATH_RESOURCES, "systemd")
export const PATH_RESOURCES_LAUNCHER = resolve(
  PATH_RESOURCES,
  "launcher/dassie.sh"
)
export const PATH_RESOURCES_INSTALLER = resolve(
  PATH_RESOURCES,
  "shell-installer/install.sh"
)

export const PATH_CACHE = resolve(PATH_PACKAGE_APP_BUILD, ".cache")
