import type { EffectContext } from "@dassie/lib-reactive"

import { registerBtpHttpUpgrade } from "./register-btp-http-upgrade"

export const startBtpServer = (sig: EffectContext) => {
  sig.run(registerBtpHttpUpgrade)
}
