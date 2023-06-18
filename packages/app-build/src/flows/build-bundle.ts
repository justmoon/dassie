import { SUPPORTED_ARCHITECTURES } from "../constants/architectures"
import { SUPPORTED_COMPRESSIONS } from "../constants/compression"
import { DassieDetailedVersion, DassieVersion } from "../constants/version"
import { buildBackend } from "../steps/build-backend"
import { buildFrontend } from "../steps/build-frontend"
import { compressBundle } from "../steps/compress-bundle"
import { copyFilesIntoBundle } from "../steps/copy-files-into-bundle"
import { createOutputPath } from "../steps/create-output-path"
import { deleteOutputPath } from "../steps/delete-output-path"
import { downloadBetterSqlite3 } from "../steps/download-better-sqlite3"
import { downloadNodeJs } from "../steps/download-node-js"
import { generateMetadata } from "../steps/generate-metadata"
import { tarBundle } from "../steps/tar-bundle"
import { getCompressedFilename, getTarFilename } from "../utils/bundle-name"

export interface BundleOptions {
  version: DassieVersion
  detailedVersion: DassieDetailedVersion
  isMainRelease: boolean
}

export const buildBundle = async ({
  version,
  detailedVersion,
  isMainRelease,
}: BundleOptions) => {
  console.info(`Creating Dassie bundles for version ${detailedVersion}`)

  await deleteOutputPath()
  await createOutputPath()

  console.info("Building backend")
  await buildBackend(detailedVersion)

  console.info("Building frontend")
  await buildFrontend(detailedVersion)

  for (const architecture of SUPPORTED_ARCHITECTURES) {
    console.info()
    console.info(`Creating ${getTarFilename(version, architecture)}`)
    await downloadNodeJs(architecture)
    await downloadBetterSqlite3(architecture)
    await copyFilesIntoBundle(version, architecture)
    await tarBundle(version, architecture)

    for (const compression of SUPPORTED_COMPRESSIONS) {
      console.info(
        `Creating ${getCompressedFilename(version, architecture, compression)}`
      )
      await compressBundle(version, architecture, compression)
    }
  }

  if (isMainRelease) {
    console.info()
    console.info("Generate metadata")
    await generateMetadata(version)
  }
}
