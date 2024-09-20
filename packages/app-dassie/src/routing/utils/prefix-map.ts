/**
 * A key-value map where the members' keys represent prefixes.
 *
 * @example
 *   const map = new PrefixMap()
 *   map.insert("foo", 1)
 *   map.insert("bar", 2)
 *   map.get("foo")     // ⇒ 1
 *   map.get("foo.bar") // ⇒ 1 ("foo" is the longest known prefix of "foo.bar")
 *   map.get("bar")     // ⇒ 2
 *   map.get("bar.foo") // ⇒ 2 ("bar" is the longest known prefix of "bar.foo")
 *   map.get("random")  // ⇒ null
 */
export default class PrefixMap<TKey extends string, TValue> extends Map<
  TKey,
  TValue
> {
  // TODO: Use a radix tree instead of a sorted array

  /**
   * All keys, sorted by length (descending) and then lexicographically (descending).
   */
  protected readonly sortedKeys: TKey[] = []

  /**
   * Remove all entries from the map.
   */
  override clear() {
    this.sortedKeys.length = 0
    super.clear()
  }

  /**
   * Find the value associated with the longest matching prefix key.
   */
  lookup(key: string) {
    const prefix = this.lookupKey(key)

    return prefix === undefined ? undefined : this.get(prefix as TKey)
  }

  /**
   * Find the longest matching prefix key.
   */
  lookupKey(key: string) {
    // First, we check for an exact match as a shortcut
    if (this.get(key as TKey)) return key

    return this.sortedKeys.find((prefix: string) => key.startsWith(prefix))
  }

  /**
   * Get all keys that start with a certain prefix.
   *
   * @param prefix - The prefix to search for.
   * @returns An iterator over all keys that start with the given prefix.
   */
  *filterPrefixKeys(prefix: string): IterableIterator<TKey> {
    for (const key of this.sortedKeys) {
      if (key.startsWith(prefix)) {
        yield key
      }
    }
  }

  /**
   * Get all values corresponding to keys that start with a certain prefix.
   *
   * @param prefix - The prefix to search for.
   * @returns An iterator over all values that start with the given prefix.
   */
  *filterPrefix(prefix: string): IterableIterator<TValue> {
    for (const key of this.filterPrefixKeys(prefix)) {
      yield this.get(key)!
    }
  }

  /**
   * Insert the key and value into the map.
   *
   * If the key already exists, the value will be overwritten.
   *
   * @param key - The key to insert.
   * @param value - The value to insert.
   * @returns The map itself, for chaining.
   */
  override set(key: TKey, value: TValue) {
    if (!this.has(key)) {
      const index = this.sortedKeys.findIndex((candidate) => {
        if (key.length === candidate.length) {
          return key > candidate
        }
        return key.length > candidate.length
      })

      if (index === -1) {
        this.sortedKeys.push(key)
      } else {
        this.sortedKeys.splice(index, 0, key)
      }
    }
    super.set(key, value)
    return this
  }

  /**
   * Remove a key from the map.
   *
   * @param key - The key to remove.
   * @returns True if the key was removed, false if it was not in the map.
   */
  override delete(key: TKey) {
    // Remove key from sorted array
    const index = this.sortedKeys.indexOf(key)
    if (index !== -1) this.sortedKeys.splice(index, 1)

    // Remove key from map
    return super.delete(key)
  }

  /**
   * Return the prefix map as a plain JavaScript object.
   *
   * @returns A plain JavaScript object with the same keys and values as the map.
   */
  toJSON() {
    return Object.fromEntries(this)
  }
}
