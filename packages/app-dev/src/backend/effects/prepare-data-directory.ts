import { $ } from "zx"

import type { EffectContext } from "@dassie/lib-reactive"

export const prepareDataDirectory = async (
  _sig: EffectContext,
  dataPath: string
) => {
  await $`mkdir -p ${dataPath}`
}
