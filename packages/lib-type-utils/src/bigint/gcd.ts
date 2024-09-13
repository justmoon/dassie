/**
 * Computes the greatest common divisor (GCD) of two bigint numbers using the Euclidean algorithm.
 *
 * @param a - The first bigint.
 * @param b - The second bigint.
 * @returns The GCD of a and b.
 */
export function bigIntGcd(a: bigint, b: bigint): bigint {
  while (b !== BigInt(0)) {
    const previousB = b
    b = a % b
    a = previousB
  }
  return a
}
