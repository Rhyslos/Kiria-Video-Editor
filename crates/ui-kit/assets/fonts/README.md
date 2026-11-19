# Bundled fonts

| File | Used for | Licence |
|---|---|---|
| `Inter_400Regular.ttf`, `Inter_600SemiBold.ttf` | Interface text | SIL Open Font License 1.1, see `Inter-OFL.txt` |
| `JetBrainsMono_400Regular.ttf` | Timecodes and numbers | SIL Open Font License 1.1, see `JetBrainsMono-OFL.txt` |
| `DejaVuSans_Arabic.ttf` | Arabic letters (the keyboard shortcuts page's Arabic keyboard), behind Inter: DejaVu Sans cut down to the Arabic block (U+0600–06FF) with fonttools' `pyftsubset`, about 40 KB | Bitstream Vera licence (DejaVu's changes are public domain), see `DejaVu-LICENSE.txt` |

The licence texts must ship with every build that includes the fonts. The fonts are compiled into the binary with `include_bytes!`.
