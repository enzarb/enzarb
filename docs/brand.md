# enzarb brand

Identity for the UI, docs and release material. Assets live in `ui/static/brand/`; the theme tokens are in `ui/src/lib/styles/global.scss`.

## Tone

The Architect: a high-performance specialist. Precise, utilitarian, low-friction.

- No marketing fluff. Short, concrete sentences.
- System-oriented: say what is correct, deterministic and resilient, and what is not.
- Developer-native: CLI- and API-first. Show the command or the field name.

## Palette

Dark-first, "terminal precision". The dark theme uses the brand values as they are.

| Role | Name | Hex | Use |
|---|---|---|---|
| Canvas | Deep Slate / Obsidian | `#0B0F17` | Primary background (`--ez-bg`) |
| Surface | Layered Navy Slate | `#131B2B` | Cards, panels, code blocks (`--ez-surface`) |
| Border | Muted Steel | `#232F46` | Dividers (`--ez-border`) |
| Accent primary | Electric Cyan | `#00F0FF` | Primary CTA, focus rings (`--ez-accent`) |
| Accent secondary | Laser Emerald | `#10B981` | Success, healthy and passing states (`--ez-success-fg`) |
| Text primary | Crisp White | `#F8FAFC` | Headings, body (`--ez-text`) |
| Text secondary | Muted Slate | `#94A3B8` | Labels, metadata (`--ez-text-muted`) |

The logo gradients run sky `#0EA5E9`, cyan `#00D8F6`, aqua `#00F5D4`, to emerald `#10B981`/`#22C55E`. The wordmark is `#F1F5F9` with a cyan `#00E5FF` ".dev".

### Light theme

The light theme keeps the neutral structure and swaps the accent for a darker cyan and emerald:

| Token | Value | Why |
|---|---|---|
| `--ez-accent`, `--ez-accent-text` | `#0E7490` | Brand cyan `#00F0FF` is unreadable on white. `#0891B2` is only about 3.7:1 on white, which is fine for large graphics but fails 4.5:1 for text and for white-on-accent button labels, so it is not used for either. |
| `--ez-success-fg` | `#047857` | Dark emerald for text on white and on the success tint. |
| `--ez-logo-ink`, `--ez-logo-dev` | `#0B0F17`, `#0E7490` | On-light wordmark colours. |

Status colours (warning, danger, info) are not part of the brand palette. They keep their previous hues, tuned per theme to pass AA.

### Contrast

Every token pair is checked against WCAG 2.x AA by `ui/src/lib/styles/contrast.spec.ts`, which parses `global.scss`. The spec is the source of truth for the measured ratios; change a token and the spec tells you if it breaks.

- Text (`--ez-text`, `-muted`, `-subtle`, `--ez-accent-text`) on `--ez-bg`, `--ez-surface`, `--ez-surface-sunken` and `--ez-surface-hover`: at least 4.5:1.
- `--ez-accent-fg` on `--ez-accent` and `--ez-accent-hover`: at least 4.5:1.
- Status `-fg` on its `-bg` tint and on the surface: at least 4.5:1.
- Control borders (`--ez-control-border`), the accent (focus ring) against bg and surface: at least 3:1 (WCAG 1.4.11).
- Decorative borders (`--ez-border`) carry no meaning and are exempt.

## Typography

- Display and body: Inter (Geist Sans is an acceptable substitute), 15-16px, line-height 1.6.
- Code: JetBrains Mono (or Fira Code).
- The font stacks in `global.scss` name these faces first and fall back to the system stack. The app never requests fonts from a third party. Self-hosted font files are not bundled yet; see "Fonts" below.

### Fonts

Bundling Inter and JetBrains Mono (OFL woff2, Latin subset, `font-display: swap`, served from `'self'`) is a follow-up: the build environment for this change could not download the files. Add them under `ui/src/lib/assets/fonts/` with their licences and `@font-face` rules in `global.scss`.

## Icons

Monoline, 1.5px stroke (Lucide style), round caps and joins, `currentColor`. On hover or active, the stroke lights up cyan (`--ez-accent`) or, for status, emerald.

## Logo

| File | Use |
|---|---|
| [`enzarb-sheet.svg`](../ui/static/brand/enzarb-sheet.svg) | The maintainer's source sheet (1200x900, with background). Never edit; the other files derive from it. |
| [`enzarb-mark.svg`](../ui/static/brand/enzarb-mark.svg) | Full emblem, transparent, no drop shadow. Heroes and documents, 96px and up. |
| [`enzarb-lockup.svg`](../ui/static/brand/enzarb-lockup.svg) | Emblem above the wordmark, transparent. On dark backgrounds. |
| [`enzarb-wordmark.svg`](../ui/static/brand/enzarb-wordmark.svg) | Wordmark only, for dark backgrounds. |
| [`enzarb-wordmark-on-light.svg`](../ui/static/brand/enzarb-wordmark-on-light.svg) | Wordmark in ink and dark cyan, for light backgrounds. |
| [`enzarb-mark-flat.svg`](../ui/static/brand/enzarb-mark-flat.svg) | Simplified flat mark for 16-64px: a cyan E and an emerald Z on an obsidian squircle. Source of the favicon, app icons and the nav logo. |

The bevels, soffits and highlights of the full emblem do not read below about 96px, which is why the flat mark exists. The flat mark is two flat facets and no gradients, filters or strokes.

Regenerate everything derived (all but the sheet and the flat mark) with `mise run ui:icons`: `ui/scripts/build-brand.mjs` writes the cropped SVGs, and `ui/scripts/generate-icons.mjs` writes `favicon.svg`, `favicon.ico` (16/32/48), `apple-touch-icon.png` (180), the 192/512 PNGs, the maskable variants and the notification badge. `manifest.webmanifest` lists them.

### Usage

- **Clear space:** keep at least half the height of the emblem's E stem (about 1/12 of the mark's height) free on all sides.
- **Minimum size:** flat mark 16px; full emblem and lockup 96px wide.
- **On dark:** use the emblem, lockup or wordmark as supplied. **On light:** use the flat mark (it carries its own obsidian background) or the on-light wordmark. In the app, `Logo.svelte` colours the wordmark from `--ez-logo-ink` and `--ez-logo-dev`, so it follows the theme.
- **Don't:** recolour the gradients, stretch or rotate the mark, add a drop shadow or outline, place the full emblem below its minimum size, put the on-dark wordmark on a light background, or set the name in another typeface.
