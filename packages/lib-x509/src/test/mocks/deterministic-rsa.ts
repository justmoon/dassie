import { concatUint8Arrays, hexToUint8Array } from "uint8array-extras"

import {
  type Crypto,
  createMockDeterministicCrypto,
  randomBigInt,
} from "@dassie/lib-reactive"
import { createCrypto } from "@dassie/lib-reactive-io"

import { bigintToUint8Array, encodeField } from "../../der"

const MILLER_RABIN_ITERATIONS = 10

export interface RsaPrivateKeyValues {
  modulus: bigint
  publicExponent: bigint
  privateExponent: bigint
  primeP: bigint
  primeQ: bigint
  exponent1: bigint
  exponent2: bigint
  coefficient: bigint
}

const VERSION = hexToUint8Array("020100")
const PRIVATE_KEY_ALGORITHM = hexToUint8Array("300D06092A864886F70D0101010500")

export function createMockDeterministicCryptoWithRsa() {
  const crypto = createMockDeterministicCrypto(createCrypto())

  function generateRsaKeyPair(this: Crypto, modulusLength: number) {
    const privateKeyValues = generateRsaPrivateKey(crypto, modulusLength)
    const privateKeyDer = serializeRsaPrivateKey(privateKeyValues)

    return this.importRsaKeyPair(privateKeyDer)
  }

  return Object.assign(crypto, {
    generateRsaKeyPair,
  })
}

export function serializeRsaPrivateKey(
  values: RsaPrivateKeyValues,
): Uint8Array {
  return encodeField(
    0x30,
    concatUint8Arrays([
      VERSION,
      PRIVATE_KEY_ALGORITHM,
      encodeField(
        0x04,
        encodeField(
          0x30,
          concatUint8Arrays([
            encodeField(0x02, hexToUint8Array("00")),
            encodeField(0x02, bigintToUint8Array(values.modulus)),
            encodeField(0x02, bigintToUint8Array(values.publicExponent)),
            encodeField(0x02, bigintToUint8Array(values.privateExponent)),
            encodeField(0x02, bigintToUint8Array(values.primeP)),
            encodeField(0x02, bigintToUint8Array(values.primeQ)),
            encodeField(0x02, bigintToUint8Array(values.exponent1)),
            encodeField(0x02, bigintToUint8Array(values.exponent2)),
            encodeField(0x02, bigintToUint8Array(values.coefficient)),
          ]),
        ),
      ),
    ]),
  )
}

export function generateRsaPrivateKey(
  crypto: Crypto,
  modulusLength: number,
): RsaPrivateKeyValues {
  for (;;) {
    const publicExponent = 65_537n // Common public exponent (65537)

    // Generate two large random prime-like numbers p and q
    const primeP = generateCandidatePrime(
      crypto,
      modulusLength,
      MILLER_RABIN_ITERATIONS,
    )
    const primeQ = generateCandidatePrime(
      crypto,
      modulusLength,
      MILLER_RABIN_ITERATIONS,
    )

    // Compute n = p * q (modulus)
    const modulus = primeP * primeQ

    // Compute φ(n) = (p - 1) * (q - 1) (Euler's totient function)
    const eulerTotient = (primeP - 1n) * (primeQ - 1n)

    // Ensure that gcd(publicExponent, φ(n)) == 1, which means publicExponent and φ(n) are coprime
    if (gcd(publicExponent, eulerTotient) !== 1n) {
      continue // If they are not coprime, retry with new primes
    }

    // Compute the private exponent d, which is the modular inverse of publicExponent mod φ(n)
    const privateExponent = modularInverse(publicExponent, eulerTotient)

    // Compute exponent1 = d mod (p-1)
    const exponent1 = privateExponent % (primeP - 1n)

    // Compute exponent2 = d mod (q-1)
    const exponent2 = privateExponent % (primeQ - 1n)

    // Compute coefficient = (inverse of q) mod p
    const coefficient = modularInverse(primeQ, primeP)

    // Return the key components in RSA PKCS#8 format
    return {
      modulus, // modulus (n)
      publicExponent, // public exponent (e)
      privateExponent, // private exponent (d)
      primeP, // prime factor 1 (p)
      primeQ, // prime factor 2 (q)
      exponent1, // d mod (p-1)
      exponent2, // d mod (q-1)
      coefficient, // (inverse of q) mod p
    }
  }
}

/**
 * Computes the greatest common divisor (GCD) of two bigint numbers using the Euclidean algorithm.
 *
 * @param a - The first bigint.
 * @param b - The second bigint.
 * @returns The GCD of a and b.
 */
function gcd(a: bigint, b: bigint): bigint {
  while (b !== BigInt(0)) {
    const previousB = b
    b = a % b
    a = previousB
  }
  return a
}

/**
 * Computes the modular inverse of x modulo m using the Extended Euclidean Algorithm.
 * Returns y such that (x * y) % m = 1.
 *
 * @param x - The number whose modular inverse is to be calculated.
 * @param modulus - The modulus.
 * @returns The modular inverse of x modulo m, or throws an error if no inverse exists.
 */
function modularInverse(x: bigint, modulus: bigint): bigint {
  const originalModulus = modulus
  let previousY = 0n
  let currentY = 1n

  if (modulus === 1n) {
    return 0n // No inverse exists
  }

  while (x > 1n) {
    // Apply the extended Euclidean algorithm
    const quotient = x / modulus
    let temporaryModulus = modulus

    // modulus is remainder now, process as in Euclidean algorithm
    modulus = x % modulus
    x = temporaryModulus
    temporaryModulus = previousY

    // Update previousY and currentY
    previousY = currentY - quotient * previousY
    currentY = temporaryModulus
  }

  // Make currentY positive if it is negative
  if (currentY < 0n) {
    currentY += originalModulus
  }

  return currentY
}

/**
 * Performs modular exponentiation: (base^exponent) % modulus
 *
 * @param base - The base number.
 * @param exponent - The exponent to raise the base.
 * @param modulus - The modulus for the operation.
 * @returns The result of (base^exponent) % modulus.
 */
function modularExponentiation(
  base: bigint,
  exponent: bigint,
  modulus: bigint,
): bigint {
  let result = 1n
  base = base % modulus

  while (exponent > 0n) {
    // If exponent is odd, multiply the base with the result
    if (exponent % 2n === 1n) {
      result = (result * base) % modulus
    }

    // Divide the exponent by 2
    exponent = exponent >> 1n

    // Square the base
    base = (base * base) % modulus
  }

  return result
}

/**
 * Miller-Rabin primality test.
 *
 * @param candidate - The number to test for primality.
 * @param iterations - The number of iterations to run (higher iterations means more reliable results).
 * @returns True if candidate is probably prime, false if it is composite.
 */
function millerRabinPrimalityTest(
  crypto: Crypto,
  candidate: bigint,
  iterations: number,
): boolean {
  // Step 1: Handle small numbers and edge cases
  if (candidate === 2n || candidate === 3n) {
    return true // 2 and 3 are prime
  }
  if (candidate < 2n || candidate % 2n === 0n) {
    return false // No even number greater than 2 can be prime
  }

  // Step 2: Write candidate - 1 as 2^r * oddFactor (where oddFactor is odd)
  let oddFactor = candidate - 1n
  let powerOfTwo = 0n

  while (oddFactor % 2n === 0n) {
    oddFactor /= 2n
    powerOfTwo++
  }

  // Step 3: Witness loop, run the test `iterations` times
  for (let index = 0; index < iterations; index++) {
    // Choose a random base in the range [2, candidate - 2]
    const randomBase = randomBigInt(crypto, 2n, candidate - 2n)

    // Compute x = randomBase^oddFactor % candidate
    let witness = modularExponentiation(randomBase, oddFactor, candidate)

    if (witness === 1n || witness === candidate - 1n) {
      continue // Proceed to the next iteration as the candidate is probably prime for this witness
    }

    let isComposite = true
    // Repeat powerOfTwo - 1 times: witness = witness^2 % candidate
    for (let index2 = 0n; index2 < powerOfTwo - 1n; index2++) {
      witness = modularExponentiation(witness, 2n, candidate)

      if (witness === candidate - 1n) {
        isComposite = false
        break
      }
    }

    if (isComposite) {
      return false // Definitely composite
    }
  }

  return true // Probably prime after all iterations
}

/**
 * Generates a candidate prime number of a specified bit length using the Miller-Rabin primality test.
 *
 * This function continuously generates random big integers within a given bit length range
 * and uses the Miller-Rabin test to check if the candidate is a probable prime. If a probable
 * prime is found, the function returns it.
 *
 * @param crypto - The cryptographic module to generate random numbers securely.
 * @param modulusLength - The desired bit length of the prime number.
 * @param iterations - The number of iterations for the Miller-Rabin test (each iteration reduces the probability of a composite number).
 * @returns A candidate prime number that passed the Miller-Rabin primality test.
 */
export function generateCandidatePrime(
  crypto: Crypto,
  modulusLength: number,
  iterations: number,
): bigint {
  for (;;) {
    // Generate a random number of the specified bit length
    const minPrimeValue = 2n ** BigInt(modulusLength - 1)
    const maxPrimeValue = 2n ** BigInt(modulusLength) - 1n

    const candidatePrime = randomBigInt(crypto, minPrimeValue, maxPrimeValue)

    // Run the Miller-Rabin test to check if the candidate is probably prime
    if (millerRabinPrimalityTest(crypto, candidatePrime, iterations)) {
      return candidatePrime // Return the candidate if it passes the primality test
    }

    // If not prime, continue the loop and generate a new candidate
  }
}
