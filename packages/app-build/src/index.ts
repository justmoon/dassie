import { createActor, createReactor } from "@dassie/lib-reactive"

import { buildBackend } from "./steps/build-backend"
import { copyNativeBindings } from "./steps/copy-native-bindings"
import { createOutputPath } from "./steps/create-output-path"
import { deleteOutputPath } from "./steps/delete-output-path"

export const rootActor = () =>
  createActor(async (sig) => {
    console.log("running build")
    await sig.run(deleteOutputPath)
    await sig.run(createOutputPath)
    await sig.run(copyNativeBindings)
    await sig.run(buildBackend)
  })

export const start = () => createReactor(rootActor)

export default start
