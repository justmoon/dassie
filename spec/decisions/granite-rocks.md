# Kopje's rocks are granite: gray blocks with hard edges, stacked, and drawn by one generator that a test holds to the rules (not drawn yet)

**Areas:** [Website](../areas/website.md). Not drawn yet ([roadmap](../roadmap.md#kopje-across-dassie)): on `main`, the scenes' rocks are Terracotta, as [visual-identity](visual-identity.md) has them.

## Facts

- In Kopje the rocks are the ledgers, and they carry the identity's idea: solid rocks, nimble dassies ([visual-identity](visual-identity.md)). They are in the footer of every page of dassie.land, in the brand page's scenes, its kopje line and its big sun, and in the far horizon at the foot of the home page.
- People who see those rocks read them as droppings. A shape reads so when it has four things together: a dark, warm brown; a soft, rounded outline; one lump with nothing inside it to say what it is made of; and the size of the animal beside it. Every rock in the scenes has all four. They are filled in dark red-browns, one flat fill each; their corners are curves; each is one mound; and a dassie lies on top of a rock hardly longer than itself, which is the outline of the emoji.
- The mark had the same trouble, and its rock became ink ([visual-identity](visual-identity.md#accepted)). That changed one drawing and left the rule: the palette still gives Terracotta to the kopjes in every scene, so each new drawing brings the brown back. The brand page already asks the mark's rock for "flat faces and tight corners: rock, not dough", and nothing holds the scenes to it.
- Real kopjes are granite. The rock splits along joints into blocks and slabs, which weather at their edges and lie stacked on each other, gray to pinkish gray; a block is many times the length of a dassie. Seen from far away, through the haze, rock turns paler and cooler.
- The node circles, every node's avatar, are to be the sun with a rock of the node's own, drawn from its node ID. No code draws them yet; whatever does needs rocks drawn from a seed.

## Decision

**A rock in Kopje is granite: gray, faceted, stacked, and large beside a dassie. One generator in the repo draws every rock, and a test holds what it draws to these rules.**

- **Gray, not brown.** A rock's faces are grays: their chroma stays below a bound that the brand page sets, so they read as stone whatever their hue leans to. The face turned away from the sun is a step darker than the face toward the viewer, and far kopjes are paler and cooler than near ones. The rim of Sunrise on the top edges stays, so the backlit look and the line of light between rock and dassie stay too.
- **Terracotta is earth, never a rock.** It keeps its place for grounds, bands and cards, such as the balance card and the veld grass, and for nothing that stands on the horizon as a closed shape.
- **Flat faces, hard corners, joints.** A rock's outline is straight lines whose corners are cut short, never rounded; tops are flat or tilted slabs, never domes. Each rock shows its joints: a crack across a face, or the rim of light where one block rests on another.
- **Stacked.** A kopje is two or more blocks: a broad base and smaller blocks on it. Only a far kopje, too small for its parts to show, is a single slab, and then a low and wide one.
- **Bigger than the dassies.** A block on which a dassie sits or lies is at least three times the dassie's length, and a dassie lying down lies on a ledge or toward one end, never on the crown of a small rock.
- **One generator, and a test.** Code in the repo draws the rocks: a generator in `packages/ui`, which the site and the wallet share, draws each kopje from a seed and a few parameters (width, number of blocks, distance). A script writes the brand page's drawings and the footer's with it, and the wallet draws a node circle's rock with it at run time, seeded by the node ID. A test draws the files again and fails when one differs from what is written, or when a rock the generator draws breaks one of the rules above: a face's chroma, a rounded corner, a kopje of one block near the viewer, a dassie's rock too short. Only the mark, a drawing on its own with its own rules, stays drawn by hand.

## Accepted

- The scenes are less warm: gray rocks under an apricot sun are cooler than brown ones. The warmth comes from the sun, the sandstone ground and the Terracotta earth instead.
- Every drawing with rocks is drawn again: the footer, both far horizons, the brand page's scenes, kopje line and big sun.
- The generator is code to keep, and its rules are harder to bend for a single drawing. A scene that wants a rock the rules forbid changes the rules, on the brand page and here, rather than drawing around them.
- A rule a test can check catches the four things above, not every way a shape can be misread. The brand page's review, by people, is still where a drawing is judged.

## Rejected

- *Keep Terracotta, and reshape the rocks:* faceted, stacked brown rocks read better, but brown is what the mark's trouble came down to, and every softer drawing later on would bring the old reading back.
- *Make the rocks gray, and keep their shapes:* smooth gray mounds read as dough, pebbles or potatoes rather than as kopjes; the form has to say rock as much as the color.
- *A lighter Terracotta, the clay orange:* a lighter brown is still brown, a mound of it reads as clay, and orange rock against the apricot sun loses the edge that makes the scene.
- *Ink rocks, as the mark's:* the mark has one dassie and one rock, parted by a rim of light. A scene's many ink dassies on ink rocks melt into one dark band, and the scene no longer shows ledgers apart from the nodes on them.
- *Red sandstone mesas, flat-topped and layered:* strata read clearly as rock, but they are red, and a mesa is tall where Kopje lies low; kopjes are granite.
- *Redraw each drawing by hand, to the brand page's rules:* that is how the mark was fixed, and the rule it followed already asks for flat faces and tight corners. Rules only on a page did not keep the scenes to them; a generator and a test do.
- *An image model in CI, asked what each drawing shows:* its answers change from run to run and need the network, and CI is deterministic.
