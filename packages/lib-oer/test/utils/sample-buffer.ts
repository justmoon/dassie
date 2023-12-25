import { createHmac } from "node:crypto"

export const sampleBuffer = initializeSampleBuffer()

function initializeSampleBuffer() {
  const buffer = new Uint8Array(1024)

  for (let index = 0; index < buffer.byteLength / 512; index++) {
    const offset = index * 64

    const seededPseudoRandom = createHmac("sha512", "xen-test-seed")
    seededPseudoRandom.update(String(index))
    buffer.set(seededPseudoRandom.digest(), offset)
  }

  return buffer
}
