# Remove the compiler-owned "## Referenced By" and "## Notes" sections from a
# rendered wiki page. Uses the compiler's fixed section order so a markdown
# *source* body that contains those headings as content is not corrupted:
#   - Notes: the compiler's is always the LAST "## Notes" → strip it to EOF.
#   - Referenced By: the compiler's is the FIRST "## Referenced By" (it precedes
#     the body; earlier sections are bullet/field content with no "## " lines) →
#     strip from it through the line before the next "## " heading.
{ line[NR] = $0 }
$0 == "## Notes" { last_notes = NR }
END {
    for (i = 1; i <= NR; i++) {
        if (!rb_start && line[i] == "## Referenced By") { rb_start = i; continue }
        if (rb_start && !rb_end && line[i] ~ /^## /)     { rb_end = i; break }
    }
    if (rb_start && !rb_end) rb_end = NR + 1   # RefBy runs to EOF (no later heading)
    for (i = 1; i <= NR; i++) {
        if (last_notes && i >= last_notes) break               # drop Notes .. EOF
        if (rb_start && i >= rb_start && i < rb_end) continue   # drop RefBy block
        print line[i]
    }
}
