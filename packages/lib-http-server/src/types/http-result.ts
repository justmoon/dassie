import type { RequestContext } from "./context"

export interface HttpResult {
  asResponse: (context: RequestContext) => Response
}
