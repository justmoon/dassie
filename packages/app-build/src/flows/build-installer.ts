import { copyInstallScript } from "../steps/copy-install-script"
import { createOutputPath } from "../steps/create-output-path"
import { deleteOutputPath } from "../steps/delete-output-path"

export const buildInstaller = async () => {
  console.info(`Creating Dassie installer`)

  await deleteOutputPath()
  await createOutputPath()

  await copyInstallScript()
}
