import z from "zod"

export const schema = z.discriminatedUnion("method", [
  z.object({
    id: z.optional(z.union([z.string(), z.number()])),
    method: z.literal("start"),
    params: z.object({
      root: z.string(),
      base: z.string(),
      entry: z.string(),
    }),
  }),
  z.object({
    id: z.optional(z.union([z.string(), z.number()])),
    method: z.literal("exit"),
    params: z.tuple([]),
  }),
])

export type ServerRequest = z.infer<typeof schema>
