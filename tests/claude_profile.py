#!/usr/bin/env python3
"""Deterministic MCP acceptance for the bounded Claude workspace subset (no model calls)."""
import argparse
import base64
import pathlib
import subprocess
import tempfile
from e2e import Client
from profiles import initialize, call
from pi_workspace import png


def exercise(binary):
    with tempfile.TemporaryDirectory(prefix='portspace-claude.') as folder:
        root = pathlib.Path(folder)
        bound = root / 'bound'
        bound.mkdir()
        (root / 'secret').write_text('outside')
        (bound / 'a.txt').write_bytes(b'\xef\xbb\xbfone\r\ntwo\r\none\r\n')
        cmd = [binary, '--workspace', 'outer='+folder, '--workspace', 'main='+str(bound),
               '--tool-profile', 'claude-code', '--tool-workspace', 'main']
        c = Client(cmd)
        def ok(name, **args):
            result = call(c, name, args)
            assert not result.get('isError'), result
            return result['content'][0]['text']
        def bad(name, **args):
            result = call(c, name, args)
            assert result.get('isError'), result
            return result
        try:
            initialize(c)
            tools = c.rpc('tools/list')['tools']
            assert {t['name'] for t in tools} == {'Read', 'Write', 'Edit', 'Glob', 'Grep', 'Bash'}
            schemas = {t['name']: t['inputSchema'] for t in tools}
            assert set(schemas['Edit']['required']) == {'file_path', 'old_string', 'new_string'}
            assert set(schemas['Read']['properties']) == {'file_path', 'offset', 'limit', 'pages'}
            for name in ('read', 'workspace_execute', 'workspace_list', 'TaskOutput', 'NotebookEdit'):
                assert 'error' in c.receive(c.send('tools/call', {'name': name, 'arguments': {}}))
            bad('Edit', file_path='a.txt', old_string='one', new_string='ONE')
            bad('Write', file_path='a.txt', content='no')
            assert ok('Read', file_path='/workspace/a.txt', offset=2, limit=1) == '     2→two\n[Output limited; continue with offset=3]'
            bad('Edit', file_path='a.txt', old_string='one', new_string='ONE')
            ok('Edit', file_path='./a.txt', old_string='one', new_string='ONE', replace_all=True)
            assert (bound/'a.txt').read_bytes() == b'\xef\xbb\xbfONE\r\ntwo\r\nONE\r\n'
            (bound/'a.txt').write_text('external change')
            bad('Edit', file_path='a.txt', old_string='external', new_string='INTERNAL')
            bad('Write', file_path='a.txt', content='no')
            ok('Read', file_path='a.txt')
            ok('Write', file_path='a.txt', content='hit\nmiddle\nhit\n')
            ok('Write', file_path='sub/b.txt', content='HIT\n')
            assert not (root/'sub').exists()
            (bound/'.gitignore').write_text('ignored\n')
            (bound/'ignored').mkdir()
            (bound/'ignored/x.txt').write_text('hit')
            (bound/'link.txt').symlink_to(root/'secret')
            result = ok('Glob', pattern='**/*.txt')
            assert '/workspace/a.txt' in result and '/workspace/sub/b.txt' in result
            assert 'ignored/' not in result and '/workspace/link.txt' not in result
            assert ok('Grep', pattern='hit', glob='*.txt').split('\n')[0] == '/workspace/a.txt'
            assert '/workspace/a.txt:2' in ok('Grep', pattern='hit', output_mode='count')
            assert '/workspace/sub/b.txt' in ok('Grep', pattern='hit', **{'-i': True})
            content = ok('Grep', pattern='hit', path='a.txt', output_mode='content', **{'-C': 1})
            assert content == '/workspace/a.txt:1:hit\n/workspace/a.txt:2:middle\n/workspace/a.txt:3:hit'
            assert ok('Grep', pattern='hit', path='a.txt', output_mode='content', offset=1, head_limit=1) == '/workspace/a.txt:3:hit'
            bad('Grep', pattern='x', multiline=True)
            bad('Grep', pattern='x', type='rust')
            bad('Grep', pattern='(?<=x)y')
            for path in ('../secret', '/tmp/x', '~/x', '/workspace/../secret', 'link.txt'):
                bad('Read', file_path=path)
                bad('Write', file_path=path, content='no')
            bad('Read', file_path='a.txt', offset=0)
            bad('Read', file_path='a.txt', pages='1')
            (bound/'doc.pdf').write_bytes(b'%PDF-1.7')
            bad('Read', file_path='doc.pdf')
            (bound/'notebook.ipynb').write_text('{}')
            bad('Read', file_path='notebook.ipynb')
            image = png()
            (bound/'image.png').write_bytes(image)
            result = call(c, 'Read', {'file_path': 'image.png'})
            assert not result.get('isError') and result['content'][1]['type'] == 'image'
            assert base64.b64decode(result['content'][1]['data']) == image
            bad('Write', file_path='image.png', content='not an image')
            (bound/'broken.png').write_bytes(b'\x89PNG\r\n\x1a\nbroken')
            bad('Read', file_path='broken.png')
            (bound/'long').write_text('中'*3000)
            assert '[line truncated]' in ok('Read', file_path='long')
            (bound/'huge').write_bytes(b'x'*(4*1024*1024+1))
            bad('Read', file_path='huge')
            (bound/'many').write_text('x'*10000)
            ok('Read', file_path='many')
            bad('Edit', file_path='many', old_string='x', new_string='y'*10000, replace_all=True)
            assert (bound/'many').stat().st_size == 10000
            assert str(bound) in ok('Bash', command='pwd', timeout=1000)
            ok('Bash', command='cd sub')
            assert str(bound) in ok('Bash', command='pwd')
            bad('Bash', command='sleep 1', timeout=10)
            bad('Bash', command='exit 7')
            for option in ('run_in_background', 'dangerouslyDisableSandbox'):
                bad('Bash', command='touch forbidden', **{option: True})
            bad('Bash', command='touch forbidden', timeout=300001)
            assert not (bound/'forbidden').exists()
            # A second server session cannot reuse another session's read snapshots.
            c2 = Client(cmd)
            try:
                initialize(c2)
                assert call(c2, 'Write', {'file_path': 'a.txt', 'content': 'no'})['isError']
            finally:
                c2.close()
            for i in range(17):
                ok('Write', file_path=f'cache/{i:02}', content='x')
            bad('Write', file_path='a.txt', content='evicted')
            assert (root/'secret').read_text() == 'outside'
        finally:
            c.close()
        proc = subprocess.run(cmd + ['--pi-tools', 'grep'], input='', text=True, capture_output=True, timeout=5)
        assert proc.returncode and not proc.stdout
    print('PASS: Claude profile discovery, schemas, binding, read freshness, exact edits, search modes, limits, safe failures, shell semantics')


if __name__ == '__main__':
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--binary', required=True)
    exercise(str(pathlib.Path(p.parse_args().binary).resolve()))
