import { mkdir } from "node:fs/promises"
import { dirname } from "node:path"

import { UnreachableCaseError } from "@dassie/lib-type-utils"

import { Architecture } from "../constants/architectures"
import { Compression } from "../constants/compression"
import { getCompressedPath, getTarPath } from "../utils/bundle-name"
import { run } from "../utils/run"

export const compressBundle = async (
  architecture: Architecture,
  compression: Compression
) => {
  const tarFile = getTarPath(architecture)
  const compressedFile = getCompressedPath(architecture, compression)

  // Make directory for compressed file
  await mkdir(dirname(compressedFile), { recursive: true })

  switch (compression) {
    case "gz": {
      await run`gzip -f ${tarFile} -c > ${compressedFile}`
      break
    }

    case "xz": {
      await run`xz -f ${tarFile} -c > ${compressedFile}`
      break
    }

    default: {
      throw new UnreachableCaseError(compression)
    }
  }
}
