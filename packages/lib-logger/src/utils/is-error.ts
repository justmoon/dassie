/**
 * Check if a value is an error object.
 *
 * @remarks
 *
 * This function first checks the value using the instanceof operator. This is the fastest check, but can fail when the error object was created in a different JavaScript context. So we also check using the `toString()` method. This relies on the ECMAScript standard's requirement that error objects must return "[object Error]" when converted to a string.
 *
 * @see https://stackoverflow.com/a/61958148
 * @see https://262.ecma-international.org/5.1/#sec-15.2.4.2
 */
export function isError(possibleError: unknown): possibleError is Error {
  return (
    possibleError instanceof Error ||
    Object.prototype.toString.call(possibleError) === "[object Error]"
  )
}

export const hasAggregatedErrors = (
  error: Error
): error is Error & { errors: Error[] } => {
  return (
    error instanceof AggregateError ||
    ("errors" in error &&
      Array.isArray(error.errors) &&
      error.errors.every((error) => isError(error)))
  )
}
