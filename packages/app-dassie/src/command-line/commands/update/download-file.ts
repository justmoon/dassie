import { createWriteStream } from "node:fs"
import { Readable } from "node:stream"
import { pipeline } from "node:stream/promises"

export const downloadFile = async (
  url: string,
  localDestinationPath: string,
) => {
  const response = await fetch(url)

  const writeStream = createWriteStream(localDestinationPath)

  if (!response.body) {
    throw new Error("Response body is empty")
  }

  // eslint-disable-next-line n/no-unsupported-features/node-builtins
  await pipeline(Readable.fromWeb(response.body), writeStream)
}
