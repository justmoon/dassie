export const isGeneratorFunction = (value: unknown) =>
  typeof value === "function" &&
  Symbol.toStringTag in value &&
  value[Symbol.toStringTag] === "GeneratorFunction"
