import { ia5String, sequence } from "@dassie/lib-oer"

export const peeringInfoSchema = sequence({
  address: ia5String([25, 35]),
})
