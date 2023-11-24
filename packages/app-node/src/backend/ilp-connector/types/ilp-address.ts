import { Opaque } from "type-fest"

import { IlpAllocationScheme } from "../../config/computed/ilp-allocation-scheme"
import { NodeId } from "../../peer-protocol/types/node-id"

export type IlpAddress = DassieIlpAddress | IldcpIlpAddress | IlpHttpIlpAddress

export type DassieIlpAddress = `${IlpAllocationScheme}.das.${NodeId}${string}`
export type IldcpIlpAddress = "peer.config"
export type IlpHttpIlpAddress = Opaque<string, "IlpHttpIlpAddress">
