import { z } from "zod"

export const schema = z.discriminatedUnion("method", [
  z.object({
    method: z.literal("hello"),
    params: z.object({
      nodeId: z.string(),
      sequence: z.number(),
      neighbors: z.array(
        z.object({
          nodeId: z.string(),
          proof: z.string(),
        })
      ),
    }),
  }),
  z.object({
    method: z.literal("lsu"), // link state update
    params: z.object({}),
  }),
])

export type type = z.infer<typeof schema>
