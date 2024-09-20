import type { Tagged } from "type-fest"

import type { IlpAllocationScheme } from "../../config/computed/ilp-allocation-scheme"
import type { NodeId } from "../../peer-protocol/types/node-id"

export type IlpAddress = DassieIlpAddress | IldcpIlpAddress | IlpHttpIlpAddress

export type DassieIlpAddress = `${IlpAllocationScheme}.das.${NodeId}${string}`
export type IldcpIlpAddress = "peer.config"
export type IlpHttpIlpAddress = Tagged<string, "IlpHttpIlpAddress">
