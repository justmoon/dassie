import { isObject } from "./is-object"

interface ErrnoException extends Error {
  errno?: number | undefined
  code?: string | undefined
  path?: string | undefined
  syscall?: string | undefined
}

export function isErrorWithCode(
  error: unknown,
  code: string,
): error is ErrnoException {
  return (
    isObject(error) &&
    typeof error["code"] === "string" &&
    error["code"] === code
  )
}
