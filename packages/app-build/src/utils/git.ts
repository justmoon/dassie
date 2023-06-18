import { $ } from "zx"

$.verbose = false

export const getHeadCommitShort = async () => {
  const gitResult = await $`git rev-parse --short HEAD`
  return gitResult.stdout.trim()
}
