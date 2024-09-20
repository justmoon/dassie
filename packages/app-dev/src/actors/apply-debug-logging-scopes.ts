import { createEnableChecker, getLogContext } from "@dassie/lib-logger"
import { createActor } from "@dassie/lib-reactive"

import { DebugScopesSignal } from "../signals/debug-scopes"

export const ApplyDebugLoggingScopes = () =>
  createActor((sig) => {
    const scopes = sig.readAndTrack(DebugScopesSignal)
    const context = getLogContext()
    context.enableChecker = createEnableChecker(scopes)
  })
