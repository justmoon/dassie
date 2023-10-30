import chalk from "chalk"

import { createFlow, header, note } from "@dassie/lib-terminal-graphics"

import {
  Architecture,
  SUPPORTED_ARCHITECTURES,
} from "../constants/architectures"
import { SUPPORTED_COMPRESSIONS } from "../constants/compression"
import { DassieDetailedVersion, DassieVersion } from "../constants/version"
import { buildBackend } from "../steps/build-backend"
import { buildFrontend } from "../steps/build-frontend"
import { clearOutputPath } from "../steps/clear-output-path"
import { compressBundle } from "../steps/compress-bundle"
import { copyFilesIntoBundle } from "../steps/copy-files-into-bundle"
import { copyInstallScript } from "../steps/copy-install-script"
import { createOutputPath } from "../steps/create-output-path"
import { downloadBetterSqlite3 } from "../steps/download-better-sqlite3"
import { downloadNodeJs } from "../steps/download-node-js"
import { generateMetadata } from "../steps/generate-metadata"
import { tarBundle } from "../steps/tar-bundle"
import { getCompressedFilename, getTarFilename } from "../utils/bundle-name"

export interface BundleOptions {
  version: DassieVersion
  detailedVersion: DassieDetailedVersion
  isMainRelease: boolean
  architectures?: readonly Architecture[]
}

export const buildBundle = async ({
  version,
  detailedVersion,
  isMainRelease,
  architectures = SUPPORTED_ARCHITECTURES,
}: BundleOptions) => {
  const flow = createFlow()

  flow.show(header({ title: "Dassie Build" }))

  flow.show(
    note({ title: `Creating Dassie bundles for version ${detailedVersion}` }),
  )

  await createOutputPath()
  await clearOutputPath()

  flow.show(note({ title: "Building backend" }))
  await buildBackend(detailedVersion)

  flow.show(note({ title: "Building frontend" }))
  await buildFrontend(detailedVersion)

  for (const architecture of architectures) {
    flow.show(note({ title: `Building bundles for ${architecture}` }))
    console.info(chalk.dim(` … ${getTarFilename(version, architecture)}`))
    await downloadNodeJs(architecture)
    await downloadBetterSqlite3(architecture)
    await copyFilesIntoBundle(version, architecture)
    await tarBundle(version, architecture)

    for (const compression of SUPPORTED_COMPRESSIONS) {
      console.info(
        chalk.dim(
          ` … ${getCompressedFilename(version, architecture, compression)}`,
        ),
      )
      await compressBundle(version, architecture, compression)
    }
  }

  flow.show(note({ title: "Copying install script" }))
  await copyInstallScript()

  if (isMainRelease) {
    flow.show(note({ title: "Generating metadata" }))
    await generateMetadata(version)
  }
}
