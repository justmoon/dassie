import { createActor, createReactor } from "@dassie/lib-reactive"

import { SUPPORTED_ARCHITECTURES } from "./constants/architectures"
import { SUPPORTED_COMPRESSIONS } from "./constants/compression"
import { DASSIE_VERSION } from "./constants/version"
import { buildBackend } from "./steps/build-backend"
import { buildFrontend } from "./steps/build-frontend"
import { compressBundle } from "./steps/compress-bundle"
import { copyFilesIntoBundle } from "./steps/copy-files-into-bundle"
import { createOutputPath } from "./steps/create-output-path"
import { deleteOutputPath } from "./steps/delete-output-path"
import { downloadBetterSqlite3 } from "./steps/download-better-sqlite3"
import { downloadNodeJs } from "./steps/download-node-js"
import { tarBundle } from "./steps/tar-bundle"
import { getTarFilename } from "./utils/bundle-name"

export const rootActor = () =>
  createActor(async () => {
    console.info(`Creating Dassie bundles for version ${DASSIE_VERSION}`)

    await deleteOutputPath()
    await createOutputPath()

    console.info("Building backend")
    await buildBackend()

    console.info("Building frontend")
    await buildFrontend()

    for (const architecture of SUPPORTED_ARCHITECTURES) {
      console.info()
      console.info(`Creating ${getTarFilename(architecture)}`)
      await downloadNodeJs(architecture)
      await downloadBetterSqlite3(architecture)
      await copyFilesIntoBundle(architecture)
      await tarBundle(architecture)

      for (const compression of SUPPORTED_COMPRESSIONS) {
        console.info(`Creating ${getTarFilename(architecture)}.${compression}`)
        await compressBundle(architecture, compression)
      }
    }

    console.info()
    console.info("Generate metadata")
    await generateMetadata()
  })

export const start = () => createReactor(rootActor)

export default start
