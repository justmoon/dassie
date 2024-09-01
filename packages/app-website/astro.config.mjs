import starlight from "@astrojs/starlight"
import { defineConfig } from "astro/config"

// https://astro.build/config
export default defineConfig({
  site: "https://dassie.land",
  integrations: [
    starlight({
      title: "Dassie",
      social: {
        github: "https://github.com/justmoon/dassie",
        discord: "https://discord.gg/HqdGNfs9kq",
      },
      sidebar: [
        {
          label: "Guides",
          items: [{ label: "Quick Start", slug: "guides/quick-start" }],
        },
      ],
      expressiveCode: {
        themes: ["dracula", "github-light"],
      },
    }),
  ],
})
