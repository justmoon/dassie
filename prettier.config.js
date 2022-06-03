module.exports = {
  plugins: [
    require("prettier-plugin-tailwindcss"),
    require("@trivago/prettier-plugin-sort-imports"),
  ],
  importOrder: ["^node:", "^[./]"],
  importOrderSeparation: true,
  importOrderSortSpecifiers: true,
  semi: false,
}
