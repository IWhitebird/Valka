# Valka brand

The mark is a **shard field**: a 5 × 5 grid of shards with a V of owned cells. It stands
for the 4096 fixed shards and single-writer ownership at the heart of Valka's storage
design, and it matches the shard heatmap in the dashboard.

Every file here is generated. To change anything, edit `build.py` and run:

```bash
python3 assets/brand/build.py
```

The script needs `fontTools` and `Pillow`. It downloads the OFL fonts into `.cache/`,
writes the files below, and copies the web icons into `web/public/`, `www/app/` and
`www/public/`. All text in the SVGs is outlined, so they render identically everywhere,
including GitHub.

## Files

| File | Use |
|---|---|
| `mark.svg` | Default mark in Patina, for any ground |
| `mark-on-dark.svg`, `mark-on-light.svg` | Mark tuned for dark or light grounds |
| `mark-mono-black.svg`, `mark-mono-white.svg` | One-color print, embossing, stickers |
| `logo-on-dark.svg`, `logo-on-light.svg` | Horizontal lockup: mark and `valka` wordmark |
| `banner-dark.svg`, `banner-light.svg` | README banner (switched with `<picture>`) |
| `social-preview.png` | GitHub social preview, 1280 × 640. Upload it in repository Settings → Social preview |
| `favicon.svg`, `favicon.ico`, `favicon-16.png`, `favicon-32.png` | Browser icons, snapped to whole pixels |
| `apple-touch-icon.png`, `icon-192.png`, `icon-512.png` | Home-screen and app icons on Ink |

## Color

| Name | Hex | Role |
|---|---|---|
| Patina | `#3E9F88` | Primary brand color |
| Patina Light | `#67C2AA` | Mark, links and primary buttons on dark grounds |
| Patina Deep | `#1F5B4F` | Mark and primary buttons on light grounds |
| Copper | `#C27A48` | Rare highlight on dark grounds, with Ink text on top |
| Copper Deep | `#9E5A2E` | Copper for text on light grounds |
| Ink | `#111816` | Dark ground |
| Slate | `#1B2422` | Dark surfaces |
| Frost | `#EEF2EF` | Light ground, text on dark |

Status colors (success, warning, error) stay separate from the brand. Success uses a
yellower green than Patina and always comes with an icon or label.

## Type

- **Schibsted Grotesk**, ExtraBold, for the wordmark and headings. The wordmark is always
  lowercase `valka`.
- **Instrument Sans** for body text and UI.
- **JetBrains Mono** for code, IDs, LSNs and logs.

## Using the mark

- Keep clear space of at least one cell width on every side.
- Don't use it smaller than 16px. Below 24px, use the pixel-snapped favicon files.
- Don't recolor individual cells, rotate the grid, add effects or gradients, or put the
  mark on a busy image. On photos, use the one-color versions on a solid panel.
- Dim cells are part of the mark. Drop them only in the one-color versions for media that
  can't print tints.
