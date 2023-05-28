import { createActor, createReactor } from "@dassie/lib-reactive"

import { buildBackend } from "./steps/build-backend"
import { createOutputPath } from "./steps/create-output-path"
import { deleteOutputPath } from "./steps/delete-output-path"

export const rootActor = () =>
  createActor(async (sig) => {
    console.log("running build")
    await sig.run(deleteOutputPath).result
    await sig.run(createOutputPath).result
    await sig.run(buildBackend).result
  })

export const start = () => createReactor(rootActor)

export default start
