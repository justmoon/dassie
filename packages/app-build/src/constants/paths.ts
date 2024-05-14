import path from "node:path"

export const PATH_PACKAGE_APP_BUILD = new URL("../../", import.meta.url)
  .pathname
export const PATH_PACKAGE_APP_NODE = new URL(
  "../../../app-node/",
  import.meta.url,
).pathname

export const PATH_DIST = path.resolve(PATH_PACKAGE_APP_BUILD, "../../dist")
export const PATH_DIST_UPLOAD = path.resolve(PATH_DIST, "upload")
export const PATH_DIST_CONTENTS = path.resolve(PATH_DIST, "contents")
export const PATH_DIST_STAGING = path.resolve(PATH_DIST, "staging")
export const PATH_DIST_STAGING_SHARED = path.resolve(
  PATH_DIST_STAGING,
  "shared",
)

export const PATH_PACKAGE_LIB_SQLITE = new URL(
  "../../../lib-sqlite/",
  import.meta.url,
).pathname

export const PATH_RESOURCES = path.resolve(PATH_PACKAGE_APP_BUILD, "resources")
export const PATH_RESOURCES_SYSTEMD = path.resolve(PATH_RESOURCES, "systemd")
export const PATH_RESOURCES_LAUNCHER = path.resolve(
  PATH_RESOURCES,
  "launcher/dassie.sh",
)
export const PATH_RESOURCES_INSTALLER = path.resolve(
  PATH_RESOURCES,
  "shell-installer/install.sh",
)

export const PATH_CACHE = path.resolve(PATH_PACKAGE_APP_BUILD, ".cache")
