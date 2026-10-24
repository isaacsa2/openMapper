#!/usr/bin/env python3
"""Check embedded catalogues against literal tr() and command-palette labels."""
import json
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
PAIR = re.compile(r'\b(?:tr\(\s*|cmd!\(\s*Icon::\w+\s*,\s*)("(?:[^"\\]|\\.)*")\s*,\s*("(?:[^"\\]|\\.)*")', re.S)
source = set()
for path in (ROOT / 'src').rglob('*.rs'):
    for match in PAIR.finditer(path.read_text()):
        source.add(json.loads(match[1]))
for language in ('en', 'ru', 'pt-BR', 'pt-PT', 'de'):
    entries = json.loads((ROOT / 'locales' / (language + '.json')).read_text())
    missing = source - entries.keys()
    extra = entries.keys() - source
    assert not missing and not extra, (language, 'missing', sorted(missing), 'obsolete', sorted(extra))
    for key, value in entries.items():
        assert value.strip(), (language, 'empty', key)
        if language == 'ru':
            continue  # Context-specific Russian text stays inline in tr().
        for token in re.findall(r'\{[^{}]*\}|\bOPENMAPPER_[A-Z_]+\b|https?://\S+', key):
            assert token in value, (language, 'changed literal', token, key)
        assert '▁' not in value, (language, 'tokenisation marker', key)
    print(f'{language}: {len(entries)} source labels covered')
