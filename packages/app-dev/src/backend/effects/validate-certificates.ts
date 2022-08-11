import { $ } from "zx"

import assert from "node:assert"
import { constants } from "node:fs"
import { access, unlink } from "node:fs/promises"
import { dirname } from "node:path"
import { isNativeError } from "node:util/types"

import type { InputConfig } from "@dassie/app-node"
import { createLogger } from "@dassie/lib-logger"
import type { EffectContext } from "@dassie/lib-reactive"

import { generateNodeConfig } from "../utils/generate-node-config"
import type { NodeDefinition } from "./run-nodes"

const logger = createLogger("das:dev:validate-certificates")

interface CertificateInfo {
  type: "web" | "dassie"
  certificatePath: string | undefined
  keyPath: string | undefined
  node: NodeDefinition<InputConfig>
}

const checkFileStatus = async (filePath: string) => {
  try {
    await access(filePath, constants.R_OK)
    return "ok"
  } catch (error) {
    const code = isNativeError(error)
      ? (error as NodeJS.ErrnoException).code
      : undefined
    switch (code) {
      case "ENOENT":
        return "missing"
      case "EACCES":
        return "unreadable"
      default:
        return "error"
    }
  }
}

const generateKey = async ({ type, keyPath }: CertificateInfo) => {
  await $`openssl genpkey -algorithm ${
    type === "web" ? "RSA" : "Ed25519"
  } -out ${keyPath}`
}

const generateCertificate = async ({
  type,
  certificatePath,
  keyPath,
  node,
}: CertificateInfo) => {
  if (type === "web") {
    await $`openssl req -new -key ${keyPath} -out ${certificatePath}.csr -days 365 -subj "/CN=${node.id}.localhost"`
    await $`mkcert -csr ${certificatePath}.csr -cert-file ${certificatePath}`
  } else {
    await $`openssl req -new -x509 -key ${keyPath} -out ${certificatePath} -days 365 -subj "/CN=g.das.${node.id}"`
  }
}

interface ValidateCertificatesProperties {
  nodeIndex: number
  nodePeers: readonly number[]
}

export const validateCertificates = async (
  _sig: EffectContext,
  properties: ValidateCertificatesProperties
) => {
  const node = generateNodeConfig(properties.nodeIndex, properties.nodePeers)

  const neededCertificates: CertificateInfo[] = [
    {
      type: "web",
      certificatePath: node.config.tlsWebCertFile,
      keyPath: node.config.tlsWebKeyFile,
      node,
    },
    {
      type: "dassie",
      certificatePath: node.config.tlsDassieCertFile,
      keyPath: node.config.tlsDassieKeyFile,
      node,
    },
  ]

  for (const certificate of neededCertificates) {
    assert(certificate.certificatePath)
    assert(certificate.keyPath)
    const certificateStatus = await checkFileStatus(certificate.certificatePath)
    const keyStatus = await checkFileStatus(certificate.keyPath)

    if (keyStatus === "unreadable") {
      logger.error("key is unreadable", {
        node: node.id,
        type: certificate.type,
      })
      return
    }

    if (certificateStatus === "unreadable") {
      logger.error("certificate is unreadable", {
        node: node.id,
        type: certificate.type,
      })
      return
    }

    if (keyStatus === "missing" || certificateStatus === "missing") {
      logger.info("certificate path not found, creating directory", {
        node: node.id,
      })
      await $`mkdir -p ${dirname(certificate.certificatePath)}`
    }

    if (keyStatus === "missing" && certificateStatus === "ok") {
      // If the key is missing, but the certificate is ok, we need to delete the certificate
      logger.warn("key missing, deleting certificate", { node: node.id })

      await unlink(certificate.certificatePath)
    }

    if (keyStatus === "missing") {
      logger.info("generating key", { node: node.id, type: certificate.type })
      await generateKey(certificate)
    }

    if (certificateStatus === "missing") {
      logger.info("generating certificate", {
        node: node.id,
        type: certificate.type,
      })
      await generateCertificate(certificate)
    }
  }
}
