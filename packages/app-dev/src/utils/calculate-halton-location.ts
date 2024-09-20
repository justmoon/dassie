/**
 * Generate a virtual node geo-location based on its index.
 *
 * This uses the Halton sequence to generate a pseudo-random sequence of points.
 */
const HALTON_BASE_1 = 2
const HALTON_BASE_2 = 3

const radicalInverse = (base: number, index: number) => {
  let result = 0
  let fraction = 1 / base

  while (index > 0) {
    result += (index % base) * fraction
    index = Math.floor(index / base)
    fraction /= base
  }

  return result
}

export const calculateHaltonLocation = (pointIndex: number) => {
  if (!Number.isInteger(pointIndex)) {
    throw new TypeError(`pointIndex must be an integer, got ${pointIndex}`)
  } else if (pointIndex <= 0) {
    throw new Error(`pointIndex must be positive, got ${pointIndex}`)
  }

  const latitude = Math.asin(2 * radicalInverse(HALTON_BASE_1, pointIndex) - 1)
  const longitude =
    (0.5 - radicalInverse(HALTON_BASE_2, pointIndex)) * 2 * Math.PI

  return { latitude, longitude }
}
