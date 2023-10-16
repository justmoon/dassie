import { ActorContext } from "@dassie/lib-reactive"

export interface TrpcContext {
  sig: ActorContext
  user: boolean
}
