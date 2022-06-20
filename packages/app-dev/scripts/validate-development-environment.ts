#!/usr/bin/env node
import colors from "picocolors"
import { $ } from "zx"

import assert from "node:assert"
import { constants } from "node:fs"
import { access, unlink } from "node:fs/promises"
import { dirname } from "node:path"
import { isNativeError } from "node:util/types"

import type { InputConfig } from "@xen-ilp/app-node/src/config"
import { createLogger } from "@xen-ilp/lib-logger"

import type { NodeDefinition } from "../src"
import { NODES } from "../src/constants/development-nodes"

const logger = createLogger("xen:dev:validate")

interface CertificateInfo {
  type: "web" | "xen"
  certificatePath: string | undefined
  keyPath: string | undefined
  node: NodeDefinition<InputConfig>
}

const neededCertificates: Array<CertificateInfo> = NODES.flatMap((node) => [
  {
    type: "web",
    certificatePath: node.config.tlsWebCertFile,
    keyPath: node.config.tlsWebKeyFile,
    node,
  },
  {
    type: "xen",
    certificatePath: node.config.tlsXenCertFile,
    keyPath: node.config.tlsXenKeyFile,
    node,
  },
])

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

const applyColorToFileStatus = (status: string) => {
  return status === "ok"
    ? colors.green(status)
    : status === "generated"
    ? colors.yellow(status)
    : colors.red(status)
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
    await $`openssl req -new -x509 -key ${keyPath} -out ${certificatePath} -days 365 -subj "/CN=g.xen.${node.id}"`
  }
}

const checkCertificates = async () => {
  console.log(colors.bold("\nValidating certificates:"))
  let anyFilesGenerated = false
  let anyFilesMissing = false
  let anyFilesUnreadable = false
  let anyFilesError = false
  let output = ""
  for (const certificate of neededCertificates) {
    try {
      assert(certificate.certificatePath)
      assert(certificate.keyPath)
      let certificateStatus = await checkFileStatus(certificate.certificatePath)
      let keyStatus = await checkFileStatus(certificate.keyPath)

      if (keyStatus === "missing" || certificateStatus === "missing") {
        await $`mkdir -p ${dirname(certificate.certificatePath)}`
      }

      if (keyStatus === "missing" && certificateStatus === "ok") {
        // If the key is missing, but the certificate is ok, we need to delete the certificate
        await unlink(certificate.certificatePath)
        certificateStatus = "missing"
      }

      if (keyStatus === "missing") {
        await generateKey(certificate)
        keyStatus = "generated"
      }

      if (certificateStatus === "missing") {
        await generateCertificate(certificate)
        certificateStatus = "generated"
      }

      anyFilesGenerated ||=
        certificateStatus === "generated" || keyStatus === "generated"
      anyFilesMissing ||=
        certificateStatus === "missing" || keyStatus === "missing"
      anyFilesUnreadable ||=
        certificateStatus === "unreadable" || keyStatus === "unreadable"
      anyFilesError ||= certificateStatus === "error" || keyStatus === "error"

      const status =
        certificateStatus === keyStatus
          ? applyColorToFileStatus(certificateStatus)
          : `certificate: ${applyColorToFileStatus(
              certificateStatus
            )} key: ${applyColorToFileStatus(keyStatus)}`
      output += `${colors.gray(
        certificate === neededCertificates.at(-1) ? "└─" : "├─"
      )} ${certificate.node.id}/${certificate.type}: ${status}\n`
    } catch {
      anyFilesError = true
    }
  }

  if (
    !anyFilesGenerated &&
    !anyFilesMissing &&
    !anyFilesUnreadable &&
    !anyFilesError
  ) {
    console.log(colors.green("└─ all ok"))
    console.log()
  } else if (anyFilesError) {
    console.log(output)
    console.error(
      colors.red(
        `${colors.bold(
          "FATAL:"
        )} An error occurred while validating/generating the certificates`
      )
    )
    process.exit(1)
  } else if (anyFilesUnreadable) {
    console.log(output)
    console.error(
      colors.red(
        `${colors.bold(
          "FATAL:"
        )} An permission error occurred while reading one or more certificate files`
      )
    )
    console.info(
      "Please make sure all certificates are readable by the current user."
    )
    process.exit(1)
  } else {
    console.log(output)
  }
}

checkCertificates().catch((error) => logger.logError(error))
