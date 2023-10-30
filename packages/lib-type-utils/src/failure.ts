/**
 * Globally unique identifier for the Failure class pattern.
 *
 * We are using a marker instead of instanceof to avoid issues with multiple JavaScript realms or multiple copies of this library. We are using a string instead of Symbol.for because TypeScript doesn't support symbol literals at the time of writing.
 */
export const FAILURE_UNIQUE_KEY = "dassie.failure"

/**
 * A class that represents a failure.
 *
 * @remarks
 *
 * You can create a subclass to represent your various failure types. If your failure doesn't have any parameters, you can also create a constant containing an instance of this class to avoid creating a new instance every time a failure occurs.
 *
 * @example
 *
 * class InvalidAccountError extends Failure \{ \}
 *
 * const INVALID_ACCOUNT_ERROR = new InvalidAccountError()
 *
 */
export abstract class Failure {
  [FAILURE_UNIQUE_KEY] = true

  abstract readonly name: string
}

/**
 * Check if something is a failure.
 *
 * @remarks
 *
 * To handle failures, you would first check if a result is a failure and then handle different types by switching on the `name` property.
 *
 * @example
 *
 * const result = doSomething()
 *
 * if (isFailure(result)) \{
 *   switch (result.name) \{
 *     case "InvalidAccountError": \{
 *       // handle invalid account
 *       break
 *     \}
 *     case "InvalidTransactionError": \{
 *       // handle invalid transaction
 *       break
 *     \}
 *   \}
 * \}
 */
export const isFailure = <T>(value: T): value is Extract<T, Failure> => {
  return Boolean(
    (value as null | { [FAILURE_UNIQUE_KEY]?: true })?.[FAILURE_UNIQUE_KEY],
  )
}
