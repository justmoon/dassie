import { $ } from "execa"

import { mkdir } from "node:fs/promises"
import path from "node:path"

import { Architecture } from "../constants/architectures"
import { BETTER_SQLITE3_VERSION, NODE_ABI_VERSION } from "../constants/version"
import { downloadFile } from "../utils/download-file"
import { getStagingPath } from "../utils/dynamic-paths"

export const getBetterSqliteDownloadUrl = (
  version: string,
  nodeAbiVersion: string,
  architecture: SqliteArchitecture,
) =>
  `https://github.com/WiseLibs/better-sqlite3/releases/download/v${version}/better-sqlite3-v${version}-node-v${nodeAbiVersion}-linux-${architecture}.tar.gz`

type SqliteArchitecture = "x64" | "arm" | "arm64"

export const SQLITE_ARCHITECTURE_MAP: {
  [key in Architecture]: SqliteArchitecture
} = {
  armv7l: "arm",
  arm64: "arm64",
  x64: "x64",
}

export const downloadBetterSqlite3 = async (architecture: Architecture) => {
  const sqliteArchitecture = SQLITE_ARCHITECTURE_MAP[architecture]
  const pathSqlite = path.resolve(
    getStagingPath(architecture),
    "better-sqlite3",
  )
  const downloadUrl = getBetterSqliteDownloadUrl(
    BETTER_SQLITE3_VERSION,
    NODE_ABI_VERSION,
    sqliteArchitecture,
  )
  const sqliteLocalFile = path.resolve(
    pathSqlite,
    path.basename(new URL(downloadUrl).pathname),
  )

  await mkdir(pathSqlite, { recursive: true })

  await downloadFile(downloadUrl, sqliteLocalFile)

  await $`tar -xzf ${sqliteLocalFile} -C ${pathSqlite} --strip-components=1`
}
