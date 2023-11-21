import { createEnableChecker, getLoggingContext } from "@dassie/lib-logger"
import { createActor } from "@dassie/lib-reactive"

import { DebugScopesSignal } from "../signals/debug-scopes"

export const ApplyDebugLoggingScopes = () =>
  createActor((sig) => {
    const scopes = sig.readAndTrack(DebugScopesSignal)
    const context = getLoggingContext()
    context.enableChecker = createEnableChecker(scopes)
  })
