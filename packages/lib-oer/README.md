# lib-oer

## What is this?

A serialization/deserialization library for [Octet Encoding Rules](https://www.itu.int/rec/T-REC-X.696/en) (OER; X.696) which provides automatic TypeScript type inference. Inspired by [zod](https://github.com/colinhacks/zod/).

## Example

```typescript
import { Infer, sequence, uint8Number, utf8String } from "@dassie/lib-oer"

const personSchema = sequence({
  name: utf8String(),
  age: uint8Number(),
})

type Person = Infer<typeof personSchema>

const result = personSchema.parse(uint8ArrayContainingPerson)

if (result.success) {
  const person: Person = result.value
} else {
  console.error("Parsing failed:", result.toString())
}
```

## Limitations

* [ ] Does not support enforcing custom limits on mantissa or exponent for REAL values
* [ ] Does not support REAL values that exceed float64 limits
* [ ] Does not support REAL values with a base other than 2
* [ ] No support for optional fields and defaults in sequences
* [ ] No support for extensions
* [ ] Bitstring values are only boolean[] rather than Record<string, boolean>
