import { createActor, createReactor } from "@dassie/lib-reactive"

import { addSystemdServiceToBundle } from "./steps/add-systemd-service-to-bundle"
import { buildBackend } from "./steps/build-backend"
import { compressBundle } from "./steps/compress-bundle"
import { copyNativeBindings } from "./steps/copy-native-bindings"
import { createOutputPath } from "./steps/create-output-path"
import { deleteOutputPath } from "./steps/delete-output-path"
import { downloadNodeJs } from "./steps/download-node-js"

export const rootActor = () =>
  createActor(async () => {
    console.log("running build")

    await deleteOutputPath()
    await createOutputPath()
    await copyNativeBindings()
    await buildBackend()
    await downloadNodeJs()
    await addSystemdServiceToBundle()
    await compressBundle()
  })

export const start = () => createReactor(rootActor)

export default start
