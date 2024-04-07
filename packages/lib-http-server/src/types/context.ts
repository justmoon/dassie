export interface DraftResponse {
  status: number | undefined
  statusText: string | undefined
  headers: Headers
}
export type RequestContext<T extends object = object> = {
  url: URL
  request: Request
  response: DraftResponse
} & T
