import { $ } from "execa"

import { mkdir, unlink } from "node:fs/promises"
import path from "node:path"

import { assert } from "@dassie/lib-logger"

import { setup as logger } from "../logger/instances"
import { checkFileStatus } from "./check-file-status"

export interface CertificateInfo {
  commonName: string
  certificatePath: string
  keyPath: string
}

const generateKey = async ({ keyPath }: CertificateInfo) => {
  await $`openssl genpkey -algorithm RSA -out ${keyPath}`
}

const generateCertificate = async ({
  commonName,
  certificatePath,
  keyPath,
}: CertificateInfo) => {
  await $`openssl req -new -key ${keyPath} -out ${certificatePath}.csr -days 365 -subj /CN=${commonName}`
  await $`mkcert -csr ${certificatePath}.csr -cert-file ${certificatePath}`
}

interface ValidateCertificatesProperties {
  id: string
  certificates: CertificateInfo[]
}

export const validateCertificates = async ({
  id,
  certificates,
}: ValidateCertificatesProperties) => {
  for (const certificate of certificates) {
    assert(logger, !!certificate.certificatePath, "certificate path is missing")
    assert(logger, !!certificate.keyPath, "key path is missing")
    const certificateStatus = await checkFileStatus(certificate.certificatePath)
    const keyStatus = await checkFileStatus(certificate.keyPath)

    if (keyStatus === "unreadable") {
      logger.error("key is unreadable", {
        id,
        name: certificate.commonName,
      })
      return
    }

    if (certificateStatus === "unreadable") {
      logger.error("certificate is unreadable", {
        id,
        name: certificate.commonName,
      })
      return
    }

    if (keyStatus === "missing" || certificateStatus === "missing") {
      logger.info("certificate path not found, creating directory", {
        id,
        name: certificate.commonName,
      })
      await mkdir(path.dirname(certificate.certificatePath), {
        recursive: true,
      })
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
      })

      await generateKey(certificate)
    }

    if (certificateStatus === "missing") {
      logger.info("generating certificate", {
        id,
        name: certificate.commonName,
      })
      await generateCertificate(certificate)
    }
  }
}
