import { copyFile, mkdir, readFile } from "node:fs/promises"
import { resolve } from "node:path"
import { arch } from "node:process"

import { PATH_DIST, PATH_DIST_BUNDLE, PATH_NVMRC } from "../constants/paths"
import { downloadFile } from "../utils/download-file"
import { run } from "../utils/run"

const getNodeUrl = (version: string) =>
  `https://nodejs.org/dist/${version}/node-${version}-linux-${arch}.tar.xz`

export const downloadNodeJs = async () => {
  // Read node version from .nvmrc
  const nvmrcFileContents = await readFile(PATH_NVMRC, "utf8")
  const nodeVersion = nvmrcFileContents.trim()

  const pathNode = resolve(PATH_DIST, "node")
  const nodeLocalFile = resolve(pathNode, `node-${nodeVersion}.tar.xz`)

  await mkdir(pathNode, { recursive: true })

  await downloadFile(getNodeUrl(nodeVersion), nodeLocalFile)

  await run`tar -xJf ${nodeLocalFile} -C ${pathNode} --strip-components=1`

  const pathBundleBin = resolve(PATH_DIST_BUNDLE, "bin")
  const pathBundleBinNode = resolve(PATH_DIST_BUNDLE, "bin/node")

  await mkdir(pathBundleBin, { recursive: true })

  await copyFile(resolve(pathNode, "bin/node"), pathBundleBinNode)
}
