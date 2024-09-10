import type { Clock, Crypto } from "@dassie/lib-reactive"

import {
  type X509EntityProperties,
  serializeCertificate,
  serializeToBeSigned,
} from "./certificate"
import { parsePem, serializePem } from "./pem"

interface SelfSignedCertificateOptions {
  clock: Clock
  crypto: Crypto
  subject: X509EntityProperties
  modulusLength?: number | undefined
}

const SERIAL_NUMBER_LENGTH = 20
const KEY_IDENTIFIER_LENGTH = 20

export async function generateSelfSignedCertificate({
  clock,
  crypto,
  subject,
  modulusLength = 2048,
}: SelfSignedCertificateOptions): Promise<{
  certificate: string
  privateKey: string
}> {
  const keyPair = await crypto.generateRsaKeyPair(modulusLength)

  const publicKey = await keyPair.getPublicKeyPem()
  const privateKey = await keyPair.getPrivateKeyPem()

  const { data: subjectPublicKeyInfo } = parsePem(publicKey)
  const serialNumber = crypto.getRandomBytes(SERIAL_NUMBER_LENGTH)

  const subjectPublicKeyInfoHash = await crypto.hash(
    "sha256",
    subjectPublicKeyInfo,
  )
  const keyIdentifier = subjectPublicKeyInfoHash.slice(0, KEY_IDENTIFIER_LENGTH)

  const now = clock.now()
  const toBeSigned = serializeToBeSigned({
    serialNumber,
    issuer: subject,
    validFrom: new Date(now),
    validUntil: new Date(now + 10 * 365 * 24 * 60 * 60 * 1000),
    subject,
    subjectPublicKeyInfo,
    subjectKeyIdentifier: keyIdentifier,
    authorityKeyIdentifier: keyIdentifier,
  })

  const signature = await keyPair.sign(toBeSigned)

  const certificateData = serializeCertificate({
    toBeSigned,
    signature,
  })

  const certificate = serializePem(certificateData, "CERTIFICATE")

  return { certificate, privateKey }
}
