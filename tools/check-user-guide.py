#!/usr/bin/env python3
"""Check portable relative links, heading anchors and PNG references in the guide."""
from pathlib import Path
import re
from urllib.parse import unquote, urlsplit

ROOT=Path(__file__).resolve().parent.parent
GUIDE=ROOT/'docs/user-guide.md'
for label,href in re.findall(r'!?\[([^\]]+)\]\(([^)]+)\)',GUIDE.read_text()):
    url=urlsplit(href)
    if url.scheme or url.netloc:
        continue
    target=(GUIDE.parent/unquote(url.path)).resolve() if url.path else GUIDE
    assert target.is_relative_to(ROOT) and target.is_file(),href
    if url.fragment:
        headings=re.findall(r'^#+\s+(.+)$',target.read_text(),re.MULTILINE)
        anchors=[re.sub(r'[^\w\- ]','',text.lower()).replace(' ','-') for text in headings]
        assert unquote(url.fragment) in anchors,href
    if target.suffix=='.png':
        assert label.strip() and target.read_bytes().startswith(b'\x89PNG\r\n\x1a\n'),href
print('User-guide relative links, anchors and screenshot assets passed')
