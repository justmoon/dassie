import { $ } from "execa"

export const getHeadCommitShort = async () => {
  const gitResult = await $`git -c safe.directory=* rev-parse --short HEAD`
  return gitResult.stdout.trim()
}
