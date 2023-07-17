import { clearOutputPath } from "../steps/clear-output-path"
import { copyInstallScript } from "../steps/copy-install-script"
import { createOutputPath } from "../steps/create-output-path"

export const buildInstaller = async () => {
  console.info(`Creating Dassie installer`)

  await createOutputPath()
  await clearOutputPath()

  await copyInstallScript()
}
