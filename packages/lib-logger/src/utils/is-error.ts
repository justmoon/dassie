import { isNativeError } from "node:util/types"

export default function isError(error: unknown): error is Error {
  return error instanceof Error || isNativeError(error)
}
