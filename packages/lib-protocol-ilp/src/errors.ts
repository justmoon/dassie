export const IlpErrorCode = {
  F00_BAD_REQUEST: "F00",
  F01_INVALID_PACKET: "F01",
  F02_UNREACHABLE: "F02",
  F03_INVALID_AMOUNT: "F03",
  F04_INSUFFICIENT_DESTINATION_AMOUNT: "F04",
  F05_WRONG_CONDITION: "F05",
  F06_UNEXPECTED_PAYMENT: "F06",
  F07_CANNOT_RECEIVE: "F07",
  F08_AMOUNT_TOO_LARGE: "F08",
  F99_APPLICATION_ERROR: "F99",
  T00_INTERNAL_ERROR: "T00",
  T01_PEER_UNREACHABLE: "T01",
  T02_PEER_BUSY: "T02",
  T03_CONNECTOR_BUSY: "T03",
  T04_INSUFFICIENT_LIQUIDITY: "T04",
  T05_RATE_LIMITED: "T05",
  T99_APPLICATION_ERROR: "T99",
  R00_TRANSFER_TIMED_OUT: "R00",
  R01_INSUFFICIENT_SOURCE_AMOUNT: "R01",
  R02_INSUFFICIENT_TIMEOUT: "R02",
  R99_APPLICATION_ERROR: "R99",
} as const

export const humanReadableIlpErrors: Record<IlpErrorCode, string> = {
  F00: "Bad Request",
  F01: "Invalid Packet",
  F02: "Unavailable",
  F03: "Invalid Amount",
  F04: "Insufficient Destination Amount",
  F05: "Wrong Condition",
  F06: "Unexpected Payment",
  F07: "Cannot Receive",
  F08: "Amount Too Large",
  F99: "Application Error",
  T00: "Internal Error",
  T01: "Peer Unreachable",
  T02: "Peer Busy",
  T03: "Connector Busy",
  T04: "Insufficient Liquidity",
  T05: "Rate Limited",
  T99: "Application Error",
  R00: "Transfer Timed Out",
  R01: "Insufficient Source Amount",
  R02: "Insufficient Timeout",
  R99: "Application Error",
}

export const getHumanReadableIlpError = (code: string) => {
  return (
    (humanReadableIlpErrors as Record<string, string>)[code] ?? "Unknown Error"
  )
}

export type IlpErrorCode = (typeof IlpErrorCode)[keyof typeof IlpErrorCode]
