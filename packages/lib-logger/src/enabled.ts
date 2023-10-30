/**
 * Create a function that will check if a given component is part of a provided scope.
 *
 * The scope is a comma-delimited list of patterns. Each pattern must match the entire name of the component. Asterisks (*) can be used to match any number of characters. Patterns are checked in the order given. If the pattern starts with a minus sign (-) and matches the component, then the function will return false immediately without testing further patterns.
 *
 * For example, if the scope is "foo*", then the function will return true for "foo" and "foobar" but false for "bar". If the scope is "-foobar,foo*", then the function will return true for "foo" but false for "foobar" and "bar".
 *
 * Please note that more precise patterns should come first. If the scope is "foo*, -foobar", then the function will be true for "foobar" because the first pattern matches and the rest are not evaluated.
 *
 * @param scope - String containing comma-delimited patterns
 * @returns Function that will check if a component is part of the scope
 * @alpha
 */
export function createEnableChecker(scope: string): (name: string) => boolean {
  if (!scope) return () => false

  const patterns = scope.split(/[\s,]+/).map((pattern): [RegExp, boolean] => {
    const negative = pattern.startsWith("-")
    const regex = new RegExp(
      `^${(negative ? pattern.slice(1) : pattern).replace("*", ".*?")}$`,
    )
    return [regex, negative]
  })

  return (name) => {
    for (const [regex, negative] of patterns) {
      if (regex.test(name)) {
        return !negative
      }
    }
    return false
  }
}
