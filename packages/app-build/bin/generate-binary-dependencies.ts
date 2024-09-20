import { $ } from "execa"
import { temporaryFile } from "tempy"

import { createHash } from "node:crypto"
import path from "node:path"
import { Readable } from "node:stream"
import { pipeline } from "node:stream/promises"

import { createFlow, header, tasklist } from "@dassie/lib-terminal-graphics"

import { SUPPORTED_ARCHITECTURES } from "../src/constants/architectures"
import {
  NODEJS_SIGNERS,
  NODEJS_SIGNERS_KEYSERVER,
} from "../src/constants/nodejs-signers"
import {
  BETTER_SQLITE3_VERSION,
  NODE_ABI_VERSION,
  NODE_VERSION,
} from "../src/constants/version"
import {
  SQLITE_ARCHITECTURE_MAP,
  getBetterSqliteDownloadUrl,
} from "../src/steps/download-better-sqlite3"

const NODE_BASE_URL = `https://nodejs.org/dist/v${NODE_VERSION}/`

const flow = createFlow({ outputStream: process.stderr })

flow.show(header({ title: "Generate Binary Dependencies" }))

const shasumsMap = new Map<string, string>()

await flow.attach(tasklist({}), async (state) => {
  state.act.addTask("fetch-nodejs-signer-keys", {
    description: "Fetching Node.js signer keys",
    progress: 0,
  })
  const temporaryKeyringPath = temporaryFile({ extension: ".gpg" })

  let keysFetched = 0
  for (const key of NODEJS_SIGNERS) {
    await $`gpg --no-default-keyring --keyring ${temporaryKeyringPath} --keyserver ${NODEJS_SIGNERS_KEYSERVER} --recv-keys ${key}`

    keysFetched++
    state.act.updateTask("fetch-nodejs-signer-keys", {
      progress: keysFetched / NODEJS_SIGNERS.length,
    })
  }

  state.act.updateTask("fetch-nodejs-signer-keys", {
    progress: "done",
  })

  state.act.addTask("fetch-nodejs-release-shasums", {
    description: "Fetching Node.js release SHASUMS256.txt.asc",
    progress: "indeterminate",
  })

  const result = await fetch(`${NODE_BASE_URL}/SHASUMS256.txt.asc`)
  const signedShasums = await result.text()

  state.act.updateTask("fetch-nodejs-release-shasums", {
    progress: "done",
  })

  state.act.addTask("verify-nodejs-release-signature", {
    description: "Verifying Node.js release signature",
    progress: "indeterminate",
  })

  const { stdout: shasums } = await $({
    input: signedShasums,
  })`gpg --no-default-keyring --keyring ${temporaryKeyringPath}`

  for (const line of shasums.split("\n")) {
    const [hash, file] = line.split("  ")
    if (!hash || !file?.startsWith("node-v")) continue

    shasumsMap.set(file, hash)
  }

  state.act.updateTask("verify-nodejs-release-signature", {
    progress: "done",
  })

  state.act.addTask("download-better-sqlite3", {
    description: "Downloading better-sqlite3 binaries to calculate checksums",
    progress: 0,
  })

  for (const architecture of SUPPORTED_ARCHITECTURES) {
    const url = getBetterSqliteDownloadUrl(
      BETTER_SQLITE3_VERSION,
      NODE_ABI_VERSION,
      SQLITE_ARCHITECTURE_MAP[architecture],
    )
    const filename = path.basename(new URL(url).pathname)

    const result = await fetch(url)
    if (!result.body) {
      throw new Error("Sqlite download failed - no response body")
    }

    const digest = createHash("sha256")
    // eslint-disable-next-line n/no-unsupported-features/node-builtins
    await pipeline(Readable.fromWeb(result.body), digest)

    const hash = digest.digest("hex")

    shasumsMap.set(filename, hash)
  }

  state.act.updateTask("download-better-sqlite3", {
    progress: "done",
  })
})

for (const architecture of SUPPORTED_ARCHITECTURES) {
  const file = `node-v${NODE_VERSION}-linux-${architecture}.tar.xz`
  if (!shasumsMap.has(file)) {
    throw new Error(`No hash for ${file}`)
  }

  console.info(`${NODE_BASE_URL}${file} ${shasumsMap.get(file)}`)
}

for (const architecture of SUPPORTED_ARCHITECTURES) {
  const url = getBetterSqliteDownloadUrl(
    BETTER_SQLITE3_VERSION,
    NODE_ABI_VERSION,
    SQLITE_ARCHITECTURE_MAP[architecture],
  )
  const file = path.basename(new URL(url).pathname)
  if (!shasumsMap.has(file)) {
    throw new Error(`No hash for ${file}`)
  }

  console.info(`${url} ${shasumsMap.get(file)}`)
}
