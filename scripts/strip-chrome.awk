# Remove compiler chrome from a rendered wiki page before publishing:
#   - the backlink sections "## Imported by" and "## Mentioned in" (Quartz
#     draws its own backlinks);
#   - the compiler's trailing "## Notes" section (the last "## Notes" to the
#     end of the page), placeholder and hand-written notes alike.
# The compiler writes its list sections before the body, so only "## "
# headings above the first "## Body" (or "## Notes", for a page with no body)
# are taken as chrome; the same headings inside a markdown body are kept.
{ line[NR] = $0 }
!head && ($0 == "## Body" || $0 == "## Notes") { head = NR }
$0 == "## Notes" { notes = NR }
END {
    if (!head) head = NR + 1
    for (i = 1; i <= NR; i++) {
        if (notes && i >= notes) break
        if (i < head && line[i] ~ /^## /)
            skip = (line[i] == "## Imported by" || line[i] == "## Mentioned in")
        if (i >= head) skip = 0
        if (!skip) print line[i]
    }
}
