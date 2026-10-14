import { assert, assertEquals } from "@std/assert";
import { Layout } from "./layout.ts";

const positions = (layout: Layout) =>
  [...layout.bodies.entries()].map(([id, b]) => [id, b.x, b.y]);

Deno.test("the same table settles the same way", () => {
  const links = [{ a: "a", b: "b" }, { a: "b", b: "c" }];
  const first = new Layout();
  first.update(["a", "b", "c"]);
  first.settle(links);
  const second = new Layout();
  second.update(["a", "b", "c"]);
  second.settle(links);
  assertEquals(positions(first), positions(second));
  assert(first.settled);
});

Deno.test("linked nodes end up nearer than unlinked ones", () => {
  const layout = new Layout();
  layout.update(["a", "b", "c", "d"]);
  layout.settle([{ a: "a", b: "b" }]);
  const distance = (x: string, y: string) => {
    const [p, q] = [layout.bodies.get(x)!, layout.bodies.get(y)!];
    return Math.hypot(p.x - q.x, p.y - q.y);
  };
  assert(distance("a", "b") < distance("c", "d"));
  // A node that leaves goes; the others keep their place.
  const before = layout.bodies.get("a")!.x;
  layout.update(["a", "b", "c"]);
  assertEquals(layout.bodies.has("d"), false);
  assertEquals(layout.bodies.get("a")!.x, before);
});
