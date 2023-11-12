/**
 * Returns the largest of a set of provided bigint values.
 *
 * @param first - The first bigint is required because there would be no maximum with zero parameters.
 * @param rest - Additional bigints to compare.
 * @returns The largest of all provided bigints.
 */
export const bigIntMax = (first: bigint, ...rest: readonly bigint[]) => {
  let currentMax = first
  for (const nextValue of rest) {
    if (nextValue > currentMax) {
      currentMax = nextValue
    }
  }
  return currentMax
}

/**
 * Returns the smallest of a set of provided bigint values.
 *
 * @param first - The first bigint is required because there would be no minimum with zero parameters.
 * @param rest - Additional bigints to compare.
 * @returns The smallest of all provided bigints.
 */
export const bigIntMin = (first: bigint, ...rest: readonly bigint[]) => {
  let currentMin = first
  for (const nextValue of rest) {
    if (nextValue < currentMin) {
      currentMin = nextValue
    }
  }
  return currentMin
}
