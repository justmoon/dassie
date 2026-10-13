# Website placeholder

This directory exists only so that the Vercel project behind dassie.land can deploy branches of the rewrite. Until the real website exists, it serves a single static page from `public/`.

## Why it is here

- **The path.** The Vercel project's Root Directory setting is `packages/app-website`, where the original keeps its Astro site. Without this directory, every Vercel deployment of a rewrite branch fails.
- **`vercel.json`.** It overrides the project's Astro settings: it turns off the framework preset and skips install and build, so Vercel serves `public/` as it is.
- **`package.json`.** It exists only to pin a Node version Vercel still supports. The project setting may name a discontinued one.

## When it goes

The rewrite's website will live in `apps/website` (spec 7 §3.1, milestone M9). When it does:

1. Change the Vercel project's Root Directory to `apps/website`.
2. Delete this directory.
