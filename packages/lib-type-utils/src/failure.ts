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
 * ```ts
 * class InvalidAccountFailure extends Failure { }
 *
 * const INVALID_ACCOUNT_FAILURE = new InvalidAccountFailure()
 * ```
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
 * ```ts
 * const result = doSomething()
 *
 * if (isFailure(result)) {
 *   switch (result.name) {
 *     case "InvalidAccountFailure": {
 *       // handle invalid account
 *       break
 *     }
 *     case "InvalidTransactionFailure": {
 *       // handle invalid transaction
 *       break
 *     }
 *   }
 * }
 * ```
 */
export const isFailure = <T>(value: T): value is Extract<T, Failure> => {
  return Boolean(
    (value as null | { [FAILURE_UNIQUE_KEY]?: true })?.[FAILURE_UNIQUE_KEY],
  )
}

export type InferFindFailure<T extends readonly unknown[]> =
  | {
      [K in keyof T]: Exclude<T[K], Failure>
    }
  | {
      [K in keyof T]: Extract<T[K], Failure>
    }[number]

/**
 * Check if any element in an array is a failure.
 *
 * @remarks
 *
 * This is useful for checking if a list of results contains any failures. The first failure that is found is returned. If no failures are found, the original array is returned.
 */
export const findFailure = <T extends readonly unknown[]>(
  values: T,
): InferFindFailure<T> => {
  const firstFailure = values.find(
    // eslint-disable-next-line unicorn/no-array-callback-reference
    isFailure as (value: unknown) => value is InferFindFailure<T>,
  )
  if (firstFailure !== undefined) return firstFailure

  return values as InferFindFailure<T>
}
