import { $ } from "execa"

import assert from "node:assert"
import { writeFileSync } from "node:fs"
import { mkdir, unlink } from "node:fs/promises"
import { dirname } from "node:path"

import { derPreamble } from "@dassie/app-node/src/backend/utils/pem"
import { createLogger } from "@dassie/lib-logger"

import { TEST_NODE_VANITY_KEYS } from "../constants/node-keys"
import { checkFileStatus } from "./check-file-status"

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

const PRIVATE_KEY_HEADER = "-----BEGIN PRIVATE KEY-----\n"
const PRIVATE_KEY_FOOTER = "\n-----END PRIVATE KEY-----\n"
const OPENSSL_ASCII_CHUNK_SIZE = 64

function splitStringIntoChunks(input: string, chunkSize: number): string[] {
  const chunks: string[] = []
  for (let index = 0; index < input.length; index += chunkSize) {
    chunks.push(input.slice(index, index + chunkSize))
  }
  return chunks
}

const writeVanityKey = ({ keyPath }: CertificateInfo, key: string) => {
  writeFileSync(
    keyPath,
    PRIVATE_KEY_HEADER +
      splitStringIntoChunks(
        Buffer.concat([derPreamble, Buffer.from(key, "hex")]).toString(
          "base64"
        ),
        OPENSSL_ASCII_CHUNK_SIZE
      ).join("\n") +
      PRIVATE_KEY_FOOTER
  )
}

const generateCertificate = async ({
  type,
  commonName,
  certificatePath,
  keyPath,
}: CertificateInfo) => {
  if (type === "web") {
    await $`openssl req -new -key ${keyPath} -out ${certificatePath}.csr -days 365 -subj /CN=${commonName}`
    await $`mkcert -csr ${certificatePath}.csr -cert-file ${certificatePath}`
  } else {
    await $`openssl req -new -x509 -key ${keyPath} -out ${certificatePath} -days 365 -subj /CN=${commonName}`
  }
}

const parseNodeIndex = (id: string) => {
  const match = id.match(/n(\d+)/)
  if (!match) return -1
  return Number.parseInt(match[1]!, 10) - 1
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
    assert(certificate.certificatePath)
    assert(certificate.keyPath)
    const certificateStatus = await checkFileStatus(certificate.certificatePath)
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
      await mkdir(dirname(certificate.certificatePath), { recursive: true })
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
      if (certificate.type === "dassie") {
        const index = parseNodeIndex(id)
        const key = TEST_NODE_VANITY_KEYS[index]
        if (key) {
          logger.info("using vanity node key", {
            id,
            name: certificate.commonName,
            type: certificate.type,
          })
          writeVanityKey(certificate, key)
        }
      } else {
        logger.info("generating key", {
          id,
          name: certificate.commonName,
          type: certificate.type,
        })

        await generateKey(certificate)
      }
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
