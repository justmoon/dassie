import type { Logger } from "@dassie/lib-logger"
import type {
  IlpPacket,
  IlpPreparePacket,
  IlpType,
} from "@dassie/lib-protocol-ilp"

import type { CryptoContext } from "../crypto/context"

export interface ConnectionContext {
  readonly crypto: CryptoContext
  readonly logger: Logger
  readonly sendPacket: (
    packet: IlpPreparePacket,
  ) => Promise<
    IlpPacket & { type: typeof IlpType.Fulfill | typeof IlpType.Reject }
  >
  readonly getDateNow: () => number
}
