import { createRequire } from "node:module"
import { versions } from "node:process"

import { PATH_PACKAGE_LIB_SQLITE } from "./paths"

export const LATEST_DASSIE_VERSION = "0.0.1"

export const NODE_VERSION = versions.node

export const NODE_ABI_VERSION = versions.modules

const sqlitePackageJson = createRequire(PATH_PACKAGE_LIB_SQLITE)(
  "better-sqlite3/package.json"
) as { version: string }
export const BETTER_SQLITE3_VERSION = sqlitePackageJson.version

export type DassieVersion = `${number}.${number}.${number}` | "canary"

export type DassieDetailedVersion =
  | `${number}.${number}.${number}`
  | `canary-${string}`
