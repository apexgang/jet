# Attribute direct edits and submit reviews as turns

Files created or changed directly in the side panel are User edits submitted through `jetd`, attributed to the user, and included in the current diff and subsequent Change checkpoints without implying Harness authorship. Inline review comments remain a GUI-local draft until submission, when Jet creates one structured user turn containing the referenced files, lines, and comments and places it in the normal Turn queue.
