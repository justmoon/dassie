import { createHash } from "node:crypto"
import { createReadStream, createWriteStream } from "node:fs"
import { copyFile, mkdir, unlink } from "node:fs/promises"
import path from "node:path"
import { Readable } from "node:stream"
import { pipeline } from "node:stream/promises"
import type { ReadableStream } from "node:stream/web"

import { VERIFIED_HASHES } from "../constants/hashes"
import { PATH_CACHE } from "../constants/paths"

export const downloadFile = async (
  url: string,
  localDestinationPath: string,
) => {
  const hash = VERIFIED_HASHES[url]

  if (!hash) {
    throw new Error(`No trusted hash available for URL: ${url}`)
  }

  const predownloadPath = process.env["DASSIE_BUILD_BINARY_DEPENDENCIES_PATH"]
  if (predownloadPath) {
    const predownloadFilePath = path.resolve(
      predownloadPath,
      path.basename(url),
    )

    try {
      const hasher = createHash("sha256")
      await pipeline(createReadStream(predownloadFilePath), hasher)
      const hashResult = hasher.digest("hex")

      if (hashResult === hash) {
        await copyFile(predownloadFilePath, localDestinationPath)
        return
      }
    } catch {
      // Ignore errors
    }
  }

  await mkdir(PATH_CACHE, { recursive: true })

  const cacheFilePath = path.resolve(PATH_CACHE, hash)
  try {
    const hasher = createHash("sha256")
    await pipeline(createReadStream(cacheFilePath), hasher)
    const hashResult = hasher.digest("hex")

    if (hashResult === hash) {
      await copyFile(cacheFilePath, localDestinationPath)
      return
    }
  } catch {
    // Ignore errors
  }

  const response = await fetch(url)

  const writer = createWriteStream(cacheFilePath)

  if (!response.body) {
    throw new Error("Response body is empty")
  }

  await pipeline(Readable.fromWeb(response.body as ReadableStream), writer)

  {
    const hasher = createHash("sha256")
    await pipeline(createReadStream(cacheFilePath), hasher)
    const hashResult = hasher.digest("hex")

    if (hashResult !== hash) {
      await unlink(cacheFilePath)
      throw new Error(`File from ${url} did not match expected hash ${hash}`)
    }
  }

  await copyFile(cacheFilePath, localDestinationPath)
}
