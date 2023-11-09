interface IlpHttpRequestEntry {
  requestId: string
  callbackUrl: string
}

let uniqueId = 0

export const nextId = () => uniqueId++

export const IncomingRequestIdMap = () => {
  return new Map<number, IlpHttpRequestEntry>()
}
