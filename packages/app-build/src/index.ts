import { createActor, createReactor } from "@dassie/lib-reactive"

import { SUPPORTED_ARCHITECTURES } from "./constants/architectures"
import { DASSIE_VERSION } from "./constants/version"
import { buildBackend } from "./steps/build-backend"
import { compressBundle } from "./steps/compress-bundle"
import { copyFilesIntoBundle } from "./steps/copy-files-into-bundle"
import { createOutputPath } from "./steps/create-output-path"
import { deleteOutputPath } from "./steps/delete-output-path"
import { downloadBetterSqlite3 } from "./steps/download-better-sqlite3"
import { downloadNodeJs } from "./steps/download-node-js"
import { getBundleFilename } from "./utils/bundle-name"

export const rootActor = () =>
  createActor(async () => {
    console.log(`Creating Dassie bundles for version ${DASSIE_VERSION}`)

    await deleteOutputPath()
    await createOutputPath()

    console.log("Building backend")
    await buildBackend()

    for (const architecture of SUPPORTED_ARCHITECTURES) {
      console.log(`Creating ${getBundleFilename(architecture)}`)
      await downloadNodeJs(architecture)
      await downloadBetterSqlite3(architecture)
      await copyFilesIntoBundle(architecture)
      await compressBundle(architecture)
    }
  })

export const start = () => createReactor(rootActor)

export default start
