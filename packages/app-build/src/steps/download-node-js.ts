import { $ } from "execa"

import { mkdir } from "node:fs/promises"
import { resolve } from "node:path"

import { Architecture } from "../constants/architectures"
import { NODE_VERSION } from "../constants/version"
import { downloadFile } from "../utils/download-file"
import { getStagingPath } from "../utils/dynamic-paths"

const getNodeUrl = (version: string, architecture: Architecture) =>
  `https://nodejs.org/dist/v${version}/node-v${version}-linux-${architecture}.tar.xz`

export const downloadNodeJs = async (architecture: Architecture) => {
  const pathNode = resolve(getStagingPath(architecture), "node")
  const nodeLocalFile = resolve(
    pathNode,
    `node-v${NODE_VERSION}-linux-${architecture}.tar.xz`,
  )

  await mkdir(pathNode, { recursive: true })

  await downloadFile(getNodeUrl(NODE_VERSION, architecture), nodeLocalFile)

  await $`tar -xJf ${nodeLocalFile} -C ${pathNode} --strip-components=1`
}
