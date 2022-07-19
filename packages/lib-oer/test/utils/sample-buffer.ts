import { createHmac } from "node:crypto"

export const sampleBuffer = new Uint8Array(1024)

for (let index = 0; index < sampleBuffer.byteLength / 512; index++) {
  const offset = index * 64

  const seededPseudoRandom = createHmac("sha512", "xen-test-seed")
  seededPseudoRandom.update(String(index))
  sampleBuffer.set(seededPseudoRandom.digest(), offset)
}
