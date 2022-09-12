import { access, constants } from "node:fs/promises"
import { isNativeError } from "node:util/types"

export const checkFileStatus = async (filePath: string) => {
  try {
    await access(filePath, constants.R_OK)
    return "ok"
  } catch (error) {
    const code = isNativeError(error)
      ? (error as NodeJS.ErrnoException).code
      : undefined
    switch (code) {
      case "ENOENT":
        return "missing"
      case "EACCES":
        return "unreadable"
      default:
        return "error"
    }
  }
}
