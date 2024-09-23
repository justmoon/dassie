import { stat } from "node:fs/promises"

export async function getFileDate(filePath: string) {
  const fileInfo = await stat(filePath).then(
    (result) => result,
    () => undefined,
  )

  if (!fileInfo) return undefined

  return Number(fileInfo.mtime)
}
