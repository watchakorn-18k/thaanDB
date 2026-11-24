# ThaanDB logo — usage guide

**Idea:** a **T** built from table cells: a teal header row over one data column. It is the T
of *Thaan* (ฐาน, "base") and a table at the same time, and it avoids the usual database
cylinder.

All files are in [`logo/`](logo/). The presentation board is [`presentation.html`](presentation.html)
(slides as PNG in [`slides/`](slides/)). Early concepts are kept in [`concepts/`](concepts/).

## The parts

| Part | File | Use it for |
|---|---|---|
| Symbol | `thaandb-symbol.svg` | App icon art, avatars, anywhere square |
| Small symbol | `thaandb-symbol-small.svg` | 24 px and below (pixel-snapped: 4 px cells, 1 px gaps at 16 px) |
| Horizontal lockup | `thaandb-horizontal.svg` | Default: README, website header, docs |
| Stacked lockup | `thaandb-stacked.svg` | Square-ish spaces: splash screen, stickers, slides |
| Wordmark | `thaandb-wordmark.svg` | Only where the symbol is already shown nearby |
| App icon | `thaandb-symbol-app-icon.svg`, `thaandb-app-icon-1024.png` | macOS / desktop icon source |
| Web icons | `favicon.svg`, `favicon.ico`, `favicon-*.png`, `apple-touch-icon.png`, `icon-192/512.png`, `maskable-512.png`, `site.webmanifest`, `head-snippet.html` | Website (paste `head-snippet.html` into `<head>`) |

Every part has versions for light backgrounds (default), dark backgrounds (`-reversed` with its
own ink background, `-on-dark` transparent), and one colour (`-black`, `-white`, `-mono-0d9488`).
`favicon.svg` switches to the dark-background colours by itself in dark mode.

The wordmark is drawn as paths on a geometric grid (stroke 34 units, horizontals 30). It does
not use a font, so there is no font licence to worry about.

## Colours

| Name | HEX | RGB | CMYK (approx.) | Role |
|---|---|---|---|---|
| Thaan Teal | `#0D9488` | 13 148 136 | 91 0 8 42 | Header row on light backgrounds |
| Teal Light | `#2DD4BF` | 45 212 191 | 79 0 10 17 | Header row on dark backgrounds |
| Ink | `#0F1E1D` | 15 30 29 | 50 0 3 88 | Column and wordmark on light; app icon tile |
| White | `#FFFFFF` | 255 255 255 | 0 0 0 0 | Column and wordmark on dark |

- Contrast: Ink on white 17.2:1, Teal Light on Ink 9.2:1, Teal on white 3.7:1. That is fine
  for a graphic mark (WCAG needs 3:1), but do **not** use Thaan Teal for small body text on white.
- The teal belongs to the same family as the app's accent (`#14b8a6`, see `docs/spec/ui.md`).
- CMYK values are converted, not proofed. No Pantone match has been checked; proof before print.

## Clear space and minimum size

- **Clear space:** keep at least **one cell** (the width of one square in the T) empty on every
  side of the logo. Nothing else should go inside that space.
- **Minimum size:** symbol 16 px (use the small symbol below 24 px). Horizontal lockup 96 px
  wide on screen, 25 mm in print. Below that, use the symbol alone.

## Backgrounds

- Light backgrounds: the full-colour version.
- Dark backgrounds (Ink or darker): the `-reversed` / `-on-dark` version.
- Photos or busy patterns: put the logo on a plain Ink or white panel first.
- Mid-tone or brand-colour backgrounds: use one colour, either `-white` or `-black`, whichever
  has more contrast.

## Do not

- Change the colours, or swap which row is teal (the teal row is always the header).
- Add or remove cells, change the gaps, or round the cells more.
- Stretch, skew, rotate, outline, or add shadows, glows or gradients.
- Re-typeset "ThaanDB" in another font, or change the spacing between the symbol and the wordmark.
- Use the full-colour version on dark backgrounds (the Ink column disappears).

## Before public launch

- These marks have not had a trademark search. Do a professional search (trademark databases
  and a reverse image search) before the public launch.
- Wiring the icon into the app (`target/ThaanDB.app` / window) is a GUI change. By project rule
  it starts with `/unslop` (see `CLAUDE.md`).
