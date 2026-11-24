# Release notes: 2.0

Version *two* brings **faster** parsing, `strict` mode and a new [guide](docs/guide.md).
Snake_case names stay put, and 5 < 6 & 7 > 2.

## Changes in detail

1. Parser rewritten
   around a block tree.
2. Escapes: \*not emphasis\* and \`not code\`.
3. Images: ![logo](img/logo.png "Logo") are kept.

- Output is HTML-escaped
- Lists may be *nested* only one level:
  - not supported yet

> Note: the old flag `--legacy` is gone.
> Use `--strict` instead.

```sh
mdconv --strict notes.md > out.html
```

---

### Next

Unfinished fence follows:
```text
still code <tag>
