export interface DraftResponse {
  status: number | undefined
  statusText: string | undefined
  headers: Headers
}

export interface BaseRequestContext {
  url: URL
  request: Request
  response: DraftResponse
}
