# 0001 · Project model: imbl, and migrations on text

**Date:** 10 November 2026

**Decision:**
1. Persistent collections come from `imbl` rather than `im`.
2. File-format migrations turn old file *text* into new file text, using frozen copies of the old types, instead of editing a generic value tree.

**Why:**
1. `imbl` is the maintained continuation of `im`, with the same design and the same MPL-2.0 licence; `im` has not had a release in years.
2. RON's generic value tree drops enum variant names (a `Video` track kind comes back as an empty value), so a migration that goes through it corrupts the file. Text in, text out, with typed structs in between, keeps every detail.

**Instead of:** `im` as named in the original plan; migrations on `ron::Value`.
