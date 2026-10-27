# A phrase says its realm and when it was made, and corrects its own errors

**Areas:** [Identity](../areas/identity.md#derivation-chain), [HTTP](../areas/http.md#setup), [Node](../areas/node.md#the-dassie-command), [Wallet](../areas/wallet.md#key-derivation), [overview](../overview.md#frozen-surfaces).

## Facts

- **The phrase is the owner's only backup,** and the top of the derivation chain: the node key, its node ID and every key to the owner's funds derive from it ([Identity](../areas/identity.md#derivation-chain)). It is a BIP-39 phrase today, turned into the 64-byte master seed by PBKDF2.
- **A phrase names no realm.** The same phrase is the same node, with the same node ID and name in DNS, on the testnet and in the live realm, so one name could mean one node in both ([public-domains](public-domains.md)), and an owner who keeps one phrase for both puts real money behind the keys they test with.
- **BIP-39 detects little and corrects nothing.** Its checksum is 4 bits in 12 words and 8 in 24, so a wrong word passes one time in 16, or 256, as a valid phrase of another node, and it has no room for a version.
- **Owners keep phrases on metal.** Cryptosteel and plates like it store four letters of each word, which identify a word of the BIP-39 English list, since all 2048 differ in their first four letters (a shorter word in full). They hold 24 words. A plate gets bent and scratched: a word may be lost, at a place the owner knows.
- **Nothing reads a Dassie phrase but Dassie.** Every key derives through Dassie's own tree of seed paths, so no other BIP-39 wallet finds the owner's funds from the phrase; BIP-39's encoding buys only its wordlist.
- **Ed25519 offers about 128 bits of security,** so a longer secret protects the node key no better.
- **The derivation chain is a frozen surface** ([overview](../overview.md#frozen-surfaces)), because owners outside a release depend on it. Today only testnet nodes do: the original was a development prototype that nobody ran, and the live realm is refused, so no node holds real money.
- **A phrase is made by a clock that may be wrong.** The wallet makes it in the owner's browser, whose clock can run ahead or behind.

## Decision

**A phrase is 20 words of the BIP-39 English list,** each word the index of its place in the list, an element of GF(2¹¹). Its 14 data words are 154 bits, written most significant first:

| Field | Bits | Holds |
|---|---|---|
| Kind | 6 | The layout and the realm: 0 is never valid; 1 is this layout in the test realm; 2 is this layout in the live realm; the rest are unassigned |
| Created | 20 | Hours since 2026-01-01T00:00:00Z, rounded down, when the phrase was made: up to 2145-08-15. A generator makes a phrase only while its clock lies within the field, and, at setup, within a day of the node's clock, and shows the date with the new phrase: `Generated 2026-02-19` |
| Secret | 128 | From the OS's CSPRNG |

**Six check words follow,** a Reed–Solomon code over GF(2¹¹) with the primitive polynomial x¹¹ + x² + 1 and α = x. The 20 words are the coefficients of a polynomial, the first word the highest. The generator is g(x) = (x − α)(x − α²)…(x − α⁶), and the check words are the remainder of the data words, shifted up by six places, divided by g(x). Any two phrases differ in at least seven words.

**Reading a phrase:**

1. **Words.** A word is matched by its first four letters, or in full if it is shorter, in any case. A word not in the list, or one the owner marks as unreadable, is an erasure: its place is known, its value is not.
2. **Length.** Only 20 words are read. 12, 15, 18, 21 or 24 words are a BIP-39 phrase, another wallet's or a Dassie phrase from before this format, and are refused as that. Any other count is not a phrase.
3. **Correction.** The code restores the phrase within at most 2 errors at places not known and 2 × errors + erasures ≤ 5, so one check word is always left to catch a mistake, among the words read too. Beyond that it is refused as too damaged. Twenty random words of the list pass as a phrase about once in 3 × 10¹² when none is marked; correcting three unknown errors would let one in 2.4 × 10⁸ through.
4. **Kind.** Kind 0 is not a phrase; an unassigned kind is refused as made by a newer Dassie. A phrase whose kind names another realm than the reader expects is refused as a phrase of that realm, at setup and at login. These, and step 5, come before any correction is shown, so an owner never confirms a phrase that is refused next.
5. **Time.** A creation time more than a day after the reader's clock is refused, and the refusal names the date the phrase gives and says a phrase cannot be made in the future: the reader's clock is behind, the words are not the owner's phrase, or the clock of the machine that made it was ahead, which the refusal says can be read past by setting this clock beyond that date. A reader with no one to confirm, which starts with its host, reads only once the host's clock is synced.
6. **Never silently.** A phrase that needed correcting is shown, with the words that change marked, and taken only once the owner confirms it. A reader with no one to confirm, `dassie setup` and the market maker, refuses a phrase that needs correcting and names the positions of the words, never the words.

**The master seed** is HKDF-SHA256 with the secret as its key material, `dassie phrase` as its salt and the realm, `test` or `live`, as its info, 64 bytes long. The derivation takes the realm its caller expects, and refuses a phrase whose kind names another, so no caller can skip the check. Everything below the master seed stays as it is. The realm enters, not the kind, so a later layout can carry the same secret to the same node; the creation time does not enter at all.

## Accepted

- **The frozen derivation chain changes, once, before the first release.** From here on it is frozen again, with this format at its top. Every phrase made before is refused, and its node cannot be reached from a new phrase. The testnet's reset absorbs it: every machine that keeps a phrase, the bootstrap nodes and the market maker's included, gets a new one.
- **A reader whose clock is behind refuses a phrase until its clock is set right,** with the phrase's date in the refusal, so the owner sees why. Showing the date and taking the phrase anyway would let a wrong phrase through on a check that is meant to stop it.
- **A phrase made on a machine whose own clock ran ahead is refused by every correct reader until its date.** The generator's check compares the wallet's clock with the node's, which are one clock in the desktop app and for a browser on the node's own host, so it cannot catch that. The date shown with a new phrase lets the owner see it before writing the phrase down, a node whose clock is a day off cannot peer, so the owner learns of it before funding the node, and the refusal says how to read such a phrase anyway. An independent clock check before setup would close the case, but would make setup depend on the network.
- **One check word is kept back while words are marked unreadable,** so a phrase with five unreadable words is the most the code restores, not six.
- **20 words where BIP-39 needs 12 for the same 128-bit secret.** The eight more carry the kind, the creation time and the correction, and 20 still fits every plate made for 24.
- **Not a BIP-39 phrase,** so BIP-39 tools cannot check it. They could not use it either.
- **The creation hour is in the phrase.** Only a holder of the phrase reads it, and the holder holds the node.
- **The creation time ends in 2145.** A layout of its own, under a kind of its own, replaces this one long before.

## Rejected

- *BIP-39 as it is:* no realm, no version, four bits of check in twelve words, and no correction.
- *Grinding a tag into a BIP-39 phrase,* drawing secrets until a hash of the phrase starts with the realm's tag: it spends the secret's own bits to say what a field says for free.
- *Folding the realm into the code,* as bech32 folds its prefix into its checksum: in a code that corrects, the unwritten symbol spends a word of the distance, so a phrase three errors from its own realm's could be three from another's too.
- *A version and a realm in two fields:* one kind names each layout and realm that exists, so a layout need not exist in both realms, and no combination means nothing.
- *The kind in the master seed's derivation:* each new layout would be a new node, and moving to it would move the owner's funds.
- *SLIP-39:* its checksum detects but its standard does not correct, it is 20 words for 128 bits and 33 for 256, and it encrypts the secret under a passphrase of its own.
- *codex32:* the strongest correction, but 48 characters of the bech32 alphabet rather than words, slower to read back and a poor fit for plates of words.
- *PBKDF2:* stretching protects secrets people choose; this one is 128 random bits.
- *A 256-bit secret:* six more words, for no more security than the node key has.
