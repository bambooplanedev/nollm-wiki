# Remove compiler chrome from a rendered wiki page before publishing:
#   - the backlink sections "## Imported by" and "## Mentioned in" (Quartz
#     draws its own backlinks);
#   - the trailing "## Notes" section when it holds only the compiler's
#     placeholder (hand-written notes are kept).
# The compiler writes its list sections before the body, so only "## "
# headings above the first "## Body" (or "## Notes", for a page with no body)
# are taken as chrome; the same headings inside a markdown body are kept.
BEGIN { placeholder = "_(add your own notes here — preserved on recompile)_" }
{ line[NR] = $0 }
!head && ($0 == "## Body" || $0 == "## Notes") { head = NR }
$0 == "## Notes" { notes = NR }
END {
    if (!head) head = NR + 1
    drop_notes = notes
    for (i = notes + 1; notes && i <= NR; i++)
        if (line[i] != "" && line[i] != placeholder) drop_notes = 0
    for (i = 1; i <= NR; i++) {
        if (drop_notes && i >= notes) break
        if (i < head && line[i] ~ /^## /)
            skip = (line[i] == "## Imported by" || line[i] == "## Mentioned in")
        if (i >= head) skip = 0
        if (!skip) print line[i]
    }
}
