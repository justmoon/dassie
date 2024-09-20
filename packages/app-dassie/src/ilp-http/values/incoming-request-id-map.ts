interface IlpHttpRequestEntry {
  requestId: string
  callbackUrl: string
}

export const IncomingRequestIdMap = () => {
  return new Map<number | string, IlpHttpRequestEntry>()
}
