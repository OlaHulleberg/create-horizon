#!/usr/bin/env python3
"""Record source and corpus provenance without inspecting runtime dependencies."""
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys


def revision(path):
    return subprocess.check_output(['git', '-C', str(path), 'rev-parse', 'HEAD'], text=True).strip()


def corpus_digest(path):
    digest = hashlib.sha256()
    files = sorted(Path(path).glob('*.json'))
    for file in files:
        digest.update(file.name.encode() + b'\0' + file.read_bytes() + b'\0')
    return len(files), digest.hexdigest()


def main():
    report = Path(os.environ.get('FUZZ_COVERAGE_REPORT', str(Path(os.environ['FUZZ_DIR']) / 'coverage')))
    path = report / 'campaign.json'
    if sys.argv[1] == 'complete':
        data = json.loads(path.read_text())
        data['complete'] = True
        data['never_reached_sha256'] = hashlib.sha256((report / 'never-reached.tsv').read_bytes()).hexdigest()
    else:
        root, engine, create, rounds = sys.argv[2:]
        count, digest = corpus_digest(os.environ.get('FUZZ_CORPUS', 'corpus'))
        data = dict(fuzzer=revision(root),
                    engine=os.environ.get('FUZZ_ENGINE_REVISION') or revision(engine),
                    create=revision(create),
                    command=f'./run.sh fuzz cover {rounds}',
                    includes=os.environ['FUZZ_COVERAGE_INCLUDES'],
                    port=os.environ.get('FUZZ_PORT', '25599'),
                    corpus_scenarios=count, corpus_sha256=digest, complete=False)
        report.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(data, indent=2, sort_keys=True) + '\n')


if __name__ == '__main__':
    main()
