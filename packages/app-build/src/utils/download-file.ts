import axios from "axios"

import { createHash } from "node:crypto"
import { createReadStream, createWriteStream } from "node:fs"
import { copyFile, mkdir, unlink } from "node:fs/promises"
import { resolve } from "node:path"
import { pipeline } from "node:stream/promises"

import { VERIFIED_HASHES } from "../constants/hashes"
import { PATH_CACHE } from "../constants/paths"

export const downloadFile = async (
  url: string,
  localDestinationPath: string
) => {
  const hash = VERIFIED_HASHES[url]

  if (!hash) {
    throw new Error(`No trusted hash available for URL: ${url}`)
  }

  await mkdir(PATH_CACHE, { recursive: true })

  const cacheFilePath = resolve(PATH_CACHE, hash)
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

  const response = await axios({
    method: "get",
    url,
    responseType: "stream",
  })

  const writer = createWriteStream(cacheFilePath)

  await pipeline(response.data, writer)

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
