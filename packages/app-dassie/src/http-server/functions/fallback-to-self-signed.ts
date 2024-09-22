import { X509Certificate } from "node:crypto"

import { generateSelfSignedCertificate } from "@dassie/lib-x509"

import type { DassieReactor } from "../../base/types/dassie-base"
import { http as logger } from "../../logger/instances"

interface FallbackToSelfSignedParameters {
  tlsWebCert: string | undefined
  tlsWebKey: string | undefined
}

export function FallbackToSelfSigned(reactor: DassieReactor) {
  /**
   * Generate a self-signed certificate if the passed in certificate or key are undefined.
   */
  return async function fallbackToSelfSigned({
    tlsWebCert,
    tlsWebKey,
  }: FallbackToSelfSignedParameters) {
    if (tlsWebCert && tlsWebKey) {
      return {
        tlsWebCert,
        tlsWebKey,
      }
    }

    logger.info(
      "no tls certificate configured,generating self-signed certificate",
    )

    const { certificate, privateKey } = await generateSelfSignedCertificate({
      clock: reactor.base.clock,
      crypto: reactor.base.crypto,
      subject: {
        commonName: "localhost",
        organizationName: "Unknown Dassie Operator",
        organizationalUnitName: "Node",
        countryName: "XX",
        stateOrProvinceName: "XX",
        localityName: "Unknown",
      },
      modulusLength: 2048,
    })

    const x509 = new X509Certificate(certificate)

    logger.info("tls certificate successfully generated", {
      sha256Fingerprint: x509.fingerprint256,
    })

    return {
      tlsWebCert: certificate,
      tlsWebKey: privateKey,
    }
  }
}
