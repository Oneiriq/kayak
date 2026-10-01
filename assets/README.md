# Kayak brand assets

The Kayak mark is a letter K. Its stem is a kayak hull seen from above,
and its two arms are the wake the hull leaves behind as it holds its line.

## Files

| File | Size | Use it for |
| --- | --- | --- |
| `banner.png` | 2560x1280 | The header image at the top of the README. |
| `social-preview.png` | 1280x640 | The GitHub social preview (see below). Also works for link cards and slides. |
| `icon.svg` | 512x512 | The app icon: the mark on a dark rounded tile. Use it for avatars, package registries, and documentation sites. |
| `icon-512.png` | 512x512 | The same icon as a PNG, for places that do not accept SVG. |
| `favicon-32.png` | 32x32 | A browser tab icon. |
| `mark-dark.svg` | 512x512 | The bare mark for dark backgrounds. The background is transparent. |
| `mark-light.svg` | 512x512 | The bare mark for light backgrounds. The background is transparent. |
| `lockup-dark.png` | 1384x352 | The mark with the `KAYAK` wordmark on a dark tile. |
| `lockup-light.png` | 1384x352 | The mark with the `KAYAK` wordmark on a light tile. |

To show the right bare mark for the reader's GitHub theme:

```html
<picture>
  <source media="(prefers-color-scheme: dark)" srcset="assets/mark-dark.svg">
  <img src="assets/mark-light.svg" alt="Kayak" width="96">
</picture>
```

## Colors

| Name | Hex | Role |
| --- | --- | --- |
| Hull | `#0E2229` | Dark ground, and the wake on light backgrounds. |
| Coral | `#DD6B47` | The hull on dark backgrounds, and the accent color. |
| Coral (light) | `#D25D38` | The hull on light backgrounds. |
| Foam | `#E4EEEA` | Light ground, and the wake on dark backgrounds. |
| Teal | `#3C8F97` | Secondary color for diagrams and highlights. It does not appear in the mark. |

The wordmark is set in Source Sans Pro, weight 700, in capitals with wide
letter spacing.

## Social preview

GitHub does not read the social preview from the repository. To set it,
open the repository's Settings, and under General > Social preview, upload
`social-preview.png`.

## Source

The artwork is drawn in Penpot, in the Brands project, file "Repository
Brands". The mark is the "kayak mark" component with Concept "K2 Hull K"
and Ink "Colour". The banner is the kayak README banner board on the
Family page. Export from there if you need another size or format.
