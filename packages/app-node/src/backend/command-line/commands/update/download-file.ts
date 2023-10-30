import axios from "axios"

import { createWriteStream } from "node:fs"
import { pipeline } from "node:stream/promises"

export const downloadFile = async (
  url: string,
  localDestinationPath: string,
) => {
  const response = await axios({
    method: "get",
    url,
    responseType: "stream",
  })

  const writer = createWriteStream(localDestinationPath)

  await pipeline(response.data, writer)
}
