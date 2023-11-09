import type { Response } from "express"

export interface HttpResponse {
  applyTo: (response: Response) => void
}
