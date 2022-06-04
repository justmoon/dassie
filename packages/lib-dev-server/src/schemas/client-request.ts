import z from "zod"

export const schema = z.discriminatedUnion("method", [
  z.object({
    id: z.union([z.string(), z.number()]),
    method: z.literal("fetchModule"),
    params: z.tuple([z.string()]),
  }),
  z.object({
    id: z.union([z.string(), z.number()]),
    method: z.literal("resolveId"),
    params: z.tuple([z.string(), z.union([z.string(), z.null()])]),
  }),
])

export type ClientRequest = z.infer<typeof schema>
