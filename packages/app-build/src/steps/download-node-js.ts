import { $ } from "execa"

import { mkdir } from "node:fs/promises"
import path from "node:path"

import type { Architecture } from "../constants/architectures"
import { NODE_VERSION } from "../constants/version"
import { downloadFile } from "../utils/download-file"
import { getStagingPath } from "../utils/dynamic-paths"

const getNodeUrl = (architecture: Architecture) =>
  `https://nodejs.org/dist/v${NODE_VERSION}/node-v${NODE_VERSION}-linux-${architecture}.tar.xz`

export const downloadNodeJs = async (architecture: Architecture) => {
  const pathNode = path.resolve(getStagingPath(architecture), "node")
  const nodeLocalFile = path.resolve(
    pathNode,
    `node-v${NODE_VERSION}-linux-${architecture}.tar.xz`,
  )

  await mkdir(pathNode, { recursive: true })

  await downloadFile(getNodeUrl(architecture), nodeLocalFile)

  await $`tar -xJf ${nodeLocalFile} -C ${pathNode} --strip-components=1`
}
