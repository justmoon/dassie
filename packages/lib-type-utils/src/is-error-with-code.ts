import { isObject } from "./is-object"

export function isErrorWithCode<T extends string>(
  error: unknown,
  code: T
): error is NodeJS.ErrnoException {
  return (
    isObject(error) &&
    typeof error["code"] === "string" &&
    error["code"] === code
  )
}
