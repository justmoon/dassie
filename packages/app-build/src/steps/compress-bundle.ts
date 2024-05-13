import { $ } from "execa"

import { mkdir } from "node:fs/promises"
import { dirname } from "node:path"

import { UnreachableCaseError } from "@dassie/lib-type-utils"

import { Architecture } from "../constants/architectures"
import { Compression } from "../constants/compression"
import { DassieVersion } from "../constants/version"
import { getCompressedPath, getTarPath } from "../utils/bundle-name"

export const compressBundle = async (
  version: DassieVersion,
  architecture: Architecture,
  compression: Compression,
) => {
  const tarFile = getTarPath(version, architecture)
  const compressedFile = getCompressedPath(version, architecture, compression)

  // Make directory for compressed file
  await mkdir(dirname(compressedFile), { recursive: true })

  switch (compression) {
    case "gz": {
      await $({ stdout: { file: compressedFile } })`gzip -f ${tarFile} -c`
      break
    }

    case "xz": {
      await $({
        stdout: { file: compressedFile },
      })`xz -T 0 -f ${tarFile} -c`
      break
    }

    default: {
      throw new UnreachableCaseError(compression)
    }
  }
}
