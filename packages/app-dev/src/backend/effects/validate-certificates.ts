import { $ } from "zx"

import assert from "node:assert"
import { unlink } from "node:fs/promises"
import { dirname } from "node:path"

import { createLogger } from "@dassie/lib-logger"
import { createActor } from "@dassie/lib-reactive"

import { checkFileStatus } from "../utils/check-file-status"

const logger = createLogger("das:dev:validate-certificates")

export interface CertificateInfo {
  type: "web" | "dassie"
  commonName: string
  certificatePath: string
  keyPath: string
}

const generateKey = async ({ type, keyPath }: CertificateInfo) => {
  await $`openssl genpkey -algorithm ${
    type === "web" ? "RSA" : "Ed25519"
  } -out ${keyPath}`
}

const generateCertificate = async ({
  type,
  commonName,
  certificatePath,
  keyPath,
}: CertificateInfo) => {
  if (type === "web") {
    await $`openssl req -new -key ${keyPath} -out ${certificatePath}.csr -days 365 -subj "/CN=${commonName}"`
    await $`mkcert -csr ${certificatePath}.csr -cert-file ${certificatePath}`
  } else {
    await $`openssl req -new -x509 -key ${keyPath} -out ${certificatePath} -days 365 -subj "/CN=${commonName}"`
  }
}

interface ValidateCertificatesProperties {
  id: string
  certificates: CertificateInfo[]
}

export const validateCertificates = () =>
  createActor(
    async (_sig, { id, certificates }: ValidateCertificatesProperties) => {
      for (const certificate of certificates) {
        assert(certificate.certificatePath)
        assert(certificate.keyPath)
        const certificateStatus = await checkFileStatus(
          certificate.certificatePath
        )
        const keyStatus = await checkFileStatus(certificate.keyPath)

        if (keyStatus === "unreadable") {
          logger.error("key is unreadable", {
            id,
            name: certificate.commonName,
            type: certificate.type,
          })
          return
        }

        if (certificateStatus === "unreadable") {
          logger.error("certificate is unreadable", {
            id,
            name: certificate.commonName,
            type: certificate.type,
          })
          return
        }

        if (keyStatus === "missing" || certificateStatus === "missing") {
          logger.info("certificate path not found, creating directory", {
            id,
            name: certificate.commonName,
          })
          await $`mkdir -p ${dirname(certificate.certificatePath)}`
        }

        if (keyStatus === "missing" && certificateStatus === "ok") {
          // If the key is missing, but the certificate is ok, we need to delete the certificate
          logger.warn("key missing, deleting certificate", {
            id,
            name: certificate.commonName,
          })

          await unlink(certificate.certificatePath)
        }

        if (keyStatus === "missing") {
          logger.info("generating key", {
            id,
            name: certificate.commonName,
            type: certificate.type,
          })

          await generateKey(certificate)
        }

        if (certificateStatus === "missing") {
          logger.info("generating certificate", {
            id,
            name: certificate.commonName,
            type: certificate.type,
          })
          await generateCertificate(certificate)
        }
      }
    }
  )
