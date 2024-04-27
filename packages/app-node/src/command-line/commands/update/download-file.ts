import { createWriteStream } from "node:fs"
import { Readable } from "node:stream"
import { pipeline } from "node:stream/promises"
import type { ReadableStream } from "node:stream/web"

export const downloadFile = async (
  url: string,
  localDestinationPath: string,
) => {
  const response = await fetch(url)

  const writeStream = createWriteStream(localDestinationPath)

  if (!response.body) {
    throw new Error("Response body is empty")
  }

  await pipeline(Readable.fromWeb(response.body as ReadableStream), writeStream)
}
