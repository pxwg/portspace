#!/usr/bin/env python3
"""Pi server-profile MCP acceptance for images, Workspace bash, opt-in read-only tools."""
import base64
import pathlib
import struct
import tempfile
import time
import zlib
from e2e import Client


def chunk(kind, data):
    return struct.pack('!I', len(data)) + kind + data + struct.pack('!I', zlib.crc32(kind + data) & 0xffffffff)


def png(width=1, height=1):
    header = b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', struct.pack('!IIBBBBB', width, height, 8, 2, 0, 0, 0))
    return header + chunk(b'IDAT', zlib.compress((b'\0' + b'\xff\0\0' * width) * height)) + chunk(b'IEND', b'')


def initialize(client):
    client.rpc('initialize', {'protocolVersion': '2025-11-25', 'capabilities': {},
                             'clientInfo': {'name': 'pi-workspace-tests', 'version': '1'}})
    client.send('notifications/initialized', notification=True)


def invoke(client, name, error=False, **arguments):
    result = client.rpc('tools/call', {'name': name, 'arguments': arguments})
    assert bool(result.get('isError')) == error, (name, arguments, result)
    return result


def text(client, name, **arguments):
    return invoke(client, name, **arguments)['content'][0]['text']


def exercise(binary):
    with tempfile.TemporaryDirectory(prefix='portspace-pi-workspace.') as folder:
        root = pathlib.Path(folder)
        command = [binary, '--workspace', 'main='+folder, '--tool-profile', 'pi', '--tool-workspace', 'main']
        client = Client(command)
        try:
            initialize(client)
            assert {t['name'] for t in client.rpc('tools/list')['tools']} == {'read', 'write', 'edit', 'bash'}
            image = png()
            (root/'misleading.txt').write_bytes(image)
            result = invoke(client, 'read', path='misleading.txt')
            assert result['content'][0]['text'] == 'Read image file [image/png]'
            assert result['content'][1]['type'] == 'image' and result['content'][1]['mimeType'] == 'image/png'
            assert base64.b64decode(result['content'][1]['data']) == image
            (root/'large.png').write_bytes(png(2100, 1))
            result = invoke(client, 'read', path='large.png')
            assert 'Resized' in result['content'][0]['text']
            resized = base64.b64decode(result['content'][1]['data'])
            assert struct.unpack('!II', resized[16:24]) == (2000, 1)
            (root/'broken.png').write_bytes(b'\x89PNG\r\n\x1a\nbroken')
            invoke(client, 'read', path='broken.png', error=True)
            (root/'bomb.png').write_bytes(b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', struct.pack('!IIBBBBB', 16384, 16384, 8, 2, 0, 0, 0)) + chunk(b'IEND', b''))
            invoke(client, 'read', path='bomb.png', error=True)
            (root/'bytes.txt').write_bytes(b'hello\xff')
            assert text(client, 'read', path='bytes.txt') == 'hello\ufffd'
            assert text(client, 'bash', command="printf out; printf err >&2") == 'outerr'
            assert pathlib.Path(text(client, 'bash', command='pwd').strip()).resolve() == root.resolve()
            text(client, 'bash', command='mkdir sub; cd sub; export PORTSPACE_TRANSIENT_TEST=value')
            assert text(client, 'bash', command='printf %s "${PORTSPACE_TRANSIENT_TEST-unset}"') == 'unset'
            assert pathlib.Path(text(client, 'bash', command='pwd').strip()).resolve() == root.resolve()
            assert text(client, 'bash', command="printf '%s' \"literal 'quotes'; $((6*7))\"") == "literal 'quotes'; 42"
            result = invoke(client, 'bash', command='printf bad >&2; exit 7', error=True)
            assert 'bad' in result['content'][0]['text'] and 'code 7' in result['content'][0]['text']
            for timeout in (0, -1, 301): invoke(client, 'bash', command='true', timeout=timeout, error=True)
            assert text(client, 'bash', command='true') == '(no output)'
            result = text(client, 'bash', command="python3 -c \"print('界'*25000)\"")
            assert 'Showing last' in result and '\ufffd' not in result
            result = text(client, 'bash', command="python3 -c \"print('x'*1100000)\"")
            assert 'Output incomplete' in result and 'first' in result
            invoke(client, 'bash', command='(sleep 1; touch timeout-leak) & wait', timeout=0.05, error=True)
            rid = client.send('tools/call', {'name': 'bash', 'arguments': {'command': 'touch started; (sleep 1; touch cancel-leak) & wait'}})
            deadline = time.monotonic() + 5
            while not (root/'started').exists() and time.monotonic() < deadline: time.sleep(0.02)
            assert (root/'started').exists()
            client.send('notifications/cancelled', {'requestId': rid, 'reason': 'test'}, notification=True)
            assert text(client, 'bash', command='printf alive') == 'alive'
            time.sleep(1.1)
            assert not (root/'timeout-leak').exists() and not (root/'cancel-leak').exists()
        finally: client.close()
        # Optional tools are additive, independently selectable, and never shell aliases.
        client = Client(command + ['--pi-tools', 'ls'])
        try:
            initialize(client)
            assert {t['name'] for t in client.rpc('tools/list')['tools']} == {'read','write','edit','bash','ls'}
            assert 'error' in client.receive(client.send('tools/call', {'name':'grep','arguments':{'pattern':'x'}}))
        finally: client.close()

    with tempfile.TemporaryDirectory(prefix='portspace-pi-search.') as folder:
        root = pathlib.Path(folder)
        fixtures = {
            '.gitignore': 'ignored.txt\nbuild/\n*.log\n!important.log\n',
            '.ignore': 'vendor/\n', '.hidden.txt':'hidden\n',
            'a.txt':'before\nNeedle.foo\nafter\n', 'b.rs':'needle.foo\n', 'empty.txt':'',
            'ignored.txt':'Needle.foo', 'build/output':'Needle.foo', 'vendor/output':'Needle.foo',
            'bad.log':'Needle.foo', 'important.log':'Needle.foo', '.git/config':'Needle.foo',
            'src/x.rs':'Needle.foo\n', 'src/.gitignore':'skip.rs\n', 'src/skip.rs':'Needle.foo\n',
        }
        for name, value in fixtures.items():
            (root/name).parent.mkdir(parents=True, exist_ok=True)
            (root/name).write_text(value)
        (root/'binary').write_bytes(b'Needle.foo\0')
        (root/'link').symlink_to(root/'a.txt')
        client = Client([binary, '--workspace', 'main='+folder, '--tool-profile', 'pi', '--tool-workspace', 'main', '--pi-tools', 'grep,find,ls'])
        try:
            initialize(client)
            tools = client.rpc('tools/list')['tools']
            assert {t['name'] for t in tools} == {'read','write','edit','bash','grep','find','ls'}
            listing = text(client, 'ls')
            assert '.hidden.txt' in listing and 'src/' in listing and 'symlinks skipped' in listing
            assert 'entries limit reached' in text(client,'ls',limit=1)
            found = text(client,'find',pattern='*.rs')
            assert 'b.rs' in found and 'src/x.rs' in found and 'skip.rs' not in found
            found = text(client,'find',pattern='src/**/*.rs')
            assert 'src/x.rs' in found and 'b.rs' not in found
            all_found = text(client,'find',pattern='*')
            for ignored in ('ignored.txt','build/','vendor/','bad.log','.git/'):
                assert ignored not in all_found, all_found
            assert 'important.log' in all_found and '.hidden.txt' in all_found
            matched = text(client,'grep',pattern='needle.foo',literal=True,ignoreCase=True,glob='*.txt',context=1)
            assert 'a.txt-1- before\na.txt:2: Needle.foo\na.txt-3- after' in matched
            assert 'ignored.txt' not in matched
            matched = text(client,'grep',pattern='Needle.foo',literal=True)
            assert 'a.txt:2:' in matched and 'src/x.rs:1:' in matched and 'important.log:1:' in matched
            assert 'binary or oversized files skipped' in matched
            assert 'matches limit reached' in text(client,'grep',pattern='Needle',limit=1)
            assert text(client,'grep',path='empty.txt',pattern='.') == 'No matches found'
            assert 'No matches found' == text(client,'grep',path='a.txt',pattern='ABSENT')
            assert 'a.txt:2: Needle.foo' == text(client,'grep',path='/workspace/a.txt',pattern='Needle')
            for name, args in [('grep',{'pattern':'['}), ('grep',{'pattern':'x','context':1001}), ('find',{'pattern':'['}), ('ls',{'path':'a.txt'}), ('find',{'pattern':'*','path':'../outside'})]:
                invoke(client,name,error=True,**args)
            for name, args in [('grep',{'pattern':'x'}),('find',{'pattern':'*'}),('ls',{})]:
                invoke(client,name,error=True,limit=0,**args)
            # Closing MCP must kill running shell groups too.
            client.send('tools/call', {'name':'bash','arguments':{'command':'touch eof-started; (sleep 1; touch eof-leak) & wait'}})
            deadline = time.monotonic()+5
            while not (root/'eof-started').exists() and time.monotonic()<deadline: time.sleep(0.02)
            assert (root/'eof-started').exists()
        finally: client.close()
        time.sleep(1.1)
        assert not (root/'eof-leak').exists()
    print('PASS: MCP images, resize/limits, lossy text, Workspace bash/cwd/output/errors/timeout/cancel/EOF, opt-in grep/find/ls, ignores/globs/context/limits')
