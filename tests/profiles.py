#!/usr/bin/env python3
"""MCP profile acceptance, optionally compared with installed native Pi 0.87.1."""
import argparse
import json
import pathlib
import subprocess
import tempfile
from e2e import Client


def initialize(client):
    client.rpc('initialize', {'protocolVersion': '2025-11-25', 'capabilities': {},
                             'clientInfo': {'name': 'profile-tests', 'version': '1'}})
    client.send('notifications/initialized', notification=True)


def call(client, name, arguments):
    return client.rpc('tools/call', {'name': name, 'arguments': arguments})


def cases():
    values = []
    def add(tool, files, args):
        values.append(dict(tool=tool, files=files, args=args))
    for text, offset, limit in [('', None, None), ('a\nb\n', 2, 1), ('a\nb\n', 3, None),
                                 ('a', 9, None), ('a\nb', 0, 0), ('中\n' * 3000, None, None),
                                 ('中' * 10000 + '\n' + '文' * 10000, None, None),
                                 ('a\r\nb\r\n', None, None)]:
        args = {'path': 'a'}
        if offset is not None: args['offset'] = offset
        if limit is not None: args['limit'] = limit
        add('read', {'a': text}, args)
    add('write', {}, {'path': 'deep/nested/a', 'content': '远程思考，本地执行。\n'})
    add('write', {'a': 'old'}, {'path': 'a', 'content': ''})
    for original, edits in [
        ('a\nb\n', [{'oldText': 'a', 'newText': 'A'}, {'oldText': 'b', 'newText': 'B'}]),
        ('\ufeffa\r\nb\r\n', [{'oldText': 'a\nb', 'newText': 'A\nB'}]),
        ('x\nx', [{'oldText': 'x', 'newText': 'y'}]),
        ('abcd', [{'oldText': 'abc', 'newText': 'A'}, {'oldText': 'bcd', 'newText': 'B'}]),
        ('a\nb', [{'oldText': 'a', 'newText': 'A'}, {'oldText': 'absent', 'newText': 'B'}]),
        ('a', [{'oldText': 'a', 'newText': 'a'}]),
        ('a', []),
        ('a', [{'oldText': '', 'newText': 'b'}]),
    ]:
        add('edit', {'a': original}, {'path': 'a', 'edits': edits})
    for original, edits, error in [
        ('“hello”\nuntouched — line   \n', [{'oldText': '"hello"', 'newText': 'hi'}], False),
        ('a   \nb\t\n', [{'oldText': 'a\nb', 'newText': 'A\nB'}], False),
        ('ＡＢＣ K ﬁ\n', [{'oldText': 'ABC K fi', 'newText': 'normalized'}], False),
        ('a\u00a0b — c\n', [{'oldText': 'a b - c', 'newText': 'x'}], False),
        ('a\n“b”\nc   \n“d”\ne   \n', [{'oldText': 'a', 'newText': 'A'}, {'oldText': '"b"', 'newText': 'B'}, {'oldText': '"d"', 'newText': 'D'}], False),
        ('"x"\n“x”\n', [{'oldText': '"x"', 'newText': 'y'}], True),
        ('\ufeff“x”\r\nunchanged  \r\n', [{'oldText': '"x"', 'newText': 'y\nz'}], False),
        ('e\u0301\n', [{'oldText': 'é', 'newText': 'E'}], False),
        ('“foo” “bar”\nkeep  \n', [{'oldText': '"foo"', 'newText': 'F'}, {'oldText': '"bar"', 'newText': 'B'}], False),
        ('“abc”\n', [{'oldText': '"abc"', 'newText': 'X'}, {'oldText': 'abc', 'newText': 'Y'}], True),
    ]:
        add('edit', {'a': original}, {'path': 'a', 'edits': edits})
        values[-1]['error'] = error
    for i in (3, 12, 13, 14, 15, 16, 17): values[i]['error'] = True
    return values


def run_case(binary, case):
    with tempfile.TemporaryDirectory(prefix='portspace-profile.') as folder:
        root = pathlib.Path(folder)
        for path, text in case['files'].items():
            (root/path).parent.mkdir(parents=True, exist_ok=True)
            (root/path).write_bytes(text.encode())
        client = Client([binary, '--workspace', 'main='+folder, '--tool-profile', 'pi', '--tool-workspace', 'main'])
        try:
            initialize(client)
            result = call(client, case['tool'], case['args'])
            observed = dict(isError=bool(result.get('isError')), text=result['content'][0]['text'], files={})
            for path in set(case['files']) | {case['args']['path']}:
                if (root/path).is_file(): observed['files'][path] = (root/path).read_bytes().decode()
            return observed
        finally: client.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', required=True)
    parser.add_argument('--pi-package', help='Optional installed Pi 0.87.1 package directory (no network/model calls)')
    args = parser.parse_args()
    binary = str(pathlib.Path(args.binary).resolve())
    fixtures = cases()
    results = [run_case(binary, case) for case in fixtures]
    for case, result in zip(fixtures, results):
        assert result['isError'] == case.get('error', False), (case, result)
        if result['isError']: assert result['files'] == case['files']
    assert results[10]['files']['a'] == 'A\nB\n'
    assert results[11]['files']['a'] == '\ufeffA\r\nB\r\n'

    with tempfile.TemporaryDirectory(prefix='portspace-profile-routing.') as folder:
        root = pathlib.Path(folder)
        (root/'other').mkdir()
        command = [binary, '--workspace', 'main='+folder, '--workspace', 'other='+folder+'/other',
                   '--tool-profile', 'pi', '--tool-workspace', 'other']
        client = Client(command)
        try:
            initialize(client)
            tools = client.rpc('tools/list')['tools']
            assert {t['name'] for t in tools} == {'read', 'write', 'edit', 'bash'}
            schemas = {t['name']: t['inputSchema'] for t in tools}
            assert set(schemas['read']['properties']) == {'path', 'offset', 'limit'}
            assert set(schemas['write']['required']) == {'path', 'content'}
            assert set(schemas['edit']['required']) == {'path', 'edits'}
            assert not call(client, 'write', {'path': '@/workspace/a', 'content': 'bound'})['isError']
            assert (root/'other/a').read_text() == 'bound' and not (root/'a').exists()
            assert call(client, 'read', {'path': 'a'})['content'][0]['text'] == 'bound'
            for path in ('../a', '/tmp/a', '~/a', '/workspace/../a', 'a/../a'):
                result = call(client, 'write', {'path': path, 'content': 'no'})
                assert result['isError'] and result['structuredContent']['error']['code'] == 'invalid_path'
            (root/'other/link').symlink_to(root/'other/a')
            assert call(client, 'read', {'path': 'link'})['isError']
            for name in ('workspace_execute', 'workspace_list', 'grep', 'find', 'ls', 'Bash', 'apply_patch'):
                assert 'error' in client.receive(client.send('tools/call', {'name': name, 'arguments': {}}))
            (root/'other/a').write_text('“hello”')
            assert not call(client, 'edit', {'path': 'a', 'edits': [{'oldText': '"hello"', 'newText': 'hi'}]})['isError']
            assert (root/'other/a').read_text() == 'hi'
            (root/'other/a').write_bytes(b'x'*(4*1024*1024+1))
            assert call(client, 'read', {'path': 'a'})['structuredContent']['error']['code'] == 'limit_exceeded'
            (root/'other/a').write_text('中'*18000)
            assert call(client, 'read', {'path': 'a'})['structuredContent']['error']['code'] == 'limit_exceeded'
        finally: client.close()
        for extra in [('--tool-profile', 'pi'), ('--tool-profile', 'codex'), ('--tool-profile', 'claude-code'),
                      ('--tool-profile', 'pi', '--tool-workspace', 'absent'), ('--tool-workspace', 'main'),
                      ('--pi-tools', 'grep'), ('--tool-profile', 'pi', '--tool-workspace', 'main', '--pi-tools', 'bash'),
                      ('--tool-profile', 'pi', '--tool-workspace', 'main', '--pi-tools', '')]:
            proc = subprocess.run([binary, '--workspace', 'main='+folder, *extra], input='', text=True, capture_output=True, timeout=5)
            assert proc.returncode != 0 and not proc.stdout, (extra, proc)
    print(f'PASS: {len(fixtures)} Pi file cases, schemas, profile isolation, explicit binding, limits, safe failures, CLI validation')
    from pi_workspace import exercise
    exercise(binary)
    if args.pi_package:
        ref = subprocess.run(['node', str(pathlib.Path(__file__).with_name('pi_reference.mjs')), args.pi_package],
                             input=json.dumps(fixtures), text=True, capture_output=True, check=True, timeout=60)
        native = json.loads(ref.stdout)
        assert len(native) == len(results)
        for i, (actual, expected) in enumerate(zip(results, native)):
            assert actual['isError'] == expected['isError'], (i, actual, expected)
            assert actual['files'] == expected['files'], (i, actual, expected)
            if not actual['isError']: assert actual['text'] == expected['text'], (i, actual, expected)
        print(f'PASS: {len(fixtures)} differential cases against native Pi 0.87.1 (success text, error status, resulting file bytes)')


if __name__ == '__main__': main()
