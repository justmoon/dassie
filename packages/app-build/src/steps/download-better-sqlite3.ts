import { $ } from "execa"

import { mkdir } from "node:fs/promises"
import { basename, resolve } from "node:path"

import { Architecture } from "../constants/architectures"
import { BETTER_SQLITE3_VERSION, NODE_ABI_VERSION } from "../constants/version"
import { downloadFile } from "../utils/download-file"
import { getStagingPath } from "../utils/dynamic-paths"

const getDownloadUrl = (
  version: string,
  nodeAbiVersion: string,
  architecture: SqliteArchitecture
) =>
  `https://github.com/WiseLibs/better-sqlite3/releases/download/v${version}/better-sqlite3-v${version}-node-v${nodeAbiVersion}-linux-${architecture}.tar.gz`

type SqliteArchitecture = "x64" | "arm" | "arm64"

const ARCHITECTURE_MAP: { [key in Architecture]: SqliteArchitecture } = {
  armv7l: "arm",
  arm64: "arm64",
  x64: "x64",
}

export const downloadBetterSqlite3 = async (architecture: Architecture) => {
  const sqliteArchitecture = ARCHITECTURE_MAP[architecture]
  const pathSqlite = resolve(getStagingPath(architecture), "better-sqlite3")
  const downloadUrl = getDownloadUrl(
    BETTER_SQLITE3_VERSION,
    NODE_ABI_VERSION,
    sqliteArchitecture
  )
  const sqliteLocalFile = resolve(
    pathSqlite,
    basename(new URL(downloadUrl).pathname)
  )

  await mkdir(pathSqlite, { recursive: true })

  await downloadFile(downloadUrl, sqliteLocalFile)

  await $`tar -xzf ${sqliteLocalFile} -C ${pathSqlite} --strip-components=1`
}
