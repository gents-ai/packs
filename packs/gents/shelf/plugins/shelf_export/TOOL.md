# shelf_export

Build an EPUB 3 edition from an ordered, reviewed manuscript in a folder.
Call with `path` (the folder), `manuscript` (a file name, default `manuscript.json`)
and `output` (a new `.epub` file name). Existing output files are never replaced; an identical retry returns the same receipt.

The manuscript is UTF-8 JSON:
```json
{"identifier":"book:edition-1","title":"A Book","author":"An Author","language":"en","modified":"2026-01-01T00:00:00Z","chapters":[{"title":"Chapter One","paragraphs":["First paragraph.","Second paragraph."]}]}
```
Chapters and paragraphs retain their given order. Text is literal, not HTML or
Markdown: turn reviewed prose into paragraphs before exporting. This exporter
supports text editions; images, inline styling, footnote links and audio overlays
are not supported. Do not flatten those features without telling the user.

The folder must permit plugin writes. The result gives `path`, `sha256`,
`chapter_count` and `bytes`. Store that receipt with the edition's book identifier.
Only simple file names are accepted; put both files directly in the bound folder.
