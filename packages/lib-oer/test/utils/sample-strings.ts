// See: https://www.cl.cam.ac.uk/~mgk25/ucs/examples/UTF-8-test.txt
export const utf8TestValues = [
  ["", ""],
  ["a", "61"],
  ["κόσμε", "cebae1bdb9cf83cebcceb5"],
  ["\u{0}", "00"],
  ["\u{80}", "c280"],
  ["\u{800}", "e0a080"],
  ["\u{10000}", "f0908080"],
  ["\u{7F}", "7f"],
  ["\u{7FF}", "dfbf"],
  ["\u{FFFF}", "efbfbf"],
  ["\u{10FFFF}", "f48fbfbf"],
  ["😄", "f09f9884"],
]
