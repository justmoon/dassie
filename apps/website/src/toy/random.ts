/**
 * The STREAM toy's randomness: a seeded generator, so that a run with the
 * same settings and seed takes the same course.
 *
 * @module
 */

/** Seeds accepted: 32-bit unsigned integers. */
export const MAX_SEED = 0xffff_ffff;

/** Uniform numbers in [0, 1) from a 32-bit seed (mulberry32). */
export class Random {
  #state: number;

  constructor(seed: number) {
    if (!Number.isInteger(seed) || seed < 0 || seed > MAX_SEED) {
      throw new RangeError(`seed ${seed} is not a 32-bit unsigned integer`);
    }
    this.#state = seed;
  }

  /** The next number in [0, 1). */
  next(): number {
    this.#state = (this.#state + 0x6d2b79f5) | 0;
    let t = this.#state;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 0x1_0000_0000;
  }

  /** A standard normal sample (Box-Muller). */
  normal(): number {
    // 1 - next() lies in (0, 1], so the logarithm is finite.
    const u = 1 - this.next();
    const v = this.next();
    return Math.sqrt(-2 * Math.log(u)) * Math.cos(2 * Math.PI * v);
  }

  /**
   * A log-normal sample with median `median` and spread `sigma`, as the
   * original's test network drew its latency.
   */
  logNormal(median: number, sigma: number): number {
    if (median <= 0) return 0;
    return median * Math.exp(sigma * this.normal());
  }
}
