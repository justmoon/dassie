// eslint-disable-next-line @typescript-eslint/triple-slash-reference
/// <reference path="../environment.d.ts" />

export { nodeTableStore } from "./peer-protocol/stores/node-table"
export type { Config, InputConfig } from "./config/environment-config"
export { environmentConfigSignal } from "./config/environment-config"
export { start, rootActor } from "./start"
