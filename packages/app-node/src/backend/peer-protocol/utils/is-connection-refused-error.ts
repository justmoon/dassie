import { isError, isErrorWithCode } from "@dassie/lib-type-utils"

export function isConnectionRefusedError(error: unknown): error is TypeError {
  return (
    isError(error) &&
    error.name === "TypeError" &&
    isError(error.cause) &&
    error.cause.name === "AggregateError" &&
    "errors" in error.cause &&
    Array.isArray(error.cause.errors) &&
    error.cause.errors.every((cause) => isErrorWithCode(cause, "ECONNREFUSED"))
  )
}
