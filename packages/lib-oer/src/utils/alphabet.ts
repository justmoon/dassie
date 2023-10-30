export type Alphabet = string

/** Uppercase Latin ASCII characters */
export const uppercaseLetters = "ABCDEFGHIJKLMNOPQRSTUVWXYZ"
/** Lowercase Latin ASCII characters */
export const lowercaseLetters = "abcdefghijklmnopqrstuvwxyz"
/** Upper- and lower-case Latin ASCII characters */
export const letters = uppercaseLetters + lowercaseLetters
/** Digits from 0-9 */
export const digits = "0123456789"
/** Corresponds to ASN.1 PrintableString */
export const printable = letters + digits + " '()+,-./:=?"

export const alphabetToFilterArray = (alphabet: Alphabet): boolean[] => {
  const filterArray = Array.from<boolean>({ length: 128 }).fill(false)
  for (const character of alphabet) {
    const codePoint = character.codePointAt(0)!

    if (codePoint > 127) {
      throw new Error(
        "Restricted character sets with UTF-8 are not supported yet",
      )
    }

    filterArray[codePoint] = true
  }

  return filterArray
}
