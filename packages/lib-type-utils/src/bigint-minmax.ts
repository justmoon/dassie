/* eslint-disable unicorn/no-array-reduce */

/**
 * Returns the largest of a set of provided bigint values.
 *
 * @param first - The first bigint is required because there would be no maximum with zero parameters.
 * @param rest - Additional bigints to compare.
 * @returns The largest of all provided bigints.
 */
export const bigIntMax = (first: bigint, ...rest: readonly bigint[]) =>
  rest.reduce(
    (currentMax, nextValue) =>
      nextValue > currentMax ? nextValue : currentMax,
    first,
  )

/**
 * Returns the smallest of a set of provided bigint values.
 *
 * @param first - The first bigint is required because there would be no minimum with zero parameters.
 * @param rest - Additional bigints to compare.
 * @returns The smallest of all provided bigints.
 */
export const bigIntMin = (first: bigint, ...rest: readonly bigint[]) =>
  rest.reduce(
    (currentMin, nextValue) =>
      nextValue < currentMin ? nextValue : currentMin,
    first,
  )
